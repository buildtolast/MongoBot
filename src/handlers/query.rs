use actix_web::{web, HttpResponse};
use std::time::Instant;
use chrono::Utc;

use crate::domain::executor;
use crate::domain::models::{ConfirmRequest, ExecutionTiming, ParsedQuery, QueryRequest, QueryResponse, FeedbackType, QueryFeedback};
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
    let provider_override = body.provider.as_deref()
        .map(crate::infra::llm::parse_provider_str)
        .transpose()?;

    let llm_start = Instant::now();

    // --- Few-shot retrieval ---
    let mut examples = Vec::new();
    if state.config.enable_feedback_retrieval {
        tracing::debug!(query = %body.query, top_k = state.config.feedback_top_k, "few-shot retrieval enabled");
        match state.llm.embed_text(&body.query).await {
            Ok(embedding) => {
                if let Ok(similar_docs) = state.mongo.find_similar_feedback(&embedding, state.config.feedback_top_k).await {
                    examples = similar_docs.into_iter()
                        .filter_map(|doc| mongodb::bson::from_document::<QueryFeedback>(doc).ok())
                        .collect();
                    if !examples.is_empty() {
                        tracing::info!(examples_retrieved = examples.len(), "injecting few-shot examples into prompt");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "embed_text failed; proceeding without few-shot");
            }
        }
    } else {
        tracing::trace!("few-shot retrieval disabled by config");
    }

    let prompt = crate::infra::llm::build_prompt_with_examples(&body.query, &schema_context, &examples);
    let (parsed, raw_llm_response, model_used) = state
        .llm
        .call_llm_with_prompt(&prompt, model_override, provider_override).await?;
    let llm_parse_ms = llm_start.elapsed().as_millis() as u64;

    tracing::info!(
        collection = %parsed.collection,
        operation = ?parsed.operation,
        filters = parsed.filters.len(),
        projection = ?parsed.projection,
        model = %model_used,
        llm_ms = llm_parse_ms,
        "Parsed query from NL"
    );

    // 3. Validate query safety (with optional schema context)
    let safety_start = Instant::now();
    let schema = state.get_schema(&parsed.collection);
    state.safety.validate_with_join(&parsed, schema.as_ref(), state.join_schema(&parsed).as_ref())?;
    let safety_check_ms = safety_start.elapsed().as_millis() as u64;

    // 4a. Preview mode (or query-only mode): return parsed query without executing
    // For write operations, still preview affected documents so backup is available
    if body.preview || state.config.query_only {
        if parsed.operation.is_write() && !state.config.query_only {
            // Fall through to the write handler below which previews affected docs
        } else {
            let resp_start = Instant::now();
            let mongo_query = executor::build_mongo_shell_command(&parsed);
            let index_analysis = Some(executor::analyze_index_usage(&parsed, schema.as_ref()));
            let advice = query_advisor::analyze_query(&parsed, schema.as_ref());
            let response_build_ms = resp_start.elapsed().as_millis() as u64;
            let total_ms = start.elapsed().as_millis() as u64;
            return Ok(HttpResponse::Ok().json(QueryResponse {
                parsed,
                results: serde_json::json!(null),
                count: 0,
                execution_ms: total_ms,
                mongo_query: mongo_query.clone(),
                index_analysis,
                advice,
                requires_confirmation: false,
                affected_count: None,
                raw_llm_response: Some(raw_llm_response.clone()),
                timing: Some(ExecutionTiming {
                    llm_parse_ms,
                    model_used: model_used.clone(),
                    safety_check_ms,
                    mongo_exec_ms: 0,
                    response_build_ms,
                    total_ms,
                }),
                schema_snapshot: Some(state.schema_context()),
            }));
        }
    }

    // 4. For write operations, return a confirmation request instead of executing.
    //    For update/delete: preview affected documents so user can backup first.
    if parsed.operation.is_write() {
        let mongo_start = Instant::now();
        let mongo_query = executor::build_mongo_shell_command(&parsed);

        // For update/delete: query affected docs as preview for backup
        let (preview_results, preview_count) = if parsed.operation == crate::domain::models::QueryOperation::Update
            || parsed.operation == crate::domain::models::QueryOperation::Delete
        {
            let filter = executor::build_filter(&parsed, schema.as_ref())?;
            match state.mongo.find_documents(
                &parsed.collection, Some(filter), None, Some(100), None,
            ).await {
                Ok(docs) => {
                    let count = docs.len();
                    let json_docs: Vec<serde_json::Value> = docs.iter()
                        .map(bson_doc_to_json)
                        .collect();
                    (serde_json::json!(json_docs), count)
                }
                Err(_) => (serde_json::json!([]), 0),
            }
        } else {
            (serde_json::json!(null), 0)
        };
        let mongo_exec_ms = mongo_start.elapsed().as_millis() as u64;

        let resp_start = Instant::now();
        let advice = query_advisor::analyze_query(&parsed, schema.as_ref());
        let response_build_ms = resp_start.elapsed().as_millis() as u64;
        let total_ms = start.elapsed().as_millis() as u64;

        let response = QueryResponse {
            parsed,
            results: preview_results,
            count: preview_count,
            execution_ms: total_ms,
            mongo_query: mongo_query.clone(),
            index_analysis: None,
            advice,
            requires_confirmation: true,
            affected_count: Some(preview_count as u64),
            raw_llm_response: Some(raw_llm_response.clone()),
            timing: Some(ExecutionTiming {
                llm_parse_ms,
                model_used,
                safety_check_ms,
                mongo_exec_ms,
                response_build_ms,
                total_ms,
            }),
            schema_snapshot: Some(state.schema_context()),
        };

        return Ok(HttpResponse::Ok().json(response));
    }

    // 5. Execute the read query against MongoDB
    let mongo_start = Instant::now();
    let result = executor::execute(&state.mongo, &parsed, schema.as_ref())
        .await
        .inspect_err(|e| log_execution_failure(&parsed, e))?;
    let mongo_exec_ms = mongo_start.elapsed().as_millis() as u64;

    // 6. Build response with index analysis
    let resp_start = Instant::now();
    let count = query_result_len(&result);
    let results_json = query_result_to_json(&result);
    let mongo_query = executor::build_mongo_shell_command(&parsed);
    let index_analysis = Some(executor::analyze_index_usage(&parsed, schema.as_ref()));
    let advice = query_advisor::analyze_query(&parsed, schema.as_ref());
    let response_build_ms = resp_start.elapsed().as_millis() as u64;
    let total_ms = start.elapsed().as_millis() as u64;

    let response = QueryResponse {
        parsed,
        results: results_json,
        count,
        execution_ms: total_ms,
        mongo_query: mongo_query.clone(),
        index_analysis,
        advice,
        requires_confirmation: false,
        affected_count: None,
        raw_llm_response: Some(raw_llm_response),
        timing: Some(ExecutionTiming {
            llm_parse_ms,
            model_used,
            safety_check_ms,
            mongo_exec_ms,
            response_build_ms,
            total_ms,
            }),
            schema_snapshot: Some(state.schema_context()),
        };

    Ok(HttpResponse::Ok().json(response))
}

