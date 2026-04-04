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

/// List available LLM models from Ollama.
pub async fn list_models(state: web::Data<AppState>) -> HttpResponse {
    match state.llm.list_models().await {
        Ok(models) => {
            let default_model = &state.config.ollama_model;
            let model_list: Vec<serde_json::Value> = models.iter()
                .map(|m| json!({
                    "name": m,
                    "default": m == default_model,
                }))
                .collect();
            HttpResponse::Ok().json(model_list)
        }
        Err(_) => HttpResponse::Ok().json(serde_json::json!([])),
    }
}

/// Version endpoint — returns build metadata.
pub async fn version() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git_hash": option_env!("GIT_HASH").unwrap_or("unknown"),
        "built_at": option_env!("BUILT_AT").unwrap_or("unknown"),
    }))
}

/// GET /api/config — Return current configuration (secrets redacted).
pub async fn get_config(state: web::Data<AppState>) -> HttpResponse {
    let config = &state.config;
    HttpResponse::Ok().json(json!({
        "llm": {
            "provider": config.llm_provider.to_string(),
            "ollama_url": config.ollama_url,
            "ollama_model": config.ollama_model,
            "lmstudio_url": config.lmstudio_url,
            "lmstudio_model": config.lmstudio_model,
            "anthropic_api_url": config.anthropic_api_url,
            "openai_api_url": config.openai_api_url,
            "cloud_llm_model": config.cloud_llm_model,
            "cloud_llm_api_key": config.cloud_llm_api_key.as_ref().map(|_| "***set***"),
        },
        "mongodb": {
            "uri": config.mongo_uri,
            "database": config.mongo_db_name,
        },
        "server": {
            "host": config.host,
            "port": config.port,
            "dashboard_auth": config.dashboard_token.is_some(),
        },
        "safety": {
            "max_result_rows": config.max_result_rows,
            "allowed_operations": config.allowed_operations,
        },
        "slack": {
            "bot_token": config.slack_bot_token.as_ref().map(|_| "***set***"),
            "signing_secret": config.slack_signing_secret.as_ref().map(|_| "***set***"),
        }
    }))
}
