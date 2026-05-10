use actix_web::{web, HttpResponse};
use chrono::Utc;
use crate::domain::models::{FeedbackRequest, FeedbackType, QueryFeedback};
use crate::state::AppState;
use crate::error::AppError;
use mongodb::bson::doc;
use serde_json::json;
use futures::stream::TryStreamExt;

/// POST /api/feedback — submit explicit feedback (liked, disliked, corrected, abandoned).
pub async fn submit_feedback(
    state: web::Data<AppState>,
    body: web::Json<FeedbackRequest>,
) -> Result<HttpResponse, AppError> {
    // Input length validation
    if body.nl_query.len() > 4000 {
        return Err(AppError::Validation("nl_query exceeds 4000 characters".into()));
    }
    if body.note.as_deref().map(|n| n.len() > 2000).unwrap_or(false) {
        return Err(AppError::Validation("note exceeds 2000 characters".into()));
    }
    if body.raw_llm_response.as_deref().map(|s| s.len() > 50_000).unwrap_or(false) {
        return Err(AppError::Validation("raw_llm_response exceeds 50000 characters".into()));
    }
    if body.mongo_query.len() > 10_000 {
        return Err(AppError::Validation("mongo_query exceeds 10000 characters".into()));
    }

    // Construct the schema snapshot string: "collection(field1:type, field2:type, ...)"
    let collection_name = &body.parsed_query.collection;
    let schema_snapshot = if let Some(schema) = state.get_schema(collection_name) {
        let fields: Vec<String> = schema.fields.iter()
            .map(|f| format!("{}:{}", f.name, f.field_type))
            .collect();
        format!("{}({})", collection_name, fields.join(","))
    } else {
        collection_name.clone()
    };

    let mut feedback = QueryFeedback {
        id: mongodb::bson::oid::ObjectId::new(),
        timestamp: Utc::now(),
        nl_query: body.nl_query.clone(),
        provider: body.provider.clone(),
        model: body.model.clone(),
        raw_llm_response: body.raw_llm_response.clone(),
        parsed_query: body.parsed_query.clone(),
        user_edited_query: body.user_edited_query.clone(),
        mongo_query: body.mongo_query.clone(),
        feedback: body.feedback.clone(),
        note: body.note.clone(),
        executed: body.executed,
        result_count: body.result_count,
        schema_snapshot,
        embedding: None,
    };

    // Generate embedding for few-shot retrieval in Phase 2
    if let Ok(emb) = state.llm.embed_text(&feedback.nl_query).await {
        feedback.embedding = Some(emb);
    } else {
        tracing::warn!("Failed to generate embedding for feedback: {}", feedback.nl_query);
    }

    // Convert to BSON for MongoDB
    let bson_doc = mongodb::bson::to_document(&feedback)
        .map_err(|e| AppError::Internal(format!("Failed to serialize feedback: {}", e)))?;

    state.mongo.insert_one("query_feedback", bson_doc).await
        .map_err(|e| AppError::Internal(format!("Failed to insert feedback: {}", e)))?;

    Ok(HttpResponse::Ok().json(json!({ "id": feedback.id.to_hex() })))
}

/// GET /api/feedback/stats — returns summary statistics of feedback.
pub async fn get_feedback_stats(
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let collection = "query_feedback";

    let mut by_provider = std::collections::HashMap::new();
    let mut by_model = std::collections::HashMap::new();
    let mut stats_map = std::collections::HashMap::new();
    let mut total = 0;

    let mut cursor = state.mongo.find(collection, Some(doc! {}), None, None, None).await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    while let Some(doc) = cursor.try_next().await
        .map_err(|e| AppError::Internal(format!("MongoDB cursor error: {e}")))? {
        let fb: QueryFeedback = mongodb::bson::from_document(doc)
            .map_err(|e| AppError::Internal(format!("Failed to deserialize: {}", e)))?;
        total += 1;
        *by_provider.entry(fb.provider.clone()).or_insert(0) += 1;
        *by_model.entry(fb.model.clone()).or_insert(0) += 1;
        *stats_map.entry(fb.feedback.to_string()).or_insert(0) += 1;
    }

    Ok(HttpResponse::Ok().json(json!({
        "total": total,
        "by_type": stats_map,
        "by_provider": by_provider,
        "by_model": by_model
    })))
}

