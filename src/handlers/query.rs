use actix_web::{web, HttpResponse};
use std::time::Instant;

use crate::domain::executor;
use crate::domain::models::{QueryRequest, QueryResponse};
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
    let parsed = state
        .llm
        .parse_query(&body.query, &schema_context)
        .await?;

    tracing::info!(
        collection = %parsed.collection,
        operation = ?parsed.operation,
        "Parsed query from NL"
    );

    // 3. Validate query safety (with optional schema context)
    let schema = state.get_schema(&parsed.collection);
    state.safety.validate(&parsed, schema.as_ref())?;

    // 4. Execute the query against MongoDB
    let result = executor::execute(&state.mongo, &parsed).await?;

    // 5. Build response with index analysis
    let count = query_result_len(&result);
    let results_json = query_result_to_json(&result);
    let execution_ms = start.elapsed().as_millis() as u64;
    let mongo_query = executor::build_mongo_shell_command(&parsed);
    let index_analysis = Some(executor::analyze_index_usage(&parsed, schema.as_ref()));

    let response = QueryResponse {
        parsed,
        results: results_json,
        count,
        execution_ms,
        mongo_query,
        index_analysis,
    };

    Ok(HttpResponse::Ok().json(response))
}

/// Get the number of items in a QueryResult.
pub(crate) fn query_result_len(result: &executor::QueryResult) -> usize {
    match result {
        executor::QueryResult::Documents(docs) => docs.len(),
        executor::QueryResult::Count(_) => 1,
        executor::QueryResult::Values(vals) => vals.len(),
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
    }
}

