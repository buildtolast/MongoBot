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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, web, App};

    #[actix_web::test]
    async fn handle_action_returns_200() {
        let app = test::init_service(
            App::new().route("/api/slack/actions", web::post().to(handle_action))
        ).await;
        let req = test::TestRequest::post()
            .uri("/api/slack/actions")
            .set_json(&serde_json::json!({ "type": "block_actions", "actions": [] }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }
}
