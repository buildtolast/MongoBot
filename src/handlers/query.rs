use actix_web::{web, HttpResponse};
use std::time::Instant;

use crate::domain::executor;
use crate::domain::models::{ConfirmRequest, QueryRequest, QueryResponse};
use crate::domain::query_advisor;
use crate::error::AppError;
use crate::state::AppState;

/// Handle a natural-language query: parse via LLM, validate, execute, return results.
pub async fn handle_query(
    state: web::Data<AppState>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, AppError> {
    let start = Instant::now();

    // 1. Get schema context for the LLM
    let schema_context = state.schema_context();

    // 2. Parse natural language into a structured query via LLM
    let model_override = body.model.as_deref();
    let parsed = state
        .llm
        .parse_query(&body.query, &schema_context, model_override)
        .await?;

    tracing::info!(
        collection = %parsed.collection,
        operation = ?parsed.operation,
        filters = parsed.filters.len(),
        projection = ?parsed.projection,
        "Parsed query from NL"
    );

    // 3. Validate query safety (with optional schema context)
    let schema = state.get_schema(&parsed.collection);
    state.safety.validate(&parsed, schema.as_ref())?;

    // 4a. Preview mode: return parsed query without executing
    if body.preview {
        let mongo_query = executor::build_mongo_shell_command(&parsed);
        let execution_ms = start.elapsed().as_millis() as u64;
        let index_analysis = Some(executor::analyze_index_usage(&parsed, schema.as_ref()));
        let advice = query_advisor::analyze_query(&parsed, schema.as_ref());
        return Ok(HttpResponse::Ok().json(QueryResponse {
            parsed,
            results: serde_json::json!(null),
            count: 0,
            execution_ms,
            mongo_query,
            index_analysis,
            advice,
            requires_confirmation: false,
            affected_count: None,
        }));
    }

    // 4. For write operations, return a confirmation request instead of executing.
    //    For update/delete: preview affected documents so user can backup first.
    if parsed.operation.is_write() {
        let mongo_query = executor::build_mongo_shell_command(&parsed);

        // For update/delete: query affected docs as preview for backup
        let (preview_results, preview_count) = if parsed.operation == crate::domain::models::QueryOperation::Update
            || parsed.operation == crate::domain::models::QueryOperation::Delete
        {
            let filter = executor::build_filter(&parsed)?;
            match state.mongo.find_documents(
                &parsed.collection, Some(filter), None, Some(100), None,
            ).await {
                Ok(docs) => {
                    let count = docs.len();
                    let json_docs: Vec<serde_json::Value> = docs.iter()
                        .filter_map(|d| {
                            let bson = mongodb::bson::Bson::Document(d.clone());
                            serde_json::to_value(&bson).ok()
                        })
                        .collect();
                    (serde_json::json!(json_docs), count)
                }
                Err(_) => (serde_json::json!([]), 0),
            }
        } else {
            (serde_json::json!(null), 0)
        };

        let execution_ms = start.elapsed().as_millis() as u64;

        let advice = query_advisor::analyze_query(&parsed, schema.as_ref());
        let response = QueryResponse {
            parsed,
            results: preview_results,
            count: preview_count,
            execution_ms,
            mongo_query,
            index_analysis: None,
            advice,
            requires_confirmation: true,
            affected_count: Some(preview_count as u64),
        };

        return Ok(HttpResponse::Ok().json(response));
    }

    // 5. Execute the read query against MongoDB
    let result = executor::execute(&state.mongo, &parsed).await?;

    // 6. Build response with index analysis
    let count = query_result_len(&result);
    let results_json = query_result_to_json(&result);
    let execution_ms = start.elapsed().as_millis() as u64;
    let mongo_query = executor::build_mongo_shell_command(&parsed);
    let index_analysis = Some(executor::analyze_index_usage(&parsed, schema.as_ref()));

    let advice = query_advisor::analyze_query(&parsed, schema.as_ref());
    let response = QueryResponse {
        parsed,
        results: results_json,
        count,
        execution_ms,
        mongo_query,
        index_analysis,
        advice,
        requires_confirmation: false,
        affected_count: None,
    };

    Ok(HttpResponse::Ok().json(response))
}

/// POST /api/execute — Execute a confirmed query (writes or approved reads).
pub async fn handle_execute(
    state: web::Data<AppState>,
    body: web::Json<ConfirmRequest>,
) -> Result<HttpResponse, AppError> {
    if !body.confirm {
        return Err(AppError::Validation("Confirmation required".into()));
    }

    let parsed = &body.parsed;

    // Validate safety
    let schema = state.get_schema(&parsed.collection);
    state.safety.validate(parsed, schema.as_ref())?;

    // Execute the query
    let start = Instant::now();
    let result = executor::execute(&state.mongo, parsed).await?;
    let execution_ms = start.elapsed().as_millis() as u64;

    let mongo_query = executor::build_mongo_shell_command(parsed);
    let index_analysis = Some(executor::analyze_index_usage(parsed, schema.as_ref()));
    let advice = query_advisor::analyze_query(parsed, schema.as_ref());

    // Build response
    let (count, results_json, affected_count) = match &result {
        executor::QueryResult::Write(w) => {
            let affected = w.matched_count.max(w.modified_count);
            let json = serde_json::json!({
                "operation": w.operation,
                "collection": w.collection,
                "matched_count": w.matched_count,
                "modified_count": w.modified_count,
                "inserted_id": w.inserted_id,
            });
            (1, json, Some(affected))
        }
        other => (query_result_len(other), query_result_to_json(other), None),
    };

    let response = QueryResponse {
        parsed: parsed.clone(),
        results: results_json,
        count,
        execution_ms,
        mongo_query,
        index_analysis,
        advice,
        requires_confirmation: false,
        affected_count,
    };

    Ok(HttpResponse::Ok().json(response))
}

/// Get the number of items in a QueryResult.
pub(crate) fn query_result_len(result: &executor::QueryResult) -> usize {
    match result {
        executor::QueryResult::Documents(docs) => docs.len(),
        executor::QueryResult::Count(_) => 1,
        executor::QueryResult::Values(vals) => vals.len(),
        executor::QueryResult::Write(_) => 1,
    }
}

/// Convert a QueryResult to a serde_json::Value.
pub(crate) fn query_result_to_json(result: &executor::QueryResult) -> serde_json::Value {
    match result {
        executor::QueryResult::Documents(docs) => {
            let json_docs: Vec<serde_json::Value> = docs
                .iter()
                .filter_map(|d| {
                    let bson = mongodb::bson::Bson::Document(d.clone());
                    serde_json::to_value(&bson).ok()
                })
                .collect();
            serde_json::json!(json_docs)
        }
        executor::QueryResult::Count(n) => serde_json::json!({ "count": n }),
        executor::QueryResult::Values(vals) => {
            let json_vals: Vec<serde_json::Value> = vals
                .iter()
                .filter_map(|v| serde_json::to_value(v).ok())
                .collect();
            serde_json::json!(json_vals)
        }
        executor::QueryResult::Write(w) => {
            serde_json::json!({
                "operation": w.operation,
                "collection": w.collection,
                "matched_count": w.matched_count,
                "modified_count": w.modified_count,
                "inserted_id": w.inserted_id,
            })
        }
    }
}

