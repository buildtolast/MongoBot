# Slack-Mongo Query Bot — Architecture Documentation

> Open `architecture-diagram.excalidraw` in [excalidraw.com](https://excalidraw.com) to view the interactive diagram.

---

## System Overview

The Slack-Mongo Query Bot translates natural language questions into MongoDB queries using one of 5 LLM providers (Ollama default, LM Studio, llama.cpp, Anthropic Claude, OpenAI GPT). Users interact through **two interfaces**:

1. **Web Dashboard** — Browser-based UI with a query bar, results table, and CSV/JSON download buttons
2. **Slack Bot** — Slash commands (`/mongo`) and app mentions (`@query-bot`)

Both interfaces share the same backend processing pipeline. The web UI is embedded at compile time (same `build.rs` pattern as GitWorkflowDashboard) — single binary, no Node.js.

**Example flows:**
```
Web UI:   User types "find active users from last week" → sees results table → clicks "Download CSV"
Slack:    User types /mongo find active users from last week → bot posts results in channel
```

---

## Architecture Layers

### 1. Entry Points (Two Interfaces)

#### Web UI (Embedded Frontend)
| Feature | Endpoint | Description |
|---------|----------|-------------|
| Dashboard | `GET /` | Serves embedded `index.html` (query bar, results, schema browser) |
| Query | `POST /api/query` | NL query → JSON response with parsed query + results (supports preview mode) |
| Execute | `POST /api/execute` | Execute a previously previewed/confirmed query (CRUD confirmation flow) |
| Export CSV | `GET /api/export?format=csv` | Re-runs query, streams CSV file download |
| Export JSON | `GET /api/export?format=json` | Re-runs query, streams JSON file download |
| Schemas | `GET /api/schemas` | Lists available collections and fields |
| Refresh Schemas | `POST /api/schemas/refresh` | Re-introspect MongoDB for new/changed collections |
| Models | `GET /api/models` | List available models (accepts `?provider=<name>` for any of the 5 providers) |
| Providers | `GET /api/providers` | List all 5 LLM providers |
| Config | `GET /api/config` | Current configuration with secrets redacted |
| WebSocket | `WS /ws/live` | Real-time query progress updates (scaffold only — no progress events currently published to WS clients) |

#### Slack Bot
| Feature | Endpoint | Description |
|---------|----------|-------------|
| Slash command | `POST /api/slack/commands` | `/mongo <query>` |
| App mention | `POST /api/slack/events` | `@query-bot <query>` |
| Modals | `POST /api/slack/actions` | Interactive component callbacks |

Slack is **optional** — the web UI works standalone without any Slack configuration.

### 2. Actix-web HTTP Server

**Framework**: `actix-web 4` with `actix-ws 0.3` for WebSocket support.

Chosen over Axum because:
- Proven in the GitWorkflowDashboard project with the same embedded-frontend pattern
- `actix-ws` for WebSocket — familiar middleware stack
- `Arc<RwLock<T>>` shared state pattern already well-established
- Same `build.rs` + `include_str!()` frontend embedding approach

**Route Registration:**
```rust
App::new()
    // Public
    .route("/",            GET  → frontend::serve_index)
    .route("/api/health",  GET  → health_check)
    .route("/api/models",  GET  → list_models)
    .route("/api/version", GET  → version)
    .route("/api/config",  GET  → get_config)

    // Slack webhooks (verified by signing secret)
    .route("/api/slack/commands", POST → handle_slash_command)
    .route("/api/slack/events",   POST → handle_events)
    .route("/api/slack/actions",  POST → handle_action)

    // Web UI API (auth-guarded if DASHBOARD_TOKEN set)
    .route("/api/query",           POST → handle_query)
    .route("/api/execute",         POST → handle_execute)
    .route("/api/export",          GET  → handle_export)
    .route("/api/schemas",         GET  → list_schemas)
    .route("/api/schemas/refresh", POST → refresh_schemas)

    // WebSocket
    .route("/ws/live", GET → ws_handler)
```

**Middleware Stack:**
- `TracingLogger` — Structured request logging
- `Compress` — Gzip/Brotli response compression
- `actix-cors` — CORS preflight handling (OPTIONS requests)
- `Auth` — Bearer token validation (optional, for `/api/*` routes)
- `SlackVerify` — HMAC-SHA256 signing secret (for `/api/slack/*` routes only)

**Shared State:**
```rust
pub struct AppState {
    pub mongo: MongoPool,
    pub llm: LlmClient,
    pub schemas: DashMap<String, CollectionSchema>,
    pub safety: SafetyChecker,
    pub http: reqwest::Client,        // For Slack response_url callbacks
    pub config: AppConfig,
}
// Wrapped as web::Data<Arc<RwLock<AppState>>>

// For detailed information on how schemas and feedback are managed,
// see [docs/MEMORY_ARCHITECTURE.md](docs/MEMORY_ARCHITECTURE.md).
```

### 3. Query Processing Pipeline

Both the Web UI and Slack handlers call into the **same** processing pipeline:

```
"find active users from last week"
          │
          ▼
┌─── 1. Schema Context ────┐
│ Auto-discovered from      │
│ MongoDB (14+ collections) │
│ DashMap cache (lock-free) │
└──────────┬────────────────┘
           ▼
┌─── 2. NLP Parser ────────┐
│ 5 providers:             │
│  Ollama (local, default) │
│  LM Studio (local)       │
│  llama.cpp (local)       │
│  Anthropic Claude (cloud)│
│  OpenAI GPT (cloud)      │
│ NL → ParsedQuery struct  │
│ Live model selection     │
└──────────┬───────────────┘
           ▼
┌─── 3. Safety Checker ────┐
│ Field whitelist           │
│ Operation whitelist       │
│ Limit enforcement         │
│ Injection detection       │
└──────────┬───────────────┘
           ▼
┌─── 3b. Query Advisor ───┐
│ 25 rules (21 functions)  │
│ Index usage analysis     │
│ Performance warnings     │
│ ($ne, $nin, unbounded)   │
└──────────┬───────────────┘
           ▼
┌─── 4. Preview / Execute ─┐
│ If approval toggle ON:   │
│   → Return parsed query  │
│   → User reviews JSON    │
│   → User can edit query  │
│   → POST /api/execute    │
│ If toggle OFF:           │
│   → Execute immediately  │
└──────────┬───────────────┘
           ▼
┌─── 5. Query Executor ────┐
│ Build BSON filter/sort   │
│ Date coercion (ISO/year) │
│ $elemMatch auto-grouping │
│ Same-field filter merge  │
│ find/count/distinct/     │
│ insert/update + confirm  │
│ 30s timeout              │
└──────────┬───────────────┘
           ▼
┌─── 6. Response ──────────┐
│ Web UI: JSON response    │
│   + mongo shell command  │
│   + index analysis       │
│   + query advice         │
│   + optional CSV/JSON    │
│ Slack: Block Kit message │
│   via response_url       │
└──────────────────────────┘
```

#### Step 1: Schema Context Loading

Collection schemas are auto-discovered from MongoDB at runtime via runtime introspection (sampling documents to infer field names and types, including array element introspection). Schemas are cached in a `DashMap` (lock-free concurrent hashmap). The schema provides field names, types, and descriptions that the LLM uses to generate accurate queries. Users can refresh schemas at runtime via `POST /api/schemas/refresh` to detect new collections without restarting.

The default database is `movies`, with 14+ collections auto-discovered at runtime. The seed data includes:
- `movies` (16 documents) — title, year, genre, rating, director, etc.
- `actors` (10 documents) — name, birth year, nationality, filmography
- `reviews` (15 documents) — movie reference, reviewer, score, text
- `employees` (5 documents)
- `products` (3 documents)
- `transactions` (1746 documents)
- Plus MongoDB Atlas sample datasets

All collections have indexes for efficient querying.

Example schema (`schemas/movies.yaml`):
```yaml
name: movies
description: Movie catalog with ratings and metadata
fields:
  - name: _id
    field_type: objectId
    description: Unique movie identifier
  - name: title
    field_type: string
  - name: year
    field_type: integer
    description: Release year
  - name: genre
    field_type: string
  - name: rating
    field_type: double
    description: Average rating (0-10)
```

#### Step 2: NLP Parsing (4 LLM Providers)

The natural language query is sent to the configured LLM provider via **direct HTTP calls** (using reqwest) along with schema context. The LLM returns a structured `ParsedQuery` JSON object.

**Providers:**
- **Ollama** (default) -- `/api/generate` with `format: "json"` enforced. Local, free, no internet required.
- **LM Studio** -- OpenAI-compatible API at `/v1/chat/completions`. Local, free, runs at `localhost:1234`.
- **llama.cpp** -- OpenAI-compatible API via `llama-server` binary at `localhost:8080`. Local, free, env vars `LLAMACPP_URL` (default `http://localhost:8080`) and `LLAMACPP_MODEL`.
- **Anthropic Claude** -- `/v1/messages` with `x-api-key` header. Cloud, requires `CLOUD_LLM_API_KEY`.
- **OpenAI GPT** -- `/v1/chat/completions` with Bearer auth. Cloud, requires `CLOUD_LLM_API_KEY`.

LM Studio, llama.cpp, and OpenAI share a `call_openai_compatible()` implementation in `src/infra/llm.rs` that handles the OpenAI chat completions format with `response_format: { "type": "json_object" }`.

**Why Ollama is the default:**
| Factor | Ollama (Local) | LM Studio (Local) | Cloud (Anthropic/OpenAI) |
|--------|---------------|-------------------|-------------------------|
| Cost | Free | Free | $0.25-15 per 1M tokens |
| Latency | ~200-500ms | ~200-500ms | ~500-2000ms |
| Privacy | Data stays local | Data stays local | Data sent to third party |
| Availability | No internet needed | No internet needed | Requires internet + API key |
| JSON reliability | `format: "json"` enforced | `response_format` enforced | Prompt-based (less reliable) |

**Default model:** `Qwen3.8:latest` — reliable JSON output and correct `$lookup` joins across collections.

#### Model Comparison Results

| Model | JSON Schema Compliance | Speed | Sort Format | Verdict |
|-------|----------------------|-------|-------------|---------|
| `qwen3-coder:30b` | High — consistently produces valid ParsedQuery JSON | Slower (~2-5s) | Uses `{ "field": "name", "order": -1 }` (matches schema) | **Recommended default** |
| `gemma3:4b` | Lower — uses different sort format causing parse failures | Faster (~0.5-1s) | Uses `{ "name": -1 }` (flat map, not ParsedQuery-compatible) | Faster but unreliable |

Use `Qwen3.8:latest` (the default) for production reliability: it is the model verified against the multi-collection join tests. Use `gemma3:4b` only if speed is critical and you can tolerate occasional parse failures.

**Example input/output:**
```
Input:  "find active users from last week"

Output (ParsedQuery):
{
  "collection": "users",
  "operation": "find",
  "filters": [
    { "field": "status", "operator": "eq", "value": "active" },
    { "field": "created_at", "operator": "gte", "value": "2026-03-25T00:00:00Z" }
  ],
  "sort": { "field": "created_at", "order": -1 },
  "limit": 100
}
```

#### Step 3: Safety Checker

| Check | Blocks |
|-------|--------|
| Operation whitelist (find, count, distinct, aggregate, insert, update) | delete, drop |
| Field whitelist (must exist in schema) | Typos, hallucinated fields, injection via field names |
| Limit enforcement (default max: 10,000) | Unbounded queries |
| Injection detection | `$where`, `__proto__`, `constructor` patterns |

**Query Advisor (25 rules, 21 check functions):** Runs after safety validation to provide warnings and optimization hints:
- Unbounded query detection (no filters)
- Large limit warnings (>1000)
- Missing index warnings
- `$ne` / `$nin` operator performance warnings
- Regex without anchor performance warnings
- Collection scan detection
- Sort without index warnings
- Projection recommendations for large documents

#### Step 4: Query Executor

Converts `ParsedQuery` to MongoDB operations:
- `find` → `collection.find(filter).with_options(sort, limit, projection)`
- `count` → `collection.count_documents(filter)`
- `distinct` → `collection.distinct(field, filter)`
- `insert` → `collection.insert_one(document)` (with confirmation dialog + backup download)
- `update` → `collection.update_many(filter, update)` (with confirmation + backup)

**Executor features:**
- Date auto-conversion: ISO strings and plain year numbers (e.g. `2020`) are coerced to BSON DateTime via `coerce_value_for_field()` and `try_parse_datetime()`
- `$elemMatch` auto-grouping: filters on array element sub-fields are automatically wrapped in `$elemMatch`
- Same-field filter merging: multiple conditions on the same field (e.g. `price > 10 AND price < 100`) are merged into a single BSON sub-document instead of overwriting
- MongoDB shell command generation: `build_mongo_shell_command()` produces the equivalent `db.collection.find(...)` command for the query viewer panel
- Index analysis: `analyze_index_usage()` reports which indexes would be used

All queries wrapped in `tokio::time::timeout(30s)`.

#### Step 5: Response

**Web UI path:**
- `POST /api/query` returns `QueryResponse` JSON (parsed query + results + count + execution_ms)
- Frontend renders results in a paginated table
- User clicks "Download CSV" → `GET /api/export?query=...&format=csv`
- Server re-runs query, streams CSV with `Content-Disposition: attachment` header
- Browser downloads the file directly

**Slack path:**
- Handler responds with HTTP 200 "Processing..." within 3 seconds (Slack requirement)
- Background `tokio::spawn` task runs the pipeline
- Posts formatted results back via Slack's `response_url`

---

## Web UI Components

The frontend is assembled by `build.rs` from modular source files:

```
src/frontend/
├── shell_head.html      → DOCTYPE, meta, CSP, <style> opener
├── styles/
│   ├── 01_variables.css → CSS custom properties (colors, spacing, fonts)
│   ├── 02_layout.css    → Grid layout, header, sidebar
│   ├── 03_query_bar.css → NL input, submit button, loading state
│   ├── 04_results_table.css → Table, pagination, sortable headers
│   ├── 05_export_panel.css  → Download buttons, format selector
│   ├── 06_schema_browser.css → Collection/field list sidebar
│   └── 07_responsive.css    → Mobile breakpoints
├── shell_mid.html       → </style>, body markup, <script> opener
├── js/
│   ├── 01_state.js      → App state, query history (localStorage)
│   ├── 02_api.js        → fetch() calls to /api/query, /api/export, /api/schemas
│   ├── 03_websocket.js  → /ws/live connection for progress updates
│   ├── 04_render.js     → DOM rendering: results table, pagination, stats
│   ├── 05_export.js     → CSV/JSON download trigger (window.location = /api/export?...)
│   └── 06_app.js        → Init, event listeners, keyboard shortcuts
└── shell_foot.html      → </script></body></html>
```

**UI Features:**
| Feature | Description |
|---------|-------------|
| NL Query Bar | Text input + submit button, shows loading spinner during LLM parsing |
| Approval Toggle | Preview parsed queries before execution; review, edit JSON, and confirm |
| Abort Button | Cancel in-progress queries |
| Results Table | Paginated table with sortable columns, auto-detected from document fields |
| Query Inspector | Collapsible panel showing the `ParsedQuery` JSON the LLM generated |
| MongoDB Query Viewer | Panel showing actual shell commands (`db.collection.find(...)`) |
| Index Analysis | Shows which indexes are used per query |
| Query Advisor | 25-rule (21 check functions) analysis with warnings and optimization hints |
| Model Selector | Switch LLM models at runtime without restart (provider-aware) |
| Settings Panel | Gear icon opens modal showing all config with secrets redacted (`GET /api/config`) |
| CSV Download | Button triggers file download via `/api/export?format=csv` |
| JSON Download | Button triggers file download via `/api/export?format=json` |
| CRUD Confirmation | Insert/update operations show confirmation dialog + backup download |
| Schema Browser | Sidebar listing collections and their fields (from `/api/schemas`) |
| Schema Refresh | Button to re-introspect MongoDB for new collections |
| Query History | Recent queries saved in `localStorage`, click to re-run |
| Execution Stats | Shows query time, document count, collection name |

---

## Data Flow Diagrams

### Web UI Flow
```
Browser                              Server
  |                                     |
  |-- GET / --------------------------->|
  |<-- 200 index.html (embedded) ------|
  |                                     |
  |-- POST /api/query ----------------->|
  |   { "query": "find users..." }     |
  |                                     |-- 1. Schema context
  |                                     |-- 2. Ollama parse
  |                                     |-- 3. Safety check
  |                                     |-- 4. MongoDB query
  |<-- 200 QueryResponse --------------|
  |   { parsed, results, count, ms }   |
  |                                     |
  | [User clicks "Download CSV"]        |
  |-- GET /api/export?format=csv ------>|
  |                                     |-- Re-run pipeline
  |<-- 200 Content-Disposition ---------|
  |   (browser saves users_20260401.csv)|
```

### Slack Flow
```
User                    Slack                    Server
  |                       |                         |
  |-- /mongo "query" ---->|                         |
  |                       |--- POST /api/slack/cmds>|
  |                       |<---- 200 "Processing" --|  (< 3s)
  |                       |                         |
  |                       |                   [tokio::spawn]
  |                       |                   1-4. Pipeline
  |                       |                         |
  |                       |<-- POST response_url ---|
  |<-- "Found 47 docs" --|                         |
```

---

## External Dependencies

```
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────────┐  ┌─────────────────┐
│     Ollama      │  │   LM Studio     │  │   llama.cpp     │  │     MongoDB      │  │  Slack API      │
│  localhost:11434│  │  localhost:1234  │  │  localhost:8080 │  │  localhost:27017  │  │  api.slack.com  │
│                 │  │                 │  │                 │  │                  │  │                 │
│  qwen3-coder    │  │  OpenAI-compat  │  │  OpenAI-compat  │  │  App database    │  │  (optional)     │
│  JSON mode      │  │  chat/completns │  │  chat/completns │  │  Read + write    │  │  Webhooks in    │
│  ~200-500ms     │  │  ~200-500ms     │  │  ~200-500ms     │  │  Pool: 2-20 conn │  │  Results out    │
│                 │  │                 │  │                 │  │                  │  │                 │
│  DEFAULT        │  │  ALTERNATIVE    │  │  ALTERNATIVE    │  │  REQUIRED        │  │  OPTIONAL       │
│  No API costs   │  │  No API costs   │  │  No API costs   │  │                  │  │                 │
└─────────────────┘  └─────────────────┘  └─────────────────┘  └──────────────────┘  └─────────────────┘

Cloud providers (one of, requires CLOUD_LLM_API_KEY):
  Anthropic Claude — api.anthropic.com (or ANTHROPIC_API_URL)
  OpenAI GPT       — api.openai.com (or OPENAI_API_URL)
```

---

## Module Map

```
src/
├── main.rs                → actix_web::main, server bootstrap, route registration
├── lib.rs                 → Module re-exports
├── config.rs              → CLI args (clap) + env vars, validation
├── error.rs               → AppError enum (thiserror + actix ResponseError)
├── state.rs               → AppState (Arc<RwLock<T>> across actix workers)
│
├── infra/                 → External service clients
│   ├── mongo.rs           → MongoPool: connect, find, count, distinct
│   ├── slack.rs           → Slack API helpers (file upload, message post)
│   └── llm.rs             → LlmClient: 5 providers (Ollama, LM Studio, llama.cpp, Anthropic, OpenAI) via direct HTTP
│
├── domain/                → Business logic (no framework dependencies)
│   ├── models.rs          → ParsedQuery, Filter, QueryRequest/Response, ExportFormat, IndexInfo
│   ├── parser.rs          → build_prompt(), parse_llm_response()
│   ├── executor.rs        → ParsedQuery → BSON → MongoDB → QueryResult (date coercion, $elemMatch, filter merge)
│   ├── safety.rs          → SafetyChecker: validate fields, ops, limits
│   ├── query_advisor.rs   → analyze_query(): 25 rules (21 check functions), warnings, optimization hints
│   └── export.rs          → documents_to_csv(), documents_to_json()
│
├── handlers/              → Actix-web request handlers
│   ├── query.rs           → POST /api/query (with preview mode) + POST /api/execute (CRUD confirm)
│   ├── export.rs          → GET /api/export (CSV/JSON download)
│   ├── schemas.rs         → GET /api/schemas + POST /api/schemas/refresh
│   ├── slack_commands.rs  → Slack slash command + event handlers
│   ├── slack_actions.rs   → Interactive component callbacks
│   ├── ws.rs              → WebSocket /ws/live
│   └── health.rs          → GET /api/health + GET /api/models + GET /api/version + GET /api/config
│
├── middleware/
│   ├── auth.rs            → Bearer token (constant-time via subtle)
│   └── slack_verify.rs    → HMAC-SHA256 signing secret
│
├── frontend/              → Embedded web UI (compile-time assembled)
│   ├── mod.rs             → include_str!(OUT_DIR/index.html), serve_index()
│   ├── shell_*.html       → HTML wrapper fragments
│   ├── styles/*.css       → 7 CSS files (numbered for concat order)
│   └── js/*.js            → 6 JS files (numbered for init order)
│
docs/                      → Reference documentation
├── mongodb_query_reference.md → Comprehensive MongoDB query syntax reference
└── query_audit_report.md      → Audit of implementation vs. reference
```

---

## Key Design Decisions

### Why actix-web over Axum?

This project follows the **same architecture as GitWorkflowDashboard**:
- Proven embedded-frontend pattern with `build.rs` + `include_str!()`
- `Arc<RwLock<T>>` shared state across actix workers
- `actix-ws` for WebSocket
- Familiar middleware stack (auth, CORS, CSP, compression)
- `tracing-actix-web` for structured logging
- Constant-time auth via `subtle::ConstantTimeEq`

Keeping the same framework reduces cognitive overhead and enables code reuse.

### Why two interfaces (Web UI + Slack)?

- **Web UI** is better for exploration: see results in a table, browse schemas, download files directly
- **Slack** is better for quick checks without leaving the chat context
- Both share the same pipeline — no logic duplication
- Web UI works standalone (Slack config is optional)

### Why compile-time frontend embedding?

- Single binary deployment — no static files to serve or CDN to configure
- `build.rs` concatenates CSS + JS + HTML into one file at compile time
- Any frontend change triggers recompilation automatically
- Same pattern proven in GitWorkflowDashboard (23 source files → 1 embedded HTML)

### Why DashMap for schema cache?

Schemas are read on every query but written rarely (startup + hot reload). `DashMap` provides lock-free reads via sharded locks, avoiding contention under concurrent query load.

---

## Deployment

### Local Development
```bash
# Prerequisites
ollama pull Qwen3.8:latest             # Local LLM (recommended model)
ollama serve                            # Start Ollama on host
mongod --dbpath /tmp/mongo              # MongoDB

# Run
cargo run -- --mongo-db-name movies
# Open http://localhost:3000
```

### Docker Compose

Ollama runs on the **host machine**, not in Docker. The bot connects via `host.docker.internal`.

```yaml
services:
  bot:
    build: .
    environment:
      MONGO_URI: mongodb://mongo:27017
      MONGO_DB_NAME: movies
      OLLAMA_URL: http://host.docker.internal:11434
      OLLAMA_MODEL: Qwen3.8:latest
    ports: ["3000:3000"]

  mongo:
    image: mongo:7
    volumes: ["mongo_data:/data/db"]
```

### Production
- Single binary (~10MB with strip + LTO)
- `debian:bookworm-slim` Docker image (~15MB total)
- Ollama on host or GPU-enabled machine (not containerized for performance)
- MongoDB via connection string (Atlas or self-hosted)
- Optional: `DASHBOARD_TOKEN` for auth, `SLACK_BOT_TOKEN` for Slack integration