/// GET /api/feedback/export — exports feedback in various formats.
pub async fn export_feedback(
    state: web::Data<AppState>,
    query: web::Query<serde_json::Value>,
) -> Result<HttpResponse, AppError> {
    let format = query.get("format").and_then(|v| v.as_str()).unwrap_or("json");
    let feedback_type = query.get("type").and_then(|v| v.as_str());

    let collection = "query_feedback";

    // Build filter based on type if provided; validate against known enum variants
    let mut filter = doc! {};
    if let Some(t) = feedback_type {
        let _validated: FeedbackType = serde_json::from_value(serde_json::json!(t))
            .map_err(|_| AppError::Validation(format!("Invalid feedback type: {t}")))?;
        filter.insert("feedback", t);
    }

    let mut cursor = state.mongo.find(collection, Some(filter), None, None, None).await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut feedbacks = Vec::new();
    while let Some(doc) = cursor.try_next().await
        .map_err(|e| AppError::Internal(format!("MongoDB cursor error: {e}")))? {
        let feedback: QueryFeedback = mongodb::bson::from_document(doc)
            .map_err(|e| AppError::Internal(format!("Failed to deserialize feedback: {}", e)))?;
        feedbacks.push(feedback);
    }

    match format {
        "jsonl" => {
            let content = format_feedback_as_openai_jsonl(&feedbacks)
                .map_err(|e| AppError::Internal(format!("Failed to serialize feedback record: {e}")))?;
            Ok(HttpResponse::Ok()
                .content_type("application/x-jsonlines")
                .body(content))
        }
        "json" => {
            Ok(HttpResponse::Ok().json(feedbacks))
        }
        "dpo-jsonl" => {
            // Only Corrected entries where user_edited_query is present (required for DPO chosen label)
            let mut content = String::new();
            for fb in feedbacks.into_iter()
                .filter(|f| f.feedback == FeedbackType::Corrected && f.user_edited_query.is_some())
            {
                let chosen = fb.user_edited_query.expect("filtered above");
                let entry = json!({
                    "prompt": fb.nl_query,
                    "chosen": chosen,
                    "rejected": fb.parsed_query
                });
                let line = serde_json::to_string(&entry)
                    .map_err(|e| AppError::Internal(format!("Failed to serialize feedback record: {e}")))?;
                content.push_str(&line);
                content.push('\n');
            }
            Ok(HttpResponse::Ok()
                .content_type("application/x-jsonlines")
                .body(content))
        }
        _ => Err(AppError::Validation("Invalid format".into())),
    }
}

/// Format a slice of feedback records as OpenAI fine-tune JSONL.
///
/// Each line is: `{"messages":[{"role":"user","content":"<nl_query>"},{"role":"assistant","content":"<parsed_query_json>"}]}`
pub(crate) fn format_feedback_as_openai_jsonl(records: &[QueryFeedback]) -> Result<String, serde_json::Error> {
    let mut content = String::new();
    for fb in records {
        let entry = json!({
            "messages": [
                { "role": "user", "content": fb.nl_query },
                { "role": "assistant", "content": serde_json::to_string(&fb.parsed_query).unwrap_or_default() }
            ]
        });
        let line = serde_json::to_string(&entry)?;
        content.push_str(&line);
        content.push('\n');
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::domain::models::{FeedbackType, ParsedQuery, QueryFeedback};

    fn make_feedback(nl_query: &str) -> QueryFeedback {
        QueryFeedback {
            id: mongodb::bson::oid::ObjectId::new(),
            timestamp: Utc::now(),
            nl_query: nl_query.to_string(),
            provider: "ollama".to_string(),
            model: "test-model".to_string(),
            raw_llm_response: None,
            parsed_query: ParsedQuery::default(),
            user_edited_query: None,
            mongo_query: "db.test.find({})".to_string(),
            feedback: FeedbackType::Liked,
            note: None,
            executed: true,
            result_count: 0,
            schema_snapshot: "test".to_string(),
            embedding: None,
        }
    }

    #[test]
    fn test_export_jsonl_format() {
        // Empty input → empty string
        assert_eq!(format_feedback_as_openai_jsonl(&[]).expect("serialization must succeed"), "");

        // One record → exactly one line, valid JSON, correct roles, assistant content is parsed_query JSON
        let record = make_feedback("find all movies");
        let output = format_feedback_as_openai_jsonl(&[record.clone()]).expect("serialization must succeed");

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1, "expected exactly one line");

        let parsed: serde_json::Value = serde_json::from_str(lines[0]).expect("line must be valid JSON");
        let messages = parsed["messages"].as_array().expect("messages must be an array");
        assert_eq!(messages.len(), 2);

        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "find all movies");

        assert_eq!(messages[1]["role"], "assistant");
        let assistant_content = messages[1]["content"].as_str().expect("assistant content must be a string");
        let assistant_json: serde_json::Value = serde_json::from_str(assistant_content)
            .expect("assistant content must be valid JSON");
        let expected_pq = serde_json::to_value(&record.parsed_query).unwrap();
        assert_eq!(assistant_json, expected_pq);
    }

    #[test]
    fn test_diff_parsed_queries() {
        // Two ParsedQuery values that differ in a filter field
        let mut pq_a = ParsedQuery::default();
        pq_a.collection = "movies".to_string();
        pq_a.limit = 10;

        let mut pq_b = pq_a.clone();
        pq_b.limit = 99; // differs only in limit

        assert!(pq_a != pq_b, "queries with different limits must not be equal");

        // Two ParsedQuery values that are equal
        let pq_c = pq_a.clone();
        assert!(pq_a == pq_c, "cloned queries must be equal");

        // This mirrors the exact comparison used in handle_execute:
        //   if original != parsed { /* insert Corrected feedback */ }
        let original = pq_a.clone();
        let user_edited = pq_b.clone();
        assert!(original != user_edited, "correction detection must fire when queries differ");

        let no_edit = pq_a.clone();
        assert!(!(original != no_edit), "correction detection must not fire when queries are equal");
    }
}