/// Log the query that MongoDB rejected; the client only receives a sanitized error.
fn log_execution_failure(parsed: &ParsedQuery, error: &AppError) {
    tracing::warn!(
        error = %error,
        query = %serde_json::to_string(parsed).unwrap_or_default(),
        "Query execution failed"
    );
}

/// POST /api/execute — Execute a confirmed query (writes or approved reads).
pub async fn handle_execute(
    state: web::Data<AppState>,
    body: web::Json<ConfirmRequest>,
) -> Result<HttpResponse, AppError> {
    if !body.confirm {
        return Err(AppError::Validation("Confirmation required".into()));
    }

    // Query-only mode: block all execution
    if state.config.query_only {
        return Err(AppError::Validation(
            "Query-only mode is enabled (QUERY_ONLY=true). Queries are parsed but not executed.".into()
        ));
    }

    let parsed = &body.parsed;

    // Validate safety
    let schema = state.get_schema(&parsed.collection);
    state.safety.validate_with_join(parsed, schema.as_ref(), state.join_schema(parsed).as_ref())?;

    // Execute the query
    let start = Instant::now();
    let result = executor::execute(&state.mongo, parsed, schema.as_ref())
        .await
        .inspect_err(|e| log_execution_failure(parsed, e))?;
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

    // Build response
    let response = QueryResponse {
        parsed: parsed.clone(),
        results: results_json,
        count,
        execution_ms,
        mongo_query: mongo_query.clone(),
        index_analysis,
        advice,
        requires_confirmation: false,
        affected_count,
        raw_llm_response: None,
        timing: None,
        schema_snapshot: Some(state.schema_context()),
    };

    // --- Auto-capture feedback if a correction was made ---
    if let (Some(original), Some(ctx)) = (&body.original_parsed, &body.context) {
        if ctx.nl_query.len() > 4000 {
            return Err(AppError::Validation("nl_query exceeds 4000 characters".into()));
        }
        if original != parsed {
            let embedding = match state.llm.embed_text(&ctx.nl_query).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::error!(error = %e, nl_query = %ctx.nl_query, "Failed to generate embedding for auto-captured feedback");
                    None
                }
            };
            let feedback = QueryFeedback {
                id: mongodb::bson::oid::ObjectId::new(),
                timestamp: Utc::now(),
                nl_query: ctx.nl_query.clone(),
                provider: ctx.provider.clone(),
                model: ctx.model.clone(),
                raw_llm_response: ctx.raw_llm_response.clone(),
                parsed_query: original.clone(),
                user_edited_query: Some(parsed.clone()),
                mongo_query: mongo_query.clone(),
                feedback: FeedbackType::Corrected,
                note: None,
                executed: true,
                result_count: count,
                schema_snapshot: state.schema_context(),
                embedding,
            };

            if let Ok(bson_doc) = mongodb::bson::to_document(&feedback) {
                let _ = state.mongo.insert_one("query_feedback", bson_doc).await;
            }
        }
    }

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
/// BSON DateTime values are formatted as ISO 8601 strings (YYYY-MM-DDTHH:MM:SS.SSSZ).
pub(crate) fn query_result_to_json(result: &executor::QueryResult) -> serde_json::Value {
    match result {
        executor::QueryResult::Documents(docs) => {
            let json_docs: Vec<serde_json::Value> = docs
                .iter()
                .map(bson_doc_to_json)
                .collect();
            serde_json::json!(json_docs)
        }
        executor::QueryResult::Count(n) => serde_json::json!({ "count": n }),
        executor::QueryResult::Values(vals) => {
            let json_vals: Vec<serde_json::Value> = vals
                .iter()
                .map(bson_to_json)
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

/// Convert a BSON Document to JSON with DateTime formatted as ISO 8601 strings.
fn bson_doc_to_json(doc: &mongodb::bson::Document) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, val) in doc {
        map.insert(key.clone(), bson_to_json(val));
    }
    serde_json::Value::Object(map)
}

/// Handler tests — actix-web integration paths (no real MongoDB/LLM).
#[cfg(test)]
mod handler_tests {
    use super::*;
    use actix_web::{test, web, App};
    use crate::domain::models::{ParsedQuery, QueryOperation};
    use crate::test_helpers::test_app_state;

    fn confirm_req(parsed: ParsedQuery, confirm: bool) -> serde_json::Value {
        serde_json::json!({ "parsed": parsed, "confirm": confirm })
    }

    fn find_query() -> ParsedQuery {
        ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Find,
            ..Default::default()
        }
    }

    #[actix_web::test]
    async fn handle_execute_rejects_when_confirm_false() {
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/execute", web::post().to(handle_execute))
        ).await;
        let req = test::TestRequest::post()
            .uri("/api/execute")
            .set_json(&confirm_req(find_query(), false))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 400);
    }

    #[actix_web::test]
    async fn handle_execute_rejects_in_query_only_mode() {
        let mut state = test_app_state().await;
        state.config.query_only = true;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/execute", web::post().to(handle_execute))
        ).await;
        let req = test::TestRequest::post()
            .uri("/api/execute")
            .set_json(&confirm_req(find_query(), true))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 400);
    }

    #[actix_web::test]
    async fn handle_execute_rejects_disallowed_operation() {
        // insert is not in test_config allowed_operations → SafetyError → 403
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/execute", web::post().to(handle_execute))
        ).await;
        let parsed = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Insert,
            document: Some(serde_json::json!({"title": "Test"})),
            ..Default::default()
        };
        let req = test::TestRequest::post()
            .uri("/api/execute")
            .set_json(&confirm_req(parsed, true))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 403);
    }

    #[actix_web::test]
    async fn handle_execute_succeeds_with_find_query_when_mongo_available() {
        use crate::infra::mongo::MongoPool;
        let pool = match MongoPool::connect("mongodb://localhost:27017", "test_query_exec").await {
            Ok(p) => p,
            Err(_) => return,
        };
        if pool.db().run_command(mongodb::bson::doc! { "ping": 1 }).await.is_err() {
            return;
        }

        use crate::config::AppConfig;
        use crate::domain::safety::SafetyChecker;
        use crate::infra::llm::LlmClient;
        use std::sync::Arc;
        use dashmap::DashMap;
        let config = crate::test_helpers::test_config();
        let state = crate::state::AppState {
            mongo: pool,
            llm: LlmClient::new(&config),
            schemas: Arc::new(DashMap::new()),
            safety: SafetyChecker::new(
                vec!["find".into(), "count".into(), "distinct".into(), "aggregate".into()],
                10000,
            ),
            http: reqwest::Client::new(),
            issues: Arc::new(tokio::sync::RwLock::new(vec![])),
            config,
        };

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/execute", web::post().to(handle_execute))
        ).await;

        // test_config uses unsafe_allow_all=false, so validation passes schema=None with query_only=false
        // We use a confirmed find on a collection to exercise the full execution path
        let parsed = ParsedQuery {
            collection: "test_query_exec_col".into(),
            operation: QueryOperation::Find,
            limit: 10,
            ..Default::default()
        };
        let req = test::TestRequest::post()
            .uri("/api/execute")
            .set_json(&serde_json::json!({ "parsed": parsed, "confirm": true }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        // Either 200 (found) or 403 (unknown collection without schema) — either way not 500
        assert!(resp.status().is_success() || resp.status() == 403);
    }
}

