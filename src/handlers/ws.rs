use actix_web::{web, HttpRequest, HttpResponse};
use actix_ws::Message;
use futures::StreamExt;
use serde_json::json;

use crate::state::AppState;

/// WebSocket upgrade handler — maintains connection and handles ping/pong.
pub async fn ws_handler(
    req: HttpRequest,
    stream: web::Payload,
    _state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, mut session, mut msg_stream) = actix_ws::handle(&req, stream)?;

    // Send initial connected message
    let connected_msg = json!({
        "type": "connected",
        "message": "WebSocket connection established"
    });
    if let Err(e) = session.text(connected_msg.to_string()).await {
        tracing::warn!(error = %e, "Failed to send initial WS message");
        return Ok(response);
    }

    // Spawn a task to handle incoming messages
    let mut session_clone = session.clone();
    actix_rt::spawn(async move {
        while let Some(Ok(msg)) = msg_stream.next().await {
            match msg {
                Message::Ping(bytes) => {
                    if session_clone.pong(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Pong(_) => {
                    // Client responded to our ping — connection is alive
                }
                Message::Text(text) => {
                    // Echo back or handle commands
                    let response = json!({
                        "type": "ack",
                        "received": text.to_string()
                    });
                    if session_clone.text(response.to_string()).await.is_err() {
                        break;
                    }
                }
                Message::Close(reason) => {
                    tracing::debug!(?reason, "WebSocket client disconnected");
                    let _ = session_clone.close(reason).await;
                    break;
                }
                _ => {}
            }
        }

        tracing::debug!("WebSocket message loop ended");
    });

    // Spawn a heartbeat ping task
    actix_rt::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if session.ping(b"").await.is_err() {
                break;
            }
        }
    });

    Ok(response)
}
