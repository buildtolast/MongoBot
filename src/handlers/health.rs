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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AppConfig {
        AppConfig {
            slack_bot_token: None,
            slack_signing_secret: None,
            mongo_uri: "mongodb://localhost:27017".into(),
            mongo_db_name: "test".into(),
            llm_provider: LlmProvider::Ollama,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "my-ollama-model".into(),
            lmstudio_url: "http://localhost:1234".into(),
            lmstudio_model: Some("my-lmstudio-model".into()),
            llamacpp_url: "http://localhost:8080".into(),
            llamacpp_model: Some("my-llamacpp-model".into()),
            cloud_llm_api_key: None,
            cloud_llm_model: Some("my-cloud-model".into()),
            anthropic_api_url: "https://api.anthropic.com".into(),
            openai_api_url: "https://api.openai.com".into(),
            max_result_rows: 10000,
            allowed_operations: vec!["find".into()],
            host: "0.0.0.0".into(),
            port: 3000,
            dashboard_token: None,
            unsafe_allow_all_operations: false,
            query_only: false,
            embedding_model: "nomic-embed-text".into(),
            embedding_url: None,
            feedback_top_k: 5,
            enable_feedback_retrieval: true,
            auto_create_indexes: false,
        }
    }

    #[test]
    fn default_model_ollama() {
        let config = test_config();
        assert_eq!(default_model_for_provider(&config, LlmProvider::Ollama), "my-ollama-model");
    }

    #[test]
    fn default_model_lmstudio() {
        let config = test_config();
        assert_eq!(default_model_for_provider(&config, LlmProvider::LmStudio), "my-lmstudio-model");
    }

    #[test]
    fn default_model_llamacpp() {
        let config = test_config();
        assert_eq!(default_model_for_provider(&config, LlmProvider::LlamaCpp), "my-llamacpp-model");
    }

    #[test]
    fn default_model_anthropic() {
        let config = test_config();
        assert_eq!(default_model_for_provider(&config, LlmProvider::Anthropic), "my-cloud-model");
    }

    #[test]
    fn default_model_openai() {
        let config = test_config();
        assert_eq!(default_model_for_provider(&config, LlmProvider::OpenAI), "my-cloud-model");
    }

    #[test]
    fn default_model_lmstudio_none_returns_empty() {
        let mut config = test_config();
        config.lmstudio_model = None;
        assert_eq!(default_model_for_provider(&config, LlmProvider::LmStudio), "");
    }

    #[test]
    fn default_model_cloud_none_returns_empty() {
        let mut config = test_config();
        config.cloud_llm_model = None;
        assert_eq!(default_model_for_provider(&config, LlmProvider::Anthropic), "");
        assert_eq!(default_model_for_provider(&config, LlmProvider::OpenAI), "");
    }

    // ── actix-web handler tests ──────────────────────────────────

    #[actix_web::test]
    async fn version_returns_json_with_expected_keys() {
        use actix_web::{test, web, App};
        let app = test::init_service(
            App::new().route("/api/version", web::get().to(version))
        ).await;
        let req = test::TestRequest::get().uri("/api/version").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert!(body.get("version").is_some());
        assert!(body.get("git_hash").is_some());
        assert!(body.get("built_at").is_some());
    }

    #[actix_web::test]
    async fn list_providers_returns_all_five() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/providers", web::get().to(list_providers))
        ).await;
        let req = test::TestRequest::get().uri("/api/providers").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        let providers = body.as_array().unwrap();
        assert_eq!(providers.len(), 5);
        let names: Vec<&str> = providers.iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"ollama"));
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"openai"));
    }

    #[actix_web::test]
    async fn list_providers_marks_configured_provider_as_default() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        // test_config uses Ollama
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/providers", web::get().to(list_providers))
        ).await;
        let req = test::TestRequest::get().uri("/api/providers").to_request();
        let resp = test::call_service(&app, req).await;
        let body: serde_json::Value = test::read_body_json(resp).await;
        let providers = body.as_array().unwrap();
        let ollama = providers.iter().find(|p| p["name"] == "ollama").unwrap();
        assert_eq!(ollama["default"], true);
        let non_default_count = providers.iter().filter(|p| p["default"] == true).count();
        assert_eq!(non_default_count, 1);
    }

    #[actix_web::test]
    async fn get_config_returns_grouped_sections() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/config", web::get().to(get_config))
        ).await;
        let req = test::TestRequest::get().uri("/api/config").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        for section in &["llm", "mongodb", "server", "safety", "slack"] {
            assert!(body.get(section).is_some(), "missing section: {section}");
        }
    }

    #[actix_web::test]
    async fn list_models_unknown_provider_returns_400() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/models", web::get().to(list_models))
        ).await;
        let req = test::TestRequest::get().uri("/api/models?provider=invalid_provider").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 400);
    }

    #[actix_web::test]
    async fn list_models_returns_empty_when_provider_unreachable() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/models", web::get().to(list_models))
        ).await;
        // Will fail to reach Ollama but should return empty array, not 500
        let req = test::TestRequest::get().uri("/api/models?provider=ollama").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert!(body.is_array());
    }

    #[actix_web::test]
    async fn get_config_redacts_secrets() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_config;
        let mut config = test_config();
        config.cloud_llm_api_key = Some("super-secret-key".into());
        config.slack_bot_token = Some("xoxb-secret-token".into());
        use std::sync::Arc;
        use dashmap::DashMap;
        use crate::domain::safety::SafetyChecker;
        use crate::infra::llm::LlmClient;
        use crate::infra::mongo::MongoPool;
        use crate::state::AppState;
        let allowed_ops = config.allowed_operations.clone();
        let max_rows = config.max_result_rows;
        let llm = LlmClient::new(&config);
        let state = AppState {
            mongo: MongoPool::new_for_tests().await,
            llm,
            schemas: Arc::new(DashMap::new()),
            safety: SafetyChecker::new(allowed_ops, max_rows),
            http: reqwest::Client::new(),
            issues: Arc::new(tokio::sync::RwLock::new(vec![])),
            config,
        };
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/config", web::get().to(get_config))
        ).await;
        let req = test::TestRequest::get().uri("/api/config").to_request();
        let resp = test::call_service(&app, req).await;
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["llm"]["cloud_llm_api_key"], "***set***");
        assert_eq!(body["slack"]["bot_token"], "***set***");
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
