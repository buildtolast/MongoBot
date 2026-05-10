use actix_web::{web, HttpResponse};
use mongodb::bson::doc;
use serde_json::json;

use crate::config::{AppConfig, LlmProvider};
use crate::state::AppState;

/// Query string for `/api/models?provider=<name>`.
#[derive(serde::Deserialize)]
pub struct ModelsQuery {
    pub provider: Option<String>,
}

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

/// List available LLM models. Accepts optional `?provider=<name>` to filter by provider.
pub async fn list_models(
    state: web::Data<AppState>,
    query: web::Query<ModelsQuery>,
) -> HttpResponse {
    let provider = match query.provider.as_deref() {
        Some(p) => match crate::infra::llm::parse_provider_str(p) {
            Ok(p) => p,
            Err(_) => return HttpResponse::BadRequest().json(json!({"error": format!("Unknown provider: {p}")})),
        },
        None => state.config.llm_provider,
    };

    let default_model = default_model_for_provider(&state.config, provider);

    match state.llm.list_models_for_provider(provider).await {
        Ok(models) => {
            let model_list: Vec<serde_json::Value> = models.iter()
                .map(|m| json!({
                    "name": m,
                    "default": *m == default_model,
                }))
                .collect();
            HttpResponse::Ok().json(model_list)
        }
        Err(_) => HttpResponse::Ok().json(serde_json::json!([])),
    }
}

/// List available LLM providers with the configured default marked.
pub async fn list_providers(state: web::Data<AppState>) -> HttpResponse {
    let default = state.config.llm_provider.to_string();
    let providers = ["ollama", "lmstudio", "llamacpp", "anthropic", "openai"];
    let list: Vec<serde_json::Value> = providers.iter()
        .map(|p| json!({ "name": p, "default": *p == default }))
        .collect();
    HttpResponse::Ok().json(list)
}

fn default_model_for_provider(config: &AppConfig, provider: LlmProvider) -> String {
    match provider {
        LlmProvider::Ollama => config.ollama_model.clone(),
        LlmProvider::LmStudio => config.lmstudio_model.clone().unwrap_or_default(),
        LlmProvider::LlamaCpp => config.llamacpp_model.clone().unwrap_or_default(),
        LlmProvider::Anthropic | LlmProvider::OpenAI => config.cloud_llm_model.clone().unwrap_or_default(),
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
            "llamacpp_url": config.llamacpp_url,
            "llamacpp_model": config.llamacpp_model,
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
        },
        "feedback": {
            "enable_retrieval": config.enable_feedback_retrieval,
            "top_k": config.feedback_top_k,
            "embedding_model": config.embedding_model,
            "embedding_url": config.embedding_url,
        }
    }))
}
