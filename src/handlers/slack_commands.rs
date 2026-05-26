use actix_web::{web, HttpResponse};
use serde::Deserialize;
use serde_json::json;

use crate::domain::executor::{self, QueryResult};
use crate::handlers::query::query_result_len;
use crate::state::AppState;

/// Slack slash command payload (form-encoded).
#[derive(Debug, Deserialize)]
pub struct SlashCommandPayload {
    #[allow(dead_code)]
    pub token: String,
    #[allow(dead_code)]
    pub team_id: String,
    #[allow(dead_code)]
    pub channel_id: String,
    #[allow(dead_code)]
    pub user_id: String,
    #[allow(dead_code)]
    pub command: String,
    pub text: String,
    pub response_url: String,
}

/// Handle Slack slash command: respond immediately, then process in background.
pub async fn handle_slash_command(
    state: web::Data<AppState>,
    form: web::Form<SlashCommandPayload>,
) -> HttpResponse {
    let text = form.text.trim().to_string();

    // If empty text, return usage help
    if text.is_empty() {
        return HttpResponse::Ok().json(json!({
            "response_type": "ephemeral",
            "text": "Usage: `/query <natural language query>`\nExample: `/query find all users created this week`"
        }));
    }

    let response_url = form.response_url.clone();

    // Spawn background task to process the query
    let state_clone = state.into_inner();
    tokio::spawn(async move {
        let result = process_query(&state_clone, &text).await;
        let message = match result {
            Ok(msg) => msg,
            Err(e) => format!(":warning: Error: {e}"),
        };

        // POST result to the Slack response_url
        if let Err(e) = post_to_response_url(&state_clone.http, &response_url, &message).await {
            tracing::error!(error = %e, "Failed to post to Slack response_url");
        }
    });

    // Immediate acknowledgment
    HttpResponse::Ok().json(json!({
        "response_type": "ephemeral",
        "text": ":hourglass_flowing_sand: Processing your query..."
    }))
}

/// Handle Slack Events API (url_verification + event_callback).
pub async fn handle_events(
    state: web::Data<AppState>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    // Handle url_verification challenge
    if body.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        let challenge = body
            .get("challenge")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return HttpResponse::Ok().json(json!({ "challenge": challenge }));
    }

    // Handle event_callback
    if body.get("type").and_then(|v| v.as_str()) == Some("event_callback") {
        if let Some(event) = body.get("event") {
            let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

            if event_type == "app_mention" {
                let text = event
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let channel = event
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Strip the bot mention from the text
                let query_text = strip_bot_mention(text);

                if !query_text.is_empty() && !channel.is_empty() {
                    let state_clone = state.into_inner();
                    let channel = channel.to_string();
                    let query_text = query_text.to_string();
                    tokio::spawn(async move {
                        let result = process_query(&state_clone, &query_text).await;
                        let message = match result {
                            Ok(msg) => msg,
                            Err(e) => format!(":warning: Error: {e}"),
                        };

                        if let Err(e) = post_to_channel(
                            &state_clone.http,
                            &state_clone.config,
                            &channel,
                            &message,
                        )
                        .await
                        {
                            tracing::error!(error = %e, "Failed to post to Slack channel");
                        }
                    });
                }
            }
        }
    }

    HttpResponse::Ok().finish()
}

/// Process a natural-language query through the full pipeline and format as Slack text.
async fn process_query(
    state: &AppState,
    query_text: &str,
) -> Result<String, crate::error::AppError> {
    let schema_context = state.schema_context();
    let (parsed, _raw, _model) = state.llm.parse_query(query_text, &schema_context, None, None).await?;

    let schema = state.get_schema(&parsed.collection);
    state.safety.validate(&parsed, schema.as_ref())?;

    let collection = parsed.collection.clone();
    let operation = parsed.operation.as_str().to_string();
    let result = executor::execute(&state.mongo, &parsed, schema.as_ref()).await?;

    Ok(format_slack_result(&collection, &operation, &result))
}

/// Format a QueryResult as Slack-friendly text with code blocks.
fn format_slack_result(collection: &str, operation: &str, result: &QueryResult) -> String {
    let count = query_result_len(result);

    match result {
        QueryResult::Documents(docs) => {
            let preview: Vec<String> = docs
                .iter()
                .take(5)
                .map(|d| format!("{d}"))
                .collect();
            let preview_text = preview.join("\n");
            let suffix = if count > 5 {
                format!("\n_...and {} more_", count - 5)
            } else {
                String::new()
            };
            format!(
                "*{operation}* on `{collection}` -- {count} result(s):\n```\n{preview_text}\n```{suffix}"
            )
        }
        QueryResult::Count(n) => {
            format!("*{operation}* on `{collection}` -- count: *{n}*")
        }
        QueryResult::Values(vals) => {
            let preview: Vec<String> = vals.iter().take(20).map(|v| format!("{v}")).collect();
            format!(
                "*{operation}* on `{collection}` -- {count} distinct value(s):\n```\n{}\n```",
                preview.join(", ")
            )
        }
        QueryResult::Write(w) => {
            let id_info = w.inserted_id.as_deref().map_or(String::new(), |id| format!("\nInserted ID: `{id}`"));
            format!(
                "*{op}* on `{coll}` -- matched: {matched}, modified: {modified}{id_info}",
                op = w.operation,
                coll = w.collection,
                matched = w.matched_count,
                modified = w.modified_count,
            )
        }
    }
}

