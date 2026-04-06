use clap::{Parser, ValueEnum};
use std::fmt;

/// LLM provider selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Ollama,
    Anthropic,
    OpenAI,
    #[value(alias = "lmstudio")]
    LmStudio,
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::Ollama
    }
}

impl fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ollama => write!(f, "ollama"),
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAI => write!(f, "openai"),
            Self::LmStudio => write!(f, "lmstudio"),
        }
    }
}

/// Application configuration — CLI args with env var fallback.
#[derive(Parser, Clone)]
#[command(name = "slack-mongo-bot", about = "NL-to-MongoDB query bot")]
pub struct AppConfig {
    /// Slack bot OAuth token
    #[arg(long, env = "SLACK_BOT_TOKEN")]
    pub slack_bot_token: Option<String>,

    /// Slack signing secret for request verification
    #[arg(long, env = "SLACK_SIGNING_SECRET")]
    pub slack_signing_secret: Option<String>,

    /// MongoDB connection URI
    #[arg(long, env = "MONGO_URI", default_value = "mongodb://localhost:27017")]
    pub mongo_uri: String,

    /// MongoDB database name
    #[arg(long, env = "MONGO_DB_NAME")]
    pub mongo_db_name: String,

    /// LLM provider to use
    #[arg(long, env = "LLM_PROVIDER", value_enum, default_value = "ollama")]
    pub llm_provider: LlmProvider,

    /// Ollama server URL
    #[arg(long, env = "OLLAMA_URL", default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Ollama model name
    #[arg(long, env = "OLLAMA_MODEL", default_value = "qwen3-coder:30b")]
    pub ollama_model: String,

    /// LM Studio server URL (OpenAI-compatible API)
    #[arg(long, env = "LMSTUDIO_URL", default_value = "http://localhost:1234")]
    pub lmstudio_url: String,

    /// LM Studio model name
    #[arg(long, env = "LMSTUDIO_MODEL")]
    pub lmstudio_model: Option<String>,

    /// API key for cloud LLM providers (Anthropic/OpenAI)
    #[arg(long, env = "CLOUD_LLM_API_KEY")]
    pub cloud_llm_api_key: Option<String>,

    /// Model name for cloud LLM providers
    #[arg(long, env = "CLOUD_LLM_MODEL")]
    pub cloud_llm_model: Option<String>,

    /// Anthropic API URL (override for proxies)
    #[arg(long, env = "ANTHROPIC_API_URL", default_value = "https://api.anthropic.com")]
    pub anthropic_api_url: String,

    /// OpenAI API URL (override for proxies or Azure)
    #[arg(long, env = "OPENAI_API_URL", default_value = "https://api.openai.com")]
    pub openai_api_url: String,

    /// Maximum result rows returned per query
    #[arg(long, env = "MAX_RESULT_ROWS", default_value = "10000")]
    pub max_result_rows: usize,

    /// Comma-separated list of allowed MongoDB operations
    #[arg(long, env = "ALLOWED_OPERATIONS", value_delimiter = ',', default_values_t = default_allowed_ops())]
    pub allowed_operations: Vec<String>,

    /// Server bind host
    #[arg(long, env = "HOST", default_value = "0.0.0.0")]
    pub host: String,

    /// Server bind port
    #[arg(long, env = "PORT", default_value = "3000")]
    pub port: u16,

    /// Dashboard auth token (optional)
    #[arg(long, env = "DASHBOARD_TOKEN")]
    pub dashboard_token: Option<String>,

    /// Allow all pipeline stages and operations, including destructive ones
    /// ($out, $merge, $function, etc.). USE WITH CAUTION — take backups first.
    #[arg(long, env = "UNSAFE_ALLOW_ALL_OPERATIONS", default_value = "false")]
    pub unsafe_allow_all_operations: bool,

    /// Query-only mode: parse and display queries without executing them.
    /// All queries behave as if preview=true. No data is read from or written to MongoDB.
    #[arg(long, env = "QUERY_ONLY", default_value = "false")]
    pub query_only: bool,
}

fn default_allowed_ops() -> Vec<String> {
    vec![
        "find".to_string(),
        "count".to_string(),
        "distinct".to_string(),
        "aggregate".to_string(),
        "insert".to_string(),
        "update".to_string(),
    ]
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("slack_bot_token", &redact_option(&self.slack_bot_token))
            .field("slack_signing_secret", &redact_option(&self.slack_signing_secret))
            .field("mongo_uri", &self.mongo_uri)
            .field("mongo_db_name", &self.mongo_db_name)
            .field("llm_provider", &self.llm_provider)
            .field("ollama_url", &self.ollama_url)
            .field("ollama_model", &self.ollama_model)
            .field("lmstudio_url", &self.lmstudio_url)
            .field("lmstudio_model", &self.lmstudio_model)
            .field("cloud_llm_api_key", &redact_option(&self.cloud_llm_api_key))
            .field("cloud_llm_model", &self.cloud_llm_model)
            .field("anthropic_api_url", &self.anthropic_api_url)
            .field("openai_api_url", &self.openai_api_url)
            .field("max_result_rows", &self.max_result_rows)
            .field("allowed_operations", &self.allowed_operations)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("dashboard_token", &redact_option(&self.dashboard_token))
            .field("unsafe_allow_all_operations", &self.unsafe_allow_all_operations)
            .field("query_only", &self.query_only)
            .finish()
    }
}

fn redact_option(val: &Option<String>) -> &str {
    match val {
        Some(_) => "[REDACTED]",
        None => "[NOT SET]",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_redacts_secrets() {
        let config = AppConfig {
            slack_bot_token: Some("xoxb-secret".into()),
            slack_signing_secret: Some("signing-secret".into()),
            mongo_uri: "mongodb://localhost:27017".into(),
            mongo_db_name: "testdb".into(),
            llm_provider: LlmProvider::Ollama,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "qwen3-coder:30b".into(),
            lmstudio_url: "http://localhost:1234".into(),
            lmstudio_model: None,
            cloud_llm_api_key: Some("sk-secret".into()),
            cloud_llm_model: None,
            anthropic_api_url: "https://api.anthropic.com".into(),
            openai_api_url: "https://api.openai.com".into(),
            max_result_rows: 10000,
            allowed_operations: vec!["find".into()],
            host: "0.0.0.0".into(),
            port: 3000,
            dashboard_token: Some("token123".into()),
            unsafe_allow_all_operations: false,
            query_only: false,
        };
        let debug_output = format!("{:?}", config);
        assert!(!debug_output.contains("xoxb-secret"));
        assert!(!debug_output.contains("signing-secret"));
        assert!(!debug_output.contains("sk-secret"));
        assert!(!debug_output.contains("token123"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn test_default_llm_provider() {
        assert_eq!(LlmProvider::default(), LlmProvider::Ollama);
    }
}
