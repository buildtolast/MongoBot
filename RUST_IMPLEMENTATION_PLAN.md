# Slack-Mongo Query Bot — Rust Implementation Plan

## Overview

A Slack bot **and web dashboard** that translates natural language queries into MongoDB queries using a **local LLM** (Ollama), executes them safely, and returns results or exports to CSV/JSON. Built on **actix-web** with an embedded single-file HTML frontend (same pattern as GitWorkflowDashboard). Users can query via Slack slash commands **or** through the browser-based UI with direct file downloads.

```
┌───────────────────────────┐  ┌──────────────────────────────┐
│       SLACK LAYER         │  │        WEB UI LAYER          │
│ /mongo  @bot  Modals      │  │ Browser dashboard            │
│                           │  │ NL query bar + results table │
│                           │  │ CSV/JSON download buttons    │
└─────────────┬─────────────┘  └──────────────┬───────────────┘
              │ HTTP webhooks                  │ HTTP API + WS
              ▼                                ▼
┌─────────────────────────────────────────────────────────────┐
│              ACTIX-WEB SERVER (actix-web 4)                 │
│  /api/slack/*   /api/query   /api/export   /api/schemas     │
│  /ws/live       /  (embedded HTML)         /api/health      │
│  Middleware: auth, CORS, CSP, Slack verify, rate limit      │
│  Shared state: Arc<RwLock<T>>                               │
└──────┬──────────────┬──────────────┬────────────────────────┘
       ▼              ▼              ▼
┌────────────┐ ┌────────────┐ ┌──────────────┐
│ NLP PARSER │ │ QUERY EXEC │ │ EXPORT       │
│ Ollama API │ │ MongoDB    │ │ CSV/JSON     │
│ (default)  │ │ driver     │ │ stream to    │
│ + fallback │ │ + safety   │ │ browser or   │
│   cloud    │ │   checker  │ │ Slack file   │
└────────────┘ └────────────┘ └──────────────┘
```

---

## Tech Stack

### Cargo.toml

```toml
[package]
name = "slack-mongo-bot"
version = "0.1.0"
edition = "2021"

[dependencies]
# Web framework (same as GitWorkflowDashboard)
actix-web = "4"
actix-ws = "0.3"
actix-governor = "0.6"          # Rate limiting

# Slack SDK (actively maintained, supports Events/Socket Mode)
slack-morphism = { version = "2.19", features = ["hyper"] }

# MongoDB (official driver)
mongodb = { version = "3.5", features = ["tokio-runtime"] }

# Local LLM via Ollama
ollama-rs = { version = "0.3", features = ["stream"] }

# HTTP client (for cloud LLM fallback + Slack file uploads)
reqwest = { version = "0.13", features = ["json", "stream", "rustls-tls"] }

# Async runtime
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "signal", "fs"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_yaml_ng = "0.10"          # YAML config (maintained fork)

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-actix-web = "0.7"

# Config
dotenvy = "0.15"
clap = { version = "4", features = ["derive"] }  # CLI args

# Security
hmac = "0.12"
sha2 = "0.10"
subtle = "2"                    # Constant-time auth comparison

# Async utilities
futures = "0.3"

# Validation
regex = "1.11"
chrono = { version = "0.4", features = ["serde"] }

# Concurrent cache
dashmap = "6.1"

# CSV export
csv = "1.3"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

---

## Project Structure

```
slack-mongo-bot/
├── Cargo.toml
├── build.rs                    # Assembles frontend into $OUT_DIR/index.html
├── .env.example
├── schemas/                    # Collection metadata (YAML)
│   └── example.yaml
│
├── src/
│   ├── main.rs                 # Entry point, actix-web server bootstrap
│   ├── lib.rs                  # Module exports
│   ├── config.rs               # CLI args (clap) + env var merging
│   ├── error.rs                # AppError enum (thiserror + actix ResponseError)
│   ├── state.rs                # Shared AppState (Arc<RwLock<T>>)
│   │
│   ├── infra/                  # External service clients
│   │   ├── mod.rs
│   │   ├── mongo.rs            # MongoDB pool + query helpers
│   │   ├── slack.rs            # Slack API client (slack-morphism)
│   │   └── llm.rs              # LLM client: Ollama primary, cloud fallback
│   │
│   ├── domain/                 # Business logic (framework-independent)
│   │   ├── mod.rs
│   │   ├── models.rs           # ParsedQuery, Filter, SortSpec, QueryResult, etc.
│   │   ├── parser.rs           # NL → ParsedQuery via LLM
│   │   ├── executor.rs         # ParsedQuery → MongoDB query → results
│   │   ├── safety.rs           # Validates queries before execution
│   │   └── export.rs           # CSV/JSON streaming writer
│   │
│   ├── handlers/               # Actix-web request handlers
│   │   ├── mod.rs
│   │   ├── slack_commands.rs   # Slack slash command + event handlers
│   │   ├── slack_actions.rs    # Interactive component callbacks
│   │   ├── query.rs            # POST /api/query — Web UI query endpoint
│   │   ├── export.rs           # GET /api/export — CSV/JSON download endpoint
│   │   ├── schemas.rs          # GET /api/schemas — list available schemas
│   │   ├── ws.rs               # WebSocket /ws/live — real-time query progress
│   │   └── health.rs           # GET /api/health
│   │
│   ├── middleware/
│   │   ├── mod.rs
│   │   ├── auth.rs             # Bearer token auth for /api/* routes
│   │   └── slack_verify.rs     # HMAC-SHA256 Slack signing secret
│   │
│   └── frontend/               # Embedded web UI (compile-time assembled)
│       ├── mod.rs              # include_str!(concat!(env!("OUT_DIR"), "/index.html"))
│       ├── shell_head.html     # DOCTYPE, meta, <style> opener
│       ├── shell_mid.html      # </style>, body, <script> opener
│       ├── shell_foot.html     # </script></body></html>
│       ├── styles/
│       │   ├── 01_variables.css
│       │   ├── 02_layout.css
│       │   ├── 03_query_bar.css
│       │   ├── 04_results_table.css
│       │   ├── 05_export_panel.css
│       │   ├── 06_schema_browser.css
│       │   └── 07_responsive.css
│       └── js/
│           ├── 01_state.js
│           ├── 02_api.js       # fetch /api/query, /api/export, /api/schemas
│           ├── 03_websocket.js # Connect to /ws/live for progress
│           ├── 04_render.js    # DOM rendering, results table, pagination
│           ├── 05_export.js    # Trigger CSV/JSON download
│           └── 06_app.js       # Init, event listeners
│
└── tests/
    ├── integration.rs
    └── fixtures/               # Mock LLM responses, sample schemas