/// Post a message to a Slack response_url.
async fn post_to_response_url(
    http: &reqwest::Client,
    response_url: &str,
    text: &str,
) -> Result<(), crate::error::AppError> {
    let payload = json!({
        "response_type": "in_channel",
        "text": text,
    });

    let resp = http
        .post(response_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("Slack POST failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        tracing::error!(status = %status, "Slack response_url returned error");
    }

    Ok(())
}

/// Post a message to a Slack channel via chat.postMessage API.
async fn post_to_channel(
    http: &reqwest::Client,
    config: &crate::config::AppConfig,
    channel: &str,
    text: &str,
) -> Result<(), crate::error::AppError> {
    let token = config
        .slack_bot_token
        .as_deref()
        .ok_or_else(|| crate::error::AppError::Config("SLACK_BOT_TOKEN not configured".into()))?;

    let payload = json!({
        "channel": channel,
        "text": text,
    });

    let resp = http
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("Slack API call failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        tracing::error!(status = %status, "Slack chat.postMessage failed");
    }

    Ok(())
}

/// Strip a Slack bot mention (e.g. "<@U12345>") from the beginning of a message.
fn strip_bot_mention(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix('<') {
        if let Some(idx) = rest.find('>') {
            return rest[idx + 1..].trim();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::executor::{QueryResult, WriteResultData};

    #[test]
    fn test_strip_bot_mention() {
        assert_eq!(strip_bot_mention("<@U123> find users"), "find users");
        assert_eq!(strip_bot_mention("  <@U123>  hello "), "hello");
        assert_eq!(strip_bot_mention("no mention"), "no mention");
    }

    #[test]
    fn strip_bot_mention_no_closing_bracket() {
        // Malformed mention without '>' — returns trimmed original
        assert_eq!(strip_bot_mention("<@U123 find users"), "<@U123 find users");
    }

    // ── format_slack_result ──────────────────────────────────────

    #[test]
    fn format_slack_result_documents_under_limit() {
        let docs = vec![
            mongodb::bson::doc! { "title": "Movie1" },
            mongodb::bson::doc! { "title": "Movie2" },
        ];
        let result = QueryResult::Documents(docs);
        let text = format_slack_result("movies", "find", &result);
        assert!(text.contains("2 result(s)"));
        assert!(text.contains("```"));
        // No "...and N more" suffix when count <= 5
        assert!(!text.contains("more"));
    }

    #[test]
    fn format_slack_result_documents_over_limit_shows_more() {
        let docs: Vec<_> = (0..7).map(|i| mongodb::bson::doc! { "i": i }).collect();
        let result = QueryResult::Documents(docs);
        let text = format_slack_result("logs", "find", &result);
        assert!(text.contains("7 result(s)"));
        assert!(text.contains("2 more"));
    }

    #[test]
    fn format_slack_result_count() {
        let result = QueryResult::Count(42);
        let text = format_slack_result("users", "count", &result);
        assert!(text.contains("count: *42*"));
        assert!(text.contains("users"));
    }

    #[test]
    fn format_slack_result_values() {
        use mongodb::bson::Bson;
        let result = QueryResult::Values(vec![Bson::String("Action".into()), Bson::String("Drama".into())]);
        let text = format_slack_result("movies", "distinct", &result);
        assert!(text.contains("2 distinct value(s)"));
        assert!(text.contains("Action"));
    }

    #[test]
    fn format_slack_result_write_without_inserted_id() {
        let result = QueryResult::Write(WriteResultData {
            operation: "update".into(),
            collection: "users".into(),
            matched_count: 5,
            modified_count: 3,
            inserted_id: None,
        });
        let text = format_slack_result("users", "update", &result);
        assert!(text.contains("matched: 5"));
        assert!(text.contains("modified: 3"));
        assert!(!text.contains("Inserted ID"));
    }

    #[test]
    fn format_slack_result_write_with_inserted_id() {
        let result = QueryResult::Write(WriteResultData {
            operation: "insert".into(),
            collection: "orders".into(),
            matched_count: 0,
            modified_count: 0,
            inserted_id: Some("abc123".into()),
        });
        let text = format_slack_result("orders", "insert", &result);
        assert!(text.contains("Inserted ID"));
        assert!(text.contains("abc123"));
    }

    // ── Handler integration tests ────────────────────────────────

    #[actix_web::test]
    async fn handle_slash_command_empty_text_returns_usage() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/slack/commands", web::post().to(handle_slash_command))
        ).await;
        let form_data = "token=test&team_id=T1&channel_id=C1&user_id=U1&command=%2Fquery&text=&response_url=http%3A%2F%2Flocalhost%2Fcallback";
        let req = test::TestRequest::post()
            .uri("/api/slack/commands")
            .insert_header(("Content-Type", "application/x-www-form-urlencoded"))
            .set_payload(form_data)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["response_type"], "ephemeral");
        assert!(body["text"].as_str().unwrap().contains("Usage"));
    }

    #[actix_web::test]
    async fn handle_events_url_verification_returns_challenge() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/slack/events", web::post().to(handle_events))
        ).await;
        let req = test::TestRequest::post()
            .uri("/api/slack/events")
            .set_json(&serde_json::json!({
                "type": "url_verification",
                "challenge": "my-challenge-token"
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["challenge"], "my-challenge-token");
    }

    #[actix_web::test]
    async fn handle_events_unknown_type_returns_200() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/slack/events", web::post().to(handle_events))
        ).await;
        let req = test::TestRequest::post()
            .uri("/api/slack/events")
            .set_json(&serde_json::json!({ "type": "other_event" }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }
}
