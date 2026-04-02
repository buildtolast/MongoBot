use actix_web::HttpResponse;

/// Handle Slack interactive actions (block_actions, etc.).
/// Stub that acknowledges with 200 OK.
pub async fn handle_action(
    _body: actix_web::web::Json<serde_json::Value>,
) -> HttpResponse {
    // Slack requires a 200 response within 3 seconds to acknowledge the action.
    // More complex action handling can be added here later.
    tracing::debug!("Received Slack action (acknowledged)");
    HttpResponse::Ok().finish()
}
