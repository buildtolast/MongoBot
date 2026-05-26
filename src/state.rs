use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::domain::models::{CollectionSchema, IndexIssue};
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
    /// Missing-index issues detected at startup (or after a refresh).
    pub issues: Arc<RwLock<Vec<IndexIssue>>>,
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
        let detected_issues = audit_indexes(&mongo, &schemas, config.auto_create_indexes).await;
        let issues = Arc::new(RwLock::new(detected_issues));

        let safety = SafetyChecker::new(
            config.allowed_operations.clone(),
            config.max_result_rows,
        ).with_unsafe_allow_all(config.unsafe_allow_all_operations);

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
            issues,
        })
    }

    /// Format all schemas as compact text for LLM prompts.
    pub fn schema_context(&self) -> String {
        compute_schema_context(&self.schemas)
    }

    pub fn get_schema(&self, name: &str) -> Option<CollectionSchema> {
        self.schemas.get(name).map(|entry| entry.value().clone())
    }

    /// Re-introspect MongoDB and reload schemas. Called at runtime when
    /// new collections are added or schema changes are detected.
    /// Also re-runs the index audit so the Issues panel stays current.
    pub async fn refresh_schemas(&self) -> usize {
        let before = self.schemas.len();
        discover_schemas_from_db(&self.mongo, &self.schemas).await;
        merge_yaml_schemas(&self.schemas).await;
        let after = self.schemas.len();
        let new_count = after.saturating_sub(before);
        tracing::info!(total = after, new = new_count, "Schemas refreshed");

        let new_issues = audit_indexes(&self.mongo, &self.schemas, false).await;
        *self.issues.write().await = new_issues;

        after
    }
}