```

**Frontend pattern**: Identical to GitWorkflowDashboard — `build.rs` concatenates `shell_head.html` + `styles/*.css` + `shell_mid.html` + `js/*.js` + `shell_foot.html` into `$OUT_DIR/index.html`. The binary embeds this via `include_str!()`. Single binary deployment, no Node.js, no static file serving.

---

## Module Designs

### 1. Config (`src/config.rs`)

```rust
use clap::Parser;
use serde::Deserialize;

#[derive(Parser, Debug, Clone)]
#[command(name = "slack-mongo-bot")]
pub struct AppConfig {
    // Slack (optional — web UI works without Slack)
    #[arg(long, env = "SLACK_BOT_TOKEN")]
    pub slack_bot_token: Option<String>,
    #[arg(long, env = "SLACK_SIGNING_SECRET")]
    pub slack_signing_secret: Option<String>,

    // MongoDB
    #[arg(long, env = "MONGO_URI", default_value = "mongodb://localhost:27017")]
    pub mongo_uri: String,
    #[arg(long, env = "MONGO_DB_NAME")]
    pub mongo_db_name: String,

    // LLM
    #[arg(long, env = "LLM_PROVIDER", default_value = "ollama")]
    pub llm_provider: LlmProvider,
    #[arg(long, env = "OLLAMA_URL", default_value = "http://localhost:11434")]
    pub ollama_url: String,
    #[arg(long, env = "OLLAMA_MODEL", default_value = "llama3.2")]
    pub ollama_model: String,
    #[arg(long, env = "CLOUD_LLM_API_KEY")]
    pub cloud_llm_api_key: Option<String>,
    #[arg(long, env = "CLOUD_LLM_MODEL")]
    pub cloud_llm_model: Option<String>,

    // Safety
    #[arg(long, env = "MAX_RESULT_ROWS", default_value = "10000")]
    pub max_result_rows: usize,
    #[arg(long, env = "ALLOWED_OPERATIONS", value_delimiter = ',',
          default_values_t = vec!["find".into(), "count".into(), "distinct".into(), "aggregate".into()])]
    pub allowed_operations: Vec<String>,

    // Server
    #[arg(long, env = "HOST", default_value = "0.0.0.0")]
    pub host: String,
    #[arg(long, env = "PORT", default_value_t = 3000)]
    pub port: u16,

    // Auth (optional — protects /api/* routes)
    #[arg(long, env = "DASHBOARD_TOKEN")]
    pub dashboard_token: Option<String>,
}

#[derive(Debug, Clone, Default, clap::ValueEnum)]
pub enum LlmProvider {
    #[default]
    Ollama,
    Anthropic,
    OpenAI,
}
```

**Changes from previous version**: Uses `clap` derive for CLI args with env var fallback (same pattern as GitWorkflowDashboard). Slack config is now `Option<String>` — the web UI works standalone without Slack configured. Added `dashboard_token` for optional auth on the web UI.

---

### 2. LLM Client (`src/infra/llm.rs`)

**Ollama is the default and primary** parser.

```rust
use ollama_rs::Ollama;
use ollama_rs::generation::completion::request::GenerationRequest;
use crate::config::{AppConfig, LlmProvider};
use crate::domain::models::ParsedQuery;

pub struct LlmClient {
    ollama: Option<Ollama>,
    http: reqwest::Client,
    provider: LlmProvider,
    model: String,
}

impl LlmClient {
    pub fn new(config: &AppConfig) -> Self {
        let ollama = match config.llm_provider {
            LlmProvider::Ollama => {
                Some(Ollama::new(&config.ollama_url, 11434))
            }
            _ => None,
        };

        Self {
            ollama,
            http: reqwest::Client::new(),
            provider: config.llm_provider.clone(),
            model: match config.llm_provider {
                LlmProvider::Ollama => config.ollama_model.clone(),
                _ => config.cloud_llm_model.clone().unwrap_or_default(),
            },
        }
    }

    /// Parse natural language to structured MongoDB query
    pub async fn parse_query(
        &self,
        user_query: &str,
        schema_context: &str,
    ) -> anyhow::Result<ParsedQuery> {
        let prompt = build_prompt(user_query, schema_context);

        let raw_json = match self.provider {
            LlmProvider::Ollama => self.call_ollama(&prompt).await?,
            LlmProvider::Anthropic => self.call_anthropic(&prompt).await?,
            LlmProvider::OpenAI => self.call_openai(&prompt).await?,
        };

        parse_llm_response(&raw_json)
    }

    async fn call_ollama(&self, prompt: &str) -> anyhow::Result<String> {
        let ollama = self.ollama.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Ollama not configured"))?;

        let request = GenerationRequest::new(self.model.clone(), prompt.to_string())
            .format(ollama_rs::generation::format::FormatType::Json);

        let response = ollama.generate(request).await?;
        Ok(response.response)
    }

    async fn call_anthropic(&self, prompt: &str) -> anyhow::Result<String> {
        todo!("Cloud fallback — implement when needed")
    }

    async fn call_openai(&self, prompt: &str) -> anyhow::Result<String> {
        todo!("Cloud fallback — implement when needed")
    }
}

fn build_prompt(query: &str, schema: &str) -> String {
    format!(
        r#"You are a MongoDB query translator. Convert the natural language request into JSON.

AVAILABLE COLLECTIONS AND SCHEMA:
{schema}

Return ONLY valid JSON matching this structure:
{{
  "collection": "collection_name",
  "operation": "find" | "count" | "aggregate" | "distinct",
  "filters": [
    {{ "field": "field_name", "operator": "eq|ne|gt|gte|lt|lte|in|contains|regex", "value": <json_value> }}
  ],
  "projection": ["field1", "field2"],
  "sort": {{ "field": "field_name", "order": 1 }},
  "limit": 100
}}

Rules:
- Only use fields from the schema above
- Infer date ranges from phrases like "last week", "past 30 days"
- Default limit to 100 if not specified
- Return ONLY JSON, no markdown

USER REQUEST: {query}"#
    )
}

fn parse_llm_response(raw: &str) -> anyhow::Result<ParsedQuery> {
    let clean = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(clean)
        .map_err(|e| anyhow::anyhow!("Failed to parse LLM response: {e}\nRaw: {clean}"))
}
```

---

### 3. MongoDB Client (`src/infra/mongo.rs`)

```rust
use mongodb::{Client, Database, Collection};
use mongodb::bson::Document;
use futures::TryStreamExt;

pub struct MongoPool {
    client: Client,
    db_name: String,
}

impl MongoPool {
    pub async fn connect(uri: &str, db_name: &str) -> anyhow::Result<Self> {
        let mut options = mongodb::options::ClientOptions::parse(uri).await?;
        options.max_pool_size = Some(20);
        options.min_pool_size = Some(2);

        let client = Client::with_options(options)?;

        // Verify connectivity
        client.database(db_name)
            .run_command(mongodb::bson::doc! { "ping": 1 })
            .await?;

        Ok(Self { client, db_name: db_name.to_string() })
    }

    pub fn db(&self) -> Database {
        self.client.database(&self.db_name)
    }

    pub fn collection(&self, name: &str) -> Collection<Document> {
        self.db().collection(name)
    }

    pub async fn find_documents(
        &self,
        collection: &str,
        filter: Document,
        sort: Option<Document>,
        limit: i64,
        projection: Option<Document>,
    ) -> anyhow::Result<Vec<Document>> {
        let coll = self.collection(collection);

        let find_options = mongodb::options::FindOptions::builder()
            .sort(sort)
            .limit(Some(limit))
            .projection(projection)
            .build();

        let cursor = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            coll.find(filter).with_options(find_options),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Query timed out after 30s"))??;

        let docs: Vec<Document> = cursor.try_collect().await?;
        Ok(docs)
    }
}
```

---

### 4. Domain Models (`src/domain/models.rs`)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedQuery {
    pub collection: String,
    pub operation: QueryOperation,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub projection: Vec<String>,
    pub sort: Option<SortSpec>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize { 100 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperation {
    Find, Count, Aggregate, Distinct,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub field: String,
    pub operator: FilterOperator,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    Eq, Ne, Gt, Gte, Lt, Lte, In, Nin, Contains, Regex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortSpec {
    pub field: String,
    pub order: i32,
}

/// Schema metadata loaded from YAML files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionSchema {
    pub name: String,
    pub description: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub field_type: String,
    pub description: Option<String>,
}

/// Web UI query request
#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub query: String,                     // Natural language query
    pub format: Option<ExportFormat>,      // If set, return as file download
}

/// Web UI query response
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub parsed: ParsedQuery,               // Show user what the LLM understood
    pub results: serde_json::Value,        // Documents/count/values
    pub count: usize,
    pub execution_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Json,
}
```

---

### 5. Safety Checker (`src/domain/safety.rs`)

```rust
use crate::domain::models::{ParsedQuery, CollectionSchema};

pub struct SafetyChecker {
    allowed_operations: Vec<String>,
    max_rows: usize,
}

impl SafetyChecker {
    pub fn new(allowed_operations: Vec<String>, max_rows: usize) -> Self {
        Self { allowed_operations, max_rows }
    }

    pub fn validate(
        &self,
        query: &ParsedQuery,
        schema: &CollectionSchema,
    ) -> Result<(), SafetyError> {
        // 1. Check operation is allowed
        let op = serde_json::to_value(&query.operation)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if !self.allowed_operations.contains(&op) {
            return Err(SafetyError::OperationBlocked(op));
        }

        // 2. Check all filter fields exist in schema
        let schema_fields: Vec<&str> = schema.fields.iter()
            .map(|f| f.name.as_str()).collect();
        for filter in &query.filters {
            if !schema_fields.contains(&filter.field.as_str()) {
                return Err(SafetyError::UnknownField(filter.field.clone()));
            }
        }

        // 3. Cap the limit
        if query.limit > self.max_rows {
            return Err(SafetyError::LimitExceeded {
                requested: query.limit,
                max: self.max_rows,
            });
        }

        // 4. Injection detection
        for filter in &query.filters {
            check_injection(&filter.value)?;
        }

        Ok(())
    }
}

fn check_injection(value: &serde_json::Value) -> Result<(), SafetyError> {
    let s = value.to_string();
    if s.contains("$where") || s.contains("__proto__") || s.contains("constructor") {
        return Err(SafetyError::InjectionDetected);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum SafetyError {
    #[error("Operation blocked: {0}")]
    OperationBlocked(String),
    #[error("Unknown field: {0} (not in schema)")]
    UnknownField(String),
    #[error("Limit exceeded: requested {requested}, max {max}")]
    LimitExceeded { requested: usize, max: usize },
    #[error("Potential NoSQL injection detected")]
    InjectionDetected,
}
```

---

### 6. Query Executor (`src/domain/executor.rs`)

```rust
use mongodb::bson::{doc, Document, Bson};
use crate::domain::models::*;
use crate::infra::mongo::MongoPool;

pub async fn execute(
    pool: &MongoPool,
    query: &ParsedQuery,
) -> anyhow::Result<QueryResult> {
    let filter = build_filter(&query.filters)?;
    let sort = query.sort.as_ref().map(|s| doc! { &s.field: s.order });
    let projection = if query.projection.is_empty() {
        None
    } else {
        let mut proj = Document::new();
        for f in &query.projection { proj.insert(f.as_str(), 1); }
        Some(proj)
    };

    match query.operation {
        QueryOperation::Find => {
            let docs = pool.find_documents(
                &query.collection, filter, sort,
                query.limit as i64, projection,
            ).await?;
            Ok(QueryResult::Documents(docs))
        }
        QueryOperation::Count => {
            let count = pool.collection(&query.collection)
                .count_documents(filter).await?;
            Ok(QueryResult::Count(count))
        }
        QueryOperation::Distinct => {
            let field = query.projection.first()
                .ok_or_else(|| anyhow::anyhow!("Distinct requires a field in projection"))?;
            let values = pool.collection(&query.collection)
                .distinct(field, filter).await?;
            Ok(QueryResult::Values(values))
        }
        QueryOperation::Aggregate => {
            anyhow::bail!("Aggregation pipelines not yet implemented")
        }
    }
}

pub enum QueryResult {
    Documents(Vec<Document>),
    Count(u64),
    Values(Vec<Bson>),
}

fn build_filter(filters: &[Filter]) -> anyhow::Result<Document> {
    let mut doc = Document::new();
    for f in filters {
        let bson_value = mongodb::bson::to_bson(&f.value).unwrap_or(Bson::Null);
        let condition = match f.operator {
            FilterOperator::Eq => bson_value,
            FilterOperator::Ne => Bson::Document(doc! { "$ne": bson_value }),
            FilterOperator::Gt => Bson::Document(doc! { "$gt": bson_value }),
            FilterOperator::Gte => Bson::Document(doc! { "$gte": bson_value }),
            FilterOperator::Lt => Bson::Document(doc! { "$lt": bson_value }),
            FilterOperator::Lte => Bson::Document(doc! { "$lte": bson_value }),
            FilterOperator::In => Bson::Document(doc! { "$in": bson_value }),
            FilterOperator::Nin => Bson::Document(doc! { "$nin": bson_value }),
            FilterOperator::Contains => {
                let p = f.value.as_str().unwrap_or("");
                Bson::Document(doc! { "$regex": p, "$options": "i" })
            }
            FilterOperator::Regex => {
                let p = f.value.as_str().unwrap_or("");
                Bson::Document(doc! { "$regex": p })
            }
        };
        doc.insert(&f.field, condition);
    }
    Ok(doc)
}
```

---

### 7. Web UI Query Handler (`src/handlers/query.rs`)

This is the **new** handler that serves the browser-based query interface.

```rust
use actix_web::{web, HttpResponse};
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::state::AppState;
use crate::domain::models::{QueryRequest, QueryResponse};

/// POST /api/query — Execute a natural language query from the Web UI
pub async fn handle_query(
    state: web::Data<Arc<RwLock<AppState>>>,
    body: web::Json<QueryRequest>,
) -> Result<HttpResponse, actix_web::Error> {
    let state = state.read().await;
    let start = std::time::Instant::now();

    // 1. Parse NL → structured query via LLM
    let schema_context = state.schema_context();
    let parsed = state.llm.parse_query(&body.query, &schema_context).await
        .map_err(actix_web::error::ErrorInternalServerError)?;

    // 2. Validate safety
    let schema = state.get_schema(&parsed.collection)
        .ok_or_else(|| actix_web::error::ErrorBadRequest(
            format!("Unknown collection: {}", parsed.collection)
        ))?;
    state.safety.validate(&parsed, &schema)
        .map_err(actix_web::error::ErrorBadRequest)?;

    // 3. Execute query
    let result = crate::domain::executor::execute(&state.mongo, &parsed).await
        .map_err(actix_web::error::ErrorInternalServerError)?;

    let elapsed = start.elapsed().as_millis() as u64;

    // 4. Serialize result
    let (json_value, count) = match &result {
        crate::domain::executor::QueryResult::Documents(docs) => {
            let count = docs.len();
            (serde_json::to_value(docs).unwrap_or_default(), count)
        }
        crate::domain::executor::QueryResult::Count(n) => {
            (serde_json::json!({ "count": n }), 1)
        }
        crate::domain::executor::QueryResult::Values(vals) => {
            let count = vals.len();
            (serde_json::to_value(vals).unwrap_or_default(), count)
        }
    };

    Ok(HttpResponse::Ok().json(QueryResponse {
        parsed,
        results: json_value,
        count,
        execution_ms: elapsed,
    }))
}
```

---

### 8. Export Handler (`src/handlers/export.rs`)

Streams CSV or JSON directly as a file download to the browser.

```rust
use actix_web::{web, HttpResponse, http::header};
use crate::state::AppState;
use crate::domain::models::ExportFormat;

/// GET /api/export?query=...&format=csv
/// Re-runs the query and streams results as a downloadable file
pub async fn handle_export(
    state: web::Data<Arc<RwLock<AppState>>>,
    params: web::Query<ExportParams>,
) -> Result<HttpResponse, actix_web::Error> {
    let state = state.read().await;

    // Parse + validate + execute (same pipeline as /api/query)
    let schema_context = state.schema_context();
    let parsed = state.llm.parse_query(&params.query, &schema_context).await
        .map_err(actix_web::error::ErrorInternalServerError)?;

    let schema = state.get_schema(&parsed.collection)
        .ok_or_else(|| actix_web::error::ErrorBadRequest("Unknown collection"))?;
    state.safety.validate(&parsed, &schema)
        .map_err(actix_web::error::ErrorBadRequest)?;

    let result = crate::domain::executor::execute(&state.mongo, &parsed).await
        .map_err(actix_web::error::ErrorInternalServerError)?;

    let format = params.format.clone().unwrap_or(ExportFormat::Csv);
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("{}_{}.{}", parsed.collection, timestamp,
        match format { ExportFormat::Csv => "csv", ExportFormat::Json => "json" });

    match (&result, &format) {
        (crate::domain::executor::QueryResult::Documents(docs), ExportFormat::Csv) => {
            let csv_body = crate::domain::export::documents_to_csv(docs)?;
            Ok(HttpResponse::Ok()
                .insert_header(header::ContentDisposition::attachment(filename))
                .content_type("text/csv; charset=utf-8")
                .body(csv_body))
        }
        (crate::domain::executor::QueryResult::Documents(docs), ExportFormat::Json) => {
            let json_body = serde_json::to_string_pretty(docs)
                .map_err(actix_web::error::ErrorInternalServerError)?;
            Ok(HttpResponse::Ok()
                .insert_header(header::ContentDisposition::attachment(filename))
                .content_type("application/json")
                .body(json_body))
        }
        _ => Ok(HttpResponse::BadRequest().body("Export only supported for find operations")),
    }
}

#[derive(serde::Deserialize)]
pub struct ExportParams {
    pub query: String,
    pub format: Option<ExportFormat>,
}
```

---

### 9. Slack Handlers (`src/handlers/slack_commands.rs`)

```rust
use actix_web::{web, HttpResponse};
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct SlashCommandPayload {
    pub token: String,
    pub team_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub command: String,
    pub text: String,
    pub response_url: String,
}

/// POST /api/slack/commands — handles /mongo slash command
pub async fn handle_slash_command(
    state: web::Data<Arc<RwLock<AppState>>>,
    form: web::Form<SlashCommandPayload>,
) -> HttpResponse {
    let query_text = form.text.trim();
    if query_text.is_empty() {
        return HttpResponse::Ok().body(
            "Usage: `/mongo <your query>`\nExample: `/mongo find users created last week`"
        );
    }

    // Respond immediately (Slack requires <3s)
    let state_clone = state.get_ref().clone();
    let response_url = form.response_url.clone();
    let query = query_text.to_string();

    tokio::spawn(async move {
        let state = state_clone.read().await;
        let result = process_query(&state, &query).await;
        let message = match result {
            Ok(text) => text,
            Err(e) => format!("Error: {e}"),
        };

        let _ = state.http.post(&response_url)
            .json(&serde_json::json!({
                "response_type": "in_channel",
                "text": message,
            }))
            .send()
            .await;
    });

    HttpResponse::Ok().body("Processing your query...")
}

async fn process_query(state: &AppState, query: &str) -> anyhow::Result<String> {
    let schema_context = state.schema_context();
    let parsed = state.llm.parse_query(query, &schema_context).await?;

    let schema = state.get_schema(&parsed.collection)
        .ok_or_else(|| anyhow::anyhow!("Unknown collection: {}", parsed.collection))?;
    state.safety.validate(&parsed, &schema)?;

    let result = crate::domain::executor::execute(&state.mongo, &parsed).await?;
    format_slack_result(&parsed, &result)
}

fn format_slack_result(
    query: &crate::domain::models::ParsedQuery,
    result: &crate::domain::executor::QueryResult,
) -> anyhow::Result<String> {
    use crate::domain::executor::QueryResult;
    match result {
        QueryResult::Count(n) => Ok(format!("Count: *{n}* documents in `{}`", query.collection)),
        QueryResult::Documents(docs) => {
            let count = docs.len();
            let preview: Vec<String> = docs.iter().take(5)
                .map(|d| format!("```{}```", serde_json::to_string_pretty(d).unwrap_or_default()))
                .collect();
            Ok(format!("Found *{count}* documents in `{}`:\n{}{}",
                query.collection, preview.join("\n"),
                if count > 5 { format!("\n_...and {} more_", count - 5) } else { String::new() },
            ))
        }
        QueryResult::Values(vals) => {
            let formatted: Vec<String> = vals.iter().take(20)
                .map(|v| format!("`{v}`")).collect();
            Ok(format!("Distinct values: {}", formatted.join(", ")))
        }
    }
}
```

---

### 10. Server Bootstrap (`src/main.rs`)

```rust
use actix_web::{web, App, HttpServer, middleware as actix_mw};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_actix_web::TracingLogger;

mod config;
mod error;
mod state;
mod infra;
mod domain;
mod handlers;
mod middleware;
mod frontend;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::AppConfig::parse();
    let state = Arc::new(RwLock::new(state::AppState::new(&config).await?));
    let state_data = web::Data::new(state);

    let bind_addr = format!("{}:{}", config.host, config.port);
    tracing::info!("Starting server on {bind_addr}");

    HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .wrap(actix_mw::Compress::default())
            .app_data(state_data.clone())

            // Public routes
            .route("/", web::get().to(frontend::serve_index))
            .route("/api/health", web::get().to(handlers::health::health_check))
            .route("/api/version", web::get().to(handlers::health::version))

            // Slack webhooks (verified by signing secret, not dashboard auth)
            .route("/api/slack/commands", web::post().to(handlers::slack_commands::handle_slash_command))
            .route("/api/slack/events", web::post().to(handlers::slack_commands::handle_events))
            .route("/api/slack/actions", web::post().to(handlers::slack_actions::handle_action))

            // Web UI API routes (auth-guarded if DASHBOARD_TOKEN set)
            .service(
                web::scope("/api")
                    // .wrap(middleware::auth::AuthMiddleware)  // Enable when dashboard_token is set
                    .route("/query", web::post().to(handlers::query::handle_query))
                    .route("/export", web::get().to(handlers::export::handle_export))
                    .route("/schemas", web::get().to(handlers::schemas::list_schemas))
            )

            // WebSocket for real-time query progress
            .route("/ws/live", web::get().to(handlers::ws::ws_handler))
    })
    .bind(&bind_addr)?
    .run()
    .await?;

    Ok(())
}
```

---

### 11. Frontend Module (`src/frontend/mod.rs`)

```rust
use actix_web::HttpResponse;

const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.html"));

pub async fn serve_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .insert_header(("Cache-Control", "no-cache"))
        .body(INDEX_HTML)
}
```

---

### 12. build.rs

```rust
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("index.html");
    let frontend = Path::new("src/frontend");

    let mut html = String::new();

    // Shell head (DOCTYPE, meta, <style>)
    html.push_str(&fs::read_to_string(frontend.join("shell_head.html")).unwrap());

    // CSS files in order
    let mut css_files: Vec<_> = fs::read_dir(frontend.join("styles"))
        .unwrap().filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "css").unwrap_or(false))
        .collect();
    css_files.sort_by_key(|e| e.file_name());
    for entry in &css_files {
        html.push_str(&fs::read_to_string(entry.path()).unwrap());
        html.push('\n');
    }

    // Shell mid (</style>, body, <script>)
    html.push_str(&fs::read_to_string(frontend.join("shell_mid.html")).unwrap());

    // JS files in order
    let mut js_files: Vec<_> = fs::read_dir(frontend.join("js"))
        .unwrap().filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "js").unwrap_or(false))
        .collect();
    js_files.sort_by_key(|e| e.file_name());
    for entry in &js_files {
        html.push_str(&fs::read_to_string(entry.path()).unwrap());
        html.push('\n');
    }

    // Shell foot (</script></body></html>)
    html.push_str(&fs::read_to_string(frontend.join("shell_foot.html")).unwrap());

    fs::write(&dest_path, html).unwrap();

    // Rebuild on any frontend file change
    println!("cargo:rerun-if-changed=src/frontend/");
}
```

---

## Web UI Features

The embedded frontend provides:

| Feature | Description |
|---------|-------------|
| **NL Query Bar** | Text input where users type natural language queries |
| **Results Table** | Paginated table rendering MongoDB documents with sortable columns |
| **Query Inspector** | Shows the `ParsedQuery` the LLM generated (transparency) |
| **CSV Download** | Button triggers `GET /api/export?query=...&format=csv` — browser downloads file |
| **JSON Download** | Same endpoint with `format=json` |
| **Schema Browser** | Sidebar showing available collections and their fields |
| **Query History** | Recent queries saved in `localStorage` |
| **Execution Stats** | Shows query time, document count, collection used |
| **WebSocket Progress** | Real-time parsing/execution status for long-running queries |

**Web UI Flow:**
```
Browser                           Server
  |                                  |
  |-- POST /api/query ------------->|
  |   { "query": "find users..." } |
  |                                  |-- Ollama parse
  |                                  |-- Safety check
  |                                  |-- MongoDB query
  |<-- 200 QueryResponse -----------|
  |   { parsed, results, count,    |
  |     execution_ms }             |
  |                                  |
  |-- GET /api/export?format=csv -->|
  |                                  |-- Re-run query
  |<-- 200 Content-Disposition -----|
  |   (browser downloads .csv)     |
```

---

## Implementation Order

| Phase | What | Depends On | Effort |
|-------|------|------------|--------|
| **1** | Scaffolding: `Cargo.toml`, `build.rs`, `config.rs`, `error.rs`, `state.rs`, `main.rs` | — | Low |
| **2** | MongoDB client (`infra/mongo.rs`) + health endpoint | Phase 1 | Low |
| **3** | Domain models (`models.rs`) + schema YAML loading | Phase 1 | Low |
| **4** | LLM client with Ollama (`infra/llm.rs`) + parser | Phase 3 | Medium |
| **5** | Safety checker + query executor | Phase 3, 2 | Medium |
| **6** | Web UI: `POST /api/query` handler + basic frontend (query bar + results) | Phase 2-5 | Medium |
| **7** | Export: `GET /api/export` handler + CSV/JSON download buttons | Phase 6 | Low-Med |
| **8** | Slack slash command handler (`/mongo` working) | Phase 2-5 | Medium |
| **9** | Slack events (app mentions), interactive modals | Phase 8 | Medium |
| **10** | WebSocket progress, query history, schema browser | Phase 6 | Medium |
| **11** | Integration tests | Phase 6, 8 | Medium |

**MVP (Phases 1-7)**: Web UI with NL query bar, results table, and CSV/JSON download — works without Slack.

**Full (Phases 1-11)**: Both Slack and Web UI fully functional.

---

## Local LLM Setup (Ollama)

```bash
# Install Ollama
brew install ollama        # macOS
# or: curl -fsSL https://ollama.com/install.sh | sh  # Linux

# Pull a model
ollama pull llama3.2

# Verify
curl http://localhost:11434/api/tags
```

The bot uses `ollama-rs` with `FormatType::Json` which constrains output to valid JSON.

Recommended models:
- `llama3.2` (3B) — fast, good for simple queries
- `llama3.1:8b` — more accurate for complex filters/aggregations
- `mistral` — strong at JSON generation
- `qwen2.5-coder` — good at structured data tasks

---

## .env.example

```bash
# MongoDB (required)
MONGO_URI=mongodb://localhost:27017
MONGO_DB_NAME=myapp

# LLM (default: Ollama)
LLM_PROVIDER=ollama
OLLAMA_URL=http://localhost:11434
OLLAMA_MODEL=llama3.2

# Slack (optional — web UI works without these)
# SLACK_BOT_TOKEN=xoxb-your-token
# SLACK_SIGNING_SECRET=your-signing-secret

# Optional: Cloud LLM fallback
# LLM_PROVIDER=anthropic
# CLOUD_LLM_API_KEY=sk-ant-...
# CLOUD_LLM_MODEL=claude-haiku-4-5-20251001

# Server
HOST=0.0.0.0
PORT=3000

# Auth (optional — protects /api/* routes)
# DASHBOARD_TOKEN=your-secret-token

# Safety
MAX_RESULT_ROWS=10000
ALLOWED_OPERATIONS=find,count,distinct,aggregate

# Logging
RUST_LOG=info
```

---

## Docker

```dockerfile
FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src/ ./src/
COPY schemas/ ./schemas/
RUN cargo build --release && strip target/release/slack-mongo-bot

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/slack-mongo-bot .
COPY --from=builder /app/schemas/ ./schemas/
ENV RUST_LOG=info
EXPOSE 3000
CMD ["./slack-mongo-bot"]
```

Docker Compose with Ollama + MongoDB:
```yaml
services:
  bot:
    build: .
    environment:
      - MONGO_URI=mongodb://mongo:27017
      - MONGO_DB_NAME=myapp
      - OLLAMA_URL=http://ollama:11434
    ports: ["3000:3000"]
    depends_on: [mongo, ollama]

  ollama:
    image: ollama/ollama
    volumes: ["ollama_data:/root/.ollama"]
    ports: ["11434:11434"]

  mongo:
    image: mongo:7
    volumes: ["mongo_data:/data/db"]
    ports: ["27017:27017"]

volumes:
  ollama_data:
  mongo_data:
```