/// Pure helper tests — no actix or async runtime needed.
#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn query_result_len_documents() {
        use mongodb::bson::Document;
        let result = executor::QueryResult::Documents(vec![Document::new(), Document::new()]);
        assert_eq!(query_result_len(&result), 2);
    }

    #[test]
    fn query_result_len_count() {
        let result = executor::QueryResult::Count(42);
        assert_eq!(query_result_len(&result), 1);
    }

    #[test]
    fn query_result_len_values() {
        use mongodb::bson::Bson;
        let result = executor::QueryResult::Values(vec![Bson::Int64(1), Bson::Int64(2), Bson::Int64(3)]);
        assert_eq!(query_result_len(&result), 3);
    }

    #[test]
    fn query_result_to_json_count() {
        let result = executor::QueryResult::Count(7);
        let json = query_result_to_json(&result);
        assert_eq!(json["count"], 7);
    }

    #[test]
    fn query_result_to_json_documents_empty() {
        let result = executor::QueryResult::Documents(vec![]);
        let json = query_result_to_json(&result);
        assert!(json.as_array().unwrap().is_empty());
    }

    #[test]
    fn bson_datetime_formats_as_iso_string() {
        use mongodb::bson::Bson;
        let dt = mongodb::bson::DateTime::from_millis(0);
        let json = bson_to_json(&Bson::DateTime(dt));
        assert!(json.as_str().unwrap().contains("1970-01-01"));
    }

    #[test]
    fn bson_objectid_formats_as_hex_string() {
        use mongodb::bson::{Bson, oid::ObjectId};
        let json = bson_to_json(&Bson::ObjectId(ObjectId::new()));
        let s = json.as_str().unwrap();
        assert_eq!(s.len(), 24);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn query_result_to_json_values() {
        use mongodb::bson::Bson;
        let result = executor::QueryResult::Values(vec![Bson::String("a".into()), Bson::Int32(2)]);
        let json = query_result_to_json(&result);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], "a");
    }

    #[test]
    fn query_result_to_json_write() {
        use crate::domain::executor::WriteResultData;
        let result = executor::QueryResult::Write(WriteResultData {
            operation: "insert".into(),
            collection: "movies".into(),
            matched_count: 0,
            modified_count: 0,
            inserted_id: Some("abc123".into()),
        });
        let json = query_result_to_json(&result);
        assert_eq!(json["operation"], "insert");
        assert_eq!(json["collection"], "movies");
        assert_eq!(json["inserted_id"], "abc123");
    }

    #[test]
    fn query_result_len_write() {
        use crate::domain::executor::WriteResultData;
        let result = executor::QueryResult::Write(WriteResultData {
            operation: "insert".into(),
            collection: "movies".into(),
            matched_count: 0,
            modified_count: 0,
            inserted_id: None,
        });
        assert_eq!(query_result_len(&result), 1);
    }

    #[test]
    fn bson_document_nested_converts_to_json_object() {
        use mongodb::bson::{Bson, doc};
        let bson = Bson::Document(doc! { "key": "value" });
        let json = bson_to_json(&bson);
        assert_eq!(json["key"], "value");
    }

    #[test]
    fn bson_array_converts_to_json_array() {
        use mongodb::bson::Bson;
        let bson = Bson::Array(vec![Bson::Int32(1), Bson::Int32(2)]);
        let json = bson_to_json(&bson);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn bson_string_passes_through() {
        use mongodb::bson::Bson;
        let json = bson_to_json(&Bson::String("hello".into()));
        assert_eq!(json, "hello");
    }

    #[test]
    fn bson_bool_passes_through() {
        use mongodb::bson::Bson;
        assert_eq!(bson_to_json(&Bson::Boolean(true)), true);
    }

    #[test]
    fn bson_null_passes_through() {
        use mongodb::bson::Bson;
        assert_eq!(bson_to_json(&Bson::Null), serde_json::Value::Null);
    }
}

/// Convert a single BSON value to JSON, formatting DateTime as "YYYY-MM-DDTHH:MM:SS.SSSZ".
fn bson_to_json(val: &mongodb::bson::Bson) -> serde_json::Value {
    use mongodb::bson::Bson;
    match val {
        Bson::DateTime(dt) => {
            let millis = dt.timestamp_millis();
            let chrono_dt = chrono::DateTime::from_timestamp_millis(millis)
                .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
            serde_json::Value::String(chrono_dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        }
        Bson::Document(d) => bson_doc_to_json(d),
        Bson::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(bson_to_json).collect())
        }
        Bson::ObjectId(oid) => serde_json::Value::String(oid.to_hex()),
        other => serde_json::to_value(other).unwrap_or(serde_json::Value::Null),
    }
}

