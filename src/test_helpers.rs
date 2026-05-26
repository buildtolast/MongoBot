use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::RwLock;

use crate::config::{AppConfig, LlmProvider};
use crate::domain::models::CollectionSchema;
use crate::domain::safety::SafetyChecker;
use crate::infra::llm::LlmClient;
use crate::infra::mongo::MongoPool;
use crate::state::AppState;

/// Minimal AppConfig for tests — no real services configured.
pub fn test_config() -> AppConfig {
    AppConfig {
        slack_bot_token: None,
        slack_signing_secret: None,
        mongo_uri: "mongodb://localhost:27017".into(),
        mongo_db_name: "test".into(),
        llm_provider: LlmProvider::Ollama,
        ollama_url: "http://localhost:11434".into(),
        ollama_model: "test-model".into(),
        lmstudio_url: "http://localhost:1234".into(),
        lmstudio_model: None,
        llamacpp_url: "http://localhost:8080".into(),
        llamacpp_model: None,
        cloud_llm_api_key: None,
        cloud_llm_model: None,
        anthropic_api_url: "https://api.anthropic.com".into(),
        openai_api_url: "https://api.openai.com".into(),
        max_result_rows: 10000,
        allowed_operations: vec![
            "find".into(), "count".into(), "aggregate".into(), "distinct".into(),
        ],
        host: "0.0.0.0".into(),
        port: 3000,
        dashboard_token: None,
        unsafe_allow_all_operations: false,
        query_only: false,
        embedding_model: "nomic-embed-text".into(),
        embedding_url: None,
        feedback_top_k: 5,
        enable_feedback_retrieval: false,
        auto_create_indexes: false,
    }
}

/// Build a test AppState with no schemas and no issues.
/// The MongoPool is created without a real connection — do not call MongoDB methods.
pub async fn test_app_state() -> AppState {
    test_app_state_with_schemas(vec![]).await
}

/// Build a test AppState pre-populated with the given schemas.
pub async fn test_app_state_with_schemas(schemas: Vec<CollectionSchema>) -> AppState {
    let map: DashMap<String, CollectionSchema> = DashMap::new();
    for s in schemas {
        map.insert(s.name.clone(), s);
    }
    let config = test_config();
    let allowed_ops = config.allowed_operations.clone();
    let max_rows = config.max_result_rows;
    let llm = LlmClient::new(&config);
    AppState {
        mongo: MongoPool::new_for_tests().await,
        llm,
        schemas: Arc::new(map),
        safety: SafetyChecker::new(allowed_ops, max_rows),
        http: reqwest::Client::new(),
        issues: Arc::new(RwLock::new(vec![])),
        config,
    }
}
