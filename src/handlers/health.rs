use actix_web::{web, HttpResponse};
use mongodb::bson::doc;
use serde_json::json;

use crate::state::AppState;

/// Health check — pings MongoDB and LLM, returns status.
pub async fn health_check(state: web::Data<AppState>) -> HttpResponse {
    let mongo_ok = state
        .mongo
        .db()
        .run_command(doc! { "ping": 1 })
        .await
        .is_ok();

    // Check if Ollama is reachable (best-effort, non-blocking)
    let ollama_ok = state
        .http
        .get(format!("{}/api/tags", state.config.ollama_url))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let status = if mongo_ok { "ok" } else { "error" };

    HttpResponse::Ok().json(json!({
        "status": status,
        "mongo": mongo_ok,
        "ollama": ollama_ok,
    }))
}

/// Version endpoint — returns build metadata.
pub async fn version() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git_hash": option_env!("GIT_HASH").unwrap_or("unknown"),
        "built_at": option_env!("BUILT_AT").unwrap_or("unknown"),
    }))
}
