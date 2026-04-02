use dashmap::DashMap;
use std::sync::Arc;

use crate::config::AppConfig;
use crate::domain::models::CollectionSchema;
use crate::domain::safety::SafetyChecker;
use crate::infra::llm::LlmClient;
use crate::infra::mongo::MongoPool;

/// Shared application state, accessible from all request handlers.
pub struct AppState {
    /// MongoDB connection pool.
    pub mongo: MongoPool,

    /// LLM client for natural language query parsing.
    pub llm: LlmClient,

    /// Collection schemas loaded from YAML files, keyed by collection name.
    pub schemas: Arc<DashMap<String, CollectionSchema>>,

    /// Safety checker for query validation.
    pub safety: SafetyChecker,

    /// Shared HTTP client for outbound requests.
    pub http: reqwest::Client,

    /// Application configuration.
    pub config: AppConfig,
}

impl AppState {
    /// Create a new AppState by connecting to MongoDB, initializing the LLM client,
    /// loading schemas, and creating the safety checker.
    pub async fn new(config: &AppConfig) -> anyhow::Result<Self> {
        // Connect to MongoDB
        let mongo = MongoPool::connect(&config.mongo_uri, &config.mongo_db_name).await?;
        tracing::info!("MongoDB connected");

        // Create LLM client
        let llm = LlmClient::new(config);
        tracing::info!(provider = %config.llm_provider, "LLM client initialized");

        // Load schemas from schemas/ directory
        let schemas = Arc::new(DashMap::new());
        load_schemas(&schemas).await;

        // Create safety checker
        let safety = SafetyChecker::new(
            config.allowed_operations.clone(),
            config.max_result_rows,
        );

        // Shared HTTP client
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            mongo,
            llm,
            schemas,
            safety,
            http,
            config: config.clone(),
        })
    }

    /// Format all loaded schemas as text context for LLM prompts.
    pub fn schema_context(&self) -> String {
        let mut ctx = String::new();

        for entry in self.schemas.iter() {
            let schema = entry.value();
            ctx.push_str(&format!("### Collection: {}\n", schema.name));
            if !schema.description.is_empty() {
                ctx.push_str(&format!("Description: {}\n", schema.description));
            }
            ctx.push_str("Fields:\n");
            for field in &schema.fields {
                ctx.push_str(&format!("  - {} ({})", field.name, field.field_type));
                if let Some(ref desc) = field.description {
                    ctx.push_str(&format!(" — {desc}"));
                }
                ctx.push('\n');
            }
            ctx.push('\n');
        }

        if ctx.is_empty() {
            ctx.push_str("No schemas loaded. The LLM will need to infer collection structure.\n");
        }

        ctx
    }

    /// Get a schema by collection name.
    pub fn get_schema(&self, name: &str) -> Option<CollectionSchema> {
        self.schemas.get(name).map(|entry| entry.value().clone())
    }
}

/// Load collection schema YAML files from the `schemas/` directory.
async fn load_schemas(schemas: &DashMap<String, CollectionSchema>) {
    let schemas_dir = std::path::Path::new("schemas");

    if !schemas_dir.exists() {
        tracing::info!("No schemas/ directory found; skipping schema loading");
        return;
    }

    let mut entries = match tokio::fs::read_dir(schemas_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read schemas/ directory");
            return;
        }
    };

    let mut count = 0u32;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();

        // Only process .yaml and .yml files
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext != "yaml" && ext != "yml" {
            continue;
        }

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => match serde_yaml_ng::from_str::<CollectionSchema>(&content) {
                Ok(schema) => {
                    tracing::info!(
                        collection = %schema.name,
                        fields = schema.fields.len(),
                        "Loaded schema"
                    );
                    schemas.insert(schema.name.clone(), schema);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "Failed to parse schema YAML"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to read schema file"
                );
            }
        }
    }

    tracing::info!(count, "Schema loading complete");
}
