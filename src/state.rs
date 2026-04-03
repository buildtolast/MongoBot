use dashmap::DashMap;
use std::sync::Arc;

use crate::config::AppConfig;
use crate::domain::models::CollectionSchema;
use crate::domain::safety::SafetyChecker;
use crate::infra::llm::LlmClient;
use crate::infra::mongo::MongoPool;

/// Shared application state, accessible from all request handlers.
pub struct AppState {
    pub mongo: MongoPool,
    pub llm: LlmClient,
    pub schemas: Arc<DashMap<String, CollectionSchema>>,
    pub safety: SafetyChecker,
    pub http: reqwest::Client,
    pub config: AppConfig,
}

impl AppState {
    pub async fn new(config: &AppConfig) -> anyhow::Result<Self> {
        let mongo = MongoPool::connect(&config.mongo_uri, &config.mongo_db_name).await?;
        tracing::info!("MongoDB connected");

        let llm = LlmClient::new(config);
        tracing::info!(provider = %config.llm_provider, "LLM client initialized");

        // Discover schemas at runtime from MongoDB, then merge with YAML overrides
        let schemas = Arc::new(DashMap::new());
        discover_schemas_from_db(&mongo, &schemas).await;
        merge_yaml_schemas(&schemas).await;

        let safety = SafetyChecker::new(
            config.allowed_operations.clone(),
            config.max_result_rows,
        );

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

    /// Format all schemas as text for LLM prompts, including index information.
    pub fn schema_context(&self) -> String {
        let mut ctx = String::new();

        for entry in self.schemas.iter() {
            let schema = entry.value();
            ctx.push_str(&format!("### Collection: {} ({})\n", schema.name, schema.source));
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

            // Include index info so LLM can generate index-friendly queries
            if !schema.indexes.is_empty() {
                ctx.push_str("Indexes (prefer filtering/sorting on indexed fields):\n");
                for idx in &schema.indexes {
                    let keys: Vec<String> = idx.keys.iter()
                        .map(|k| format!("{}:{}", k.field, k.direction))
                        .collect();
                    ctx.push_str(&format!("  - {} [{}]{}\n",
                        idx.name,
                        keys.join(", "),
                        if idx.unique { " UNIQUE" } else { "" }
                    ));
                }
            }
            ctx.push('\n');
        }

        if ctx.is_empty() {
            ctx.push_str("No schemas available.\n");
        }

        ctx
    }

    pub fn get_schema(&self, name: &str) -> Option<CollectionSchema> {
        self.schemas.get(name).map(|entry| entry.value().clone())
    }
}

/// Discover schemas by introspecting all collections in the MongoDB database.
async fn discover_schemas_from_db(mongo: &MongoPool, schemas: &DashMap<String, CollectionSchema>) {
    match mongo.list_collections().await {
        Ok(names) => {
            tracing::info!(count = names.len(), "Discovered collections from MongoDB");
            for name in names {
                match mongo.introspect_collection(&name).await {
                    Ok(schema) => {
                        tracing::info!(
                            collection = %schema.name,
                            fields = schema.fields.len(),
                            indexes = schema.indexes.len(),
                            "Introspected collection"
                        );
                        schemas.insert(name, schema);
                    }
                    Err(e) => {
                        tracing::warn!(collection = %name, error = %e, "Failed to introspect collection");
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list collections; falling back to YAML schemas only");
        }
    }
}

/// Load YAML schema files and merge with runtime-discovered schemas.
/// YAML fields with descriptions override introspected fields.
/// YAML-only collections are added directly.
async fn merge_yaml_schemas(schemas: &DashMap<String, CollectionSchema>) {
    let schemas_dir = std::path::Path::new("schemas");
    if !schemas_dir.exists() {
        tracing::info!("No schemas/ directory found; using introspected schemas only");
        return;
    }

    let mut entries = match tokio::fs::read_dir(schemas_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read schemas/ directory");
            return;
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "yaml" && ext != "yml" {
            continue;
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to read schema file");
                continue;
            }
        };

        let yaml_schema: CollectionSchema = match serde_yaml_ng::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to parse schema YAML");
                continue;
            }
        };

        if let Some(mut existing) = schemas.get_mut(&yaml_schema.name) {
            let schema = existing.value_mut();

            // Merge: YAML descriptions override introspected ones
            if !yaml_schema.description.is_empty() {
                schema.description = yaml_schema.description.clone();
            }

            // Enrich introspected fields with YAML descriptions
            for yaml_field in &yaml_schema.fields {
                if let Some(field) = schema.fields.iter_mut().find(|f| f.name == yaml_field.name) {
                    if yaml_field.description.is_some() {
                        field.description = yaml_field.description.clone();
                    }
                    // YAML type can override if more specific
                    if !yaml_field.field_type.is_empty() {
                        field.field_type = yaml_field.field_type.clone();
                    }
                }
                // If YAML has a field not in introspected data, skip it (stale schema)
            }

            schema.source = "merged".to_string();
            tracing::info!(collection = %schema.name, "Merged YAML descriptions into introspected schema");
        } else {
            // Collection exists in YAML but not in DB — add as yaml-only
            tracing::info!(
                collection = %yaml_schema.name,
                fields = yaml_schema.fields.len(),
                "Loaded YAML-only schema (collection not found in DB)"
            );
            schemas.insert(yaml_schema.name.clone(), yaml_schema);
        }
    }
}