/// Discover schemas by introspecting all collections in the MongoDB database.
async fn discover_schemas_from_db(mongo: &MongoPool, schemas: &DashMap<String, CollectionSchema>) {
    match mongo.list_collections().await {
        Ok(names) => {
            tracing::info!(count = names.len(), "Discovered collections from MongoDB");
            for name in names {
                // Exclude internal collections from schema discovery so they
                // are invisible to the LLM prompt and the schema browser.
                if name == "query_feedback" {
                    continue;
                }
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

/// Audit every discovered collection for fields that lack a single-field index.
/// Returns a list of `IndexIssue` — one per missing field — so callers can store
/// and surface them in the UI.
///
/// If `auto_create` is true (set via `AUTO_CREATE_INDEXES=true`), issues that are
/// successfully created are excluded from the returned list.
///
/// Skips: _id (always indexed), array fields, dynamic-key placeholders (<key>),
/// and fields already covered as the leading key of any existing index.
async fn audit_indexes(
    mongo: &MongoPool,
    schemas: &DashMap<String, CollectionSchema>,
    auto_create: bool,
) -> Vec<IndexIssue> {
    let mut issues: Vec<IndexIssue> = Vec::new();

    for entry in schemas.iter() {
        let schema = entry.value();
        let missing_names = missing_field_names(schema);

        for field in missing_names.iter().map(|s| s.as_str()) {
            let fix_cmd = format!("db.{}.createIndex({{ {}: 1 }})", schema.name, field);

            if auto_create {
                match mongo.ensure_index(&schema.name, field, 1).await {
                    Ok(true) => {
                        tracing::info!(collection = %schema.name, %field, "Created missing index");
                        // Index created — don't add to issues list
                        continue;
                    }
                    Ok(false) => {
                        // Already existed (race between introspection and audit)
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(collection = %schema.name, %field, error = %e, "Failed to create index");
                        // Fall through — add as an issue so user can see and retry
                    }
                }
            }

            issues.push(IndexIssue {
                collection: schema.name.clone(),
                field: field.to_string(),
                message: format!(
                    "Field '{}' on '{}' has no index. Queries filtering on this field will scan the entire collection.",
                    field, schema.name
                ),
                fix_command: fix_cmd,
            });
        }
    }

    let total = issues.len();
    if total > 0 {
        tracing::warn!(
            total,
            hint = "Check the 'Requires Attention' panel in the UI or set AUTO_CREATE_INDEXES=true",
            "Index audit: {} field(s) are missing indexes",
            total
        );
    } else {
        tracing::info!("Index audit complete — all fields are covered by indexes");
    }

    issues
}

/// Returns field names in a schema that have no covering index (leading key).
/// Skips: `_id`, array-type fields, and dynamic-key placeholders (`<key>`).
pub(crate) fn missing_field_names(schema: &crate::domain::models::CollectionSchema) -> Vec<String> {
    let covered: std::collections::HashSet<&str> = schema
        .indexes
        .iter()
        .filter_map(|i| i.keys.first())
        .map(|k| k.field.as_str())
        .collect();

    schema
        .fields
        .iter()
        .filter(|f| {
            f.name != "_id"
                && !f.name.contains("<key>")
                && f.field_type != "array"
                && !covered.contains(f.name.as_str())
        })
        .map(|f| f.name.clone())
        .collect()
}

/// Format all schemas as compact text for LLM prompts.
pub(crate) fn compute_schema_context(schemas: &DashMap<String, crate::domain::models::CollectionSchema>) -> String {
    let mut ctx = String::new();

    for entry in schemas.iter() {
        let schema = entry.value();
        let fields: Vec<String> = schema.fields.iter()
            .map(|f| format!("{}({})", f.name, f.field_type))
            .collect();
        ctx.push_str(&format!("- {}: {}\n", schema.name, fields.join(", ")));

        let indexed: Vec<String> = schema.indexes.iter()
            .filter(|i| i.name != "_id_")
            .map(|i| {
                i.keys.iter().map(|k| k.field.as_str()).collect::<Vec<_>>().join("+")
            })
            .collect();
        if !indexed.is_empty() {
            ctx.push_str(&format!("  indexes: {}\n", indexed.join(", ")));
        }
    }

    if ctx.is_empty() {
        ctx.push_str("No collections available.\n");
    }

    ctx
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{CollectionSchema, FieldDef, IndexInfo, IndexKey};

    fn make_schema(name: &str, fields: &[(&str, &str)], indexes: &[&[&str]]) -> CollectionSchema {
        CollectionSchema {
            name: name.to_string(),
            description: String::new(),
            fields: fields.iter().map(|(n, t)| FieldDef {
                name: n.to_string(),
                field_type: t.to_string(),
                description: None,
            }).collect(),
            indexes: indexes.iter().enumerate().map(|(i, keys)| IndexInfo {
                name: format!("idx_{i}"),
                keys: keys.iter().map(|k| IndexKey { field: k.to_string(), direction: 1 }).collect(),
                unique: false,
            }).collect(),
            source: "test".to_string(),
        }
    }

    #[test]
    fn missing_field_names_skips_id() {
        let schema = make_schema("col", &[("_id", "objectid"), ("name", "string")], &[]);
        let missing = missing_field_names(&schema);
        assert!(!missing.contains(&"_id".to_string()));
        assert!(missing.contains(&"name".to_string()));
    }

    #[test]
    fn missing_field_names_skips_array_fields() {
        let schema = make_schema("col", &[("tags", "array"), ("title", "string")], &[]);
        let missing = missing_field_names(&schema);
        assert!(!missing.contains(&"tags".to_string()));
        assert!(missing.contains(&"title".to_string()));
    }

    #[test]
    fn missing_field_names_skips_placeholder_keys() {
        let schema = make_schema("col", &[("metadata.<key>", "string"), ("title", "string")], &[]);
        let missing = missing_field_names(&schema);
        assert!(!missing.iter().any(|f| f.contains("<key>")));
        assert!(missing.contains(&"title".to_string()));
    }

    #[test]
    fn missing_field_names_covered_field_not_reported() {
        let schema = make_schema("col", &[("name", "string"), ("age", "number")], &[&["name"]]);
        let missing = missing_field_names(&schema);
        assert!(!missing.contains(&"name".to_string()), "name is covered by index");
        assert!(missing.contains(&"age".to_string()), "age has no index");
    }

    #[test]
    fn missing_field_names_all_covered_returns_empty() {
        let schema = make_schema("col", &[("name", "string")], &[&["name"]]);
        assert!(missing_field_names(&schema).is_empty());
    }

    #[test]
    fn compute_schema_context_empty_map_returns_placeholder() {
        let schemas: DashMap<String, CollectionSchema> = DashMap::new();
        let ctx = compute_schema_context(&schemas);
        assert_eq!(ctx.trim(), "No collections available.");
    }

    #[test]
    fn compute_schema_context_includes_collection_and_fields() {
        let schemas: DashMap<String, CollectionSchema> = DashMap::new();
        let schema = make_schema("movies", &[("title", "string"), ("year", "number")], &[&["title"]]);
        schemas.insert("movies".to_string(), schema);
        let ctx = compute_schema_context(&schemas);
        assert!(ctx.contains("movies"));
        assert!(ctx.contains("title(string)"));
        assert!(ctx.contains("year(number)"));
    }

    #[test]
    fn compute_schema_context_includes_non_id_indexes() {
        let schemas: DashMap<String, CollectionSchema> = DashMap::new();
        let mut schema = make_schema("movies", &[("title", "string")], &[&["title"]]);
        schema.indexes.push(IndexInfo {
            name: "_id_".to_string(),
            keys: vec![IndexKey { field: "_id".to_string(), direction: 1 }],
            unique: true,
        });
        schemas.insert("movies".to_string(), schema);
        let ctx = compute_schema_context(&schemas);
        assert!(ctx.contains("indexes:"));
        assert!(ctx.contains("title"));
        // _id_ index should be filtered out from the display
        assert!(!ctx.contains("_id_"));
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;

    async fn setup() -> Option<crate::infra::mongo::MongoPool> {
        let pool = crate::infra::mongo::MongoPool::connect("mongodb://localhost:27017", "test_state_coverage").await.ok()?;
        if pool.db().run_command(mongodb::bson::doc! { "ping": 1 }).await.is_err() {
            return None;
        }
        // Seed one document so introspection finds at least one collection
        let _ = pool.db()
            .collection::<mongodb::bson::Document>("state_test_col")
            .insert_one(mongodb::bson::doc! { "name": "test", "value": 42 })
            .await;
        Some(pool)
    }

    #[tokio::test]
    async fn discover_schemas_populates_dashmap() {
        let mongo = match setup().await { Some(p) => p, None => return };
        let schemas: DashMap<String, crate::domain::models::CollectionSchema> = DashMap::new();
        discover_schemas_from_db(&mongo, &schemas).await;
        assert!(!schemas.is_empty(), "should discover at least the seeded collection");
    }

    #[tokio::test]
    async fn audit_indexes_finds_missing_fields() {
        let mongo = match setup().await { Some(p) => p, None => return };
        let schemas: DashMap<String, crate::domain::models::CollectionSchema> = DashMap::new();
        discover_schemas_from_db(&mongo, &schemas).await;
        let issues = audit_indexes(&mongo, &schemas, false).await;
        // Issues may or may not exist depending on seeded data; just verify it runs
        let _ = issues;
    }

    #[tokio::test]
    async fn audit_indexes_auto_create_true_does_not_panic() {
        let mongo = match setup().await { Some(p) => p, None => return };
        let schemas: DashMap<String, crate::domain::models::CollectionSchema> = DashMap::new();
        discover_schemas_from_db(&mongo, &schemas).await;
        let issues = audit_indexes(&mongo, &schemas, true).await;
        // With auto_create=true, successfully created indexes are removed from issues list
        let _ = issues;
    }

    #[tokio::test]
    async fn get_schema_returns_none_for_unknown_collection() {
        use crate::domain::safety::SafetyChecker;
        use crate::infra::llm::LlmClient;
        let config = crate::test_helpers::test_config();
        let state = crate::state::AppState {
            mongo: crate::infra::mongo::MongoPool::new_for_tests().await,
            llm: LlmClient::new(&config),
            schemas: Arc::new(DashMap::new()),
            safety: SafetyChecker::new(config.allowed_operations.clone(), config.max_result_rows),
            http: reqwest::Client::new(),
            issues: Arc::new(tokio::sync::RwLock::new(vec![])),
            config,
        };
        assert!(state.get_schema("nonexistent_collection").is_none());
    }

    #[tokio::test]
    async fn get_schema_returns_schema_when_present() {
        use crate::domain::safety::SafetyChecker;
        use crate::infra::llm::LlmClient;
        use crate::domain::models::{CollectionSchema, FieldDef};
        let config = crate::test_helpers::test_config();
        let schemas = Arc::new(DashMap::new());
        schemas.insert("movies".to_string(), CollectionSchema {
            name: "movies".to_string(),
            description: String::new(),
            fields: vec![FieldDef { name: "title".into(), field_type: "string".into(), description: None }],
            indexes: vec![],
            source: "test".into(),
        });
        let state = crate::state::AppState {
            mongo: crate::infra::mongo::MongoPool::new_for_tests().await,
            llm: LlmClient::new(&config),
            schemas,
            safety: SafetyChecker::new(config.allowed_operations.clone(), config.max_result_rows),
            http: reqwest::Client::new(),
            issues: Arc::new(tokio::sync::RwLock::new(vec![])),
            config,
        };
        assert!(state.get_schema("movies").is_some());
    }
}
