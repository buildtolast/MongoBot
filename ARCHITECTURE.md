# Slack-Mongo Query Bot — Architecture Documentation

> Open `architecture-diagram.excalidraw` in [excalidraw.com](https://excalidraw.com) to view the interactive diagram.

---

## System Overview

The Slack-Mongo Query Bot translates natural language questions into MongoDB queries using a local LLM (Ollama). Users interact through **two interfaces**:

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
| Query | `POST /api/query` | NL query → JSON response with parsed query + results |
| Export CSV | `GET /api/export?format=csv` | Re-runs query, streams CSV file download |
| Export JSON | `GET /api/export?format=json` | Re-runs query, streams JSON file download |
| Schemas | `GET /api/schemas` | Lists available collections and fields |
| WebSocket | `WS /ws/live` | Real-time query progress updates |

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
- `actix-ws` for WebSocket, `actix-governor` for rate limiting — familiar middleware stack
- `Arc<RwLock<T>>` shared state pattern already well-established
- Same `build.rs` + `include_str!()` frontend embedding approach

**Route Registration:**
```rust
App::new()
    // Public
    .route("/",            GET  → frontend::serve_index)
    .route("/api/health",  GET  → health_check)
    .route("/api/version", GET  → version)

    // Slack webhooks (verified by signing secret)
    .route("/api/slack/commands", POST → handle_slash_command)
    .route("/api/slack/events",   POST → handle_events)
    .route("/api/slack/actions",  POST → handle_action)

    // Web UI API (auth-guarded if DASHBOARD_TOKEN set)
    .route("/api/query",   POST → handle_query)
    .route("/api/export",  GET  → handle_export)
    .route("/api/schemas", GET  → list_schemas)

    // WebSocket
    .route("/ws/live", GET → ws_handler)
```

**Middleware Stack:**
- `TracingLogger` — Structured request logging
- `Compress` — Gzip/Brotli response compression
- `Auth` — Bearer token validation (optional, for `/api/*` routes)
- `SlackVerify` — HMAC-SHA256 signing secret (for `/api/slack/*` routes only)
- Rate limiting via `actix-governor`

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
```

### 3. Query Processing Pipeline

Both the Web UI and Slack handlers call into the **same** processing pipeline:

```
"find active users from last week"
          │
          ▼
┌─── 1. Schema Context ───┐
│ Load YAML from DashMap   │
│ cache (lock-free reads)  │
└──────────┬───────────────┘
           ▼
┌─── 2. NLP Parser ────────┐
│ Ollama (local, default)  │
│ FormatType::Json mode    │
│ NL → ParsedQuery struct  │
└──────────┬───────────────┘
           ▼
┌─── 3. Safety Checker ────┐
│ Field whitelist           │
│ Operation whitelist       │
│ Limit enforcement         │
│ Injection detection       │
└──────────┬───────────────┘
           ▼
┌─── 4. Query Executor ────┐
│ Build BSON filter/sort   │
│ find / count / distinct  │
│ 30s timeout              │
└──────────┬───────────────┘
           ▼
┌─── 5. Response ──────────┐
│ Web UI: JSON response    │
│   + optional CSV/JSON    │
│     file download        │
│ Slack: Block Kit message │
│   via response_url       │
└──────────────────────────┘
```

#### Step 1: Schema Context Loading

Collection schemas are loaded from `schemas/*.yaml` files at startup and cached in a `DashMap` (lock-free concurrent hashmap). The schema provides field names, types, and descriptions that the LLM uses to generate accurate queries.

Example schema (`schemas/users.yaml`):
```yaml
name: users
description: Application user accounts
fields:
  - name: _id
    field_type: objectId
    description: Unique user identifier
  - name: email
    field_type: string
  - name: status
    field_type: string
    description: Account status (active, suspended, deleted)
  - name: created_at
    field_type: date
```

#### Step 2: NLP Parsing (Ollama LLM)

The natural language query is sent to Ollama along with schema context. The LLM returns a structured `ParsedQuery` JSON object.

**Why Ollama is the default:**
| Factor | Ollama (Local) | Cloud (Anthropic/OpenAI) |
|--------|---------------|-------------------------|
| Cost | Free | $0.25-15 per 1M tokens |
| Latency | ~200-500ms | ~500-2000ms |
| Privacy | Data stays local | Data sent to third party |
| Availability | No internet needed | Requires internet + API key |
| JSON reliability | `FormatType::Json` enforced | Prompt-based (less reliable) |

Recommended models: `llama3.2` (3B, fast), `llama3.1:8b` (accurate), `mistral` (good JSON), `qwen2.5-coder` (structured data).

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
| Operation whitelist (find, count, distinct, aggregate) | delete, update, drop, insert |
| Field whitelist (must exist in schema) | Typos, hallucinated fields, injection via field names |
| Limit enforcement (default max: 10,000) | Unbounded queries |
| Injection detection | `$where`, `__proto__`, `constructor` patterns |

#### Step 4: Query Executor

Converts `ParsedQuery` to MongoDB operations:
- `find` → `collection.find(filter).with_options(sort, limit, projection)`
- `count` → `collection.count_documents(filter)`
- `distinct` → `collection.distinct(field, filter)`

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
| Results Table | Paginated table with sortable columns, auto-detected from document fields |
| Query Inspector | Collapsible panel showing the `ParsedQuery` JSON the LLM generated |
| CSV Download | Button triggers file download via `/api/export?format=csv` |
| JSON Download | Button triggers file download via `/api/export?format=json` |
| Schema Browser | Sidebar listing collections and their fields (from `/api/schemas`) |
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
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│     Ollama      │     │     MongoDB      │     │  Slack API      │
│  localhost:11434│     │  localhost:27017  │     │  api.slack.com  │
│                 │     │                  │     │                 │
│  llama3.2 model │     │  App database    │     │  (optional)     │
│  JSON mode      │     │  Read-only ops   │     │  Webhooks in    │
│  ~200-500ms     │     │  Pool: 2-20 conn │     │  Results out    │
│                 │     │                  │     │                 │
│  PRIMARY        │     │  REQUIRED        │     │  OPTIONAL       │
│  No API costs   │     │                  │     │                 │
└─────────────────┘     └──────────────────┘     └─────────────────┘
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
│   └── llm.rs             → LlmClient: Ollama primary, cloud fallback
│
├── domain/                → Business logic (no framework dependencies)
│   ├── models.rs          → ParsedQuery, Filter, QueryRequest/Response, ExportFormat
│   ├── parser.rs          → build_prompt(), parse_llm_response()
│   ├── executor.rs        → ParsedQuery → BSON → MongoDB → QueryResult
│   ├── safety.rs          → SafetyChecker: validate fields, ops, limits
│   └── export.rs          → documents_to_csv(), documents_to_json()
│
├── handlers/              → Actix-web request handlers
│   ├── query.rs           → POST /api/query (Web UI)
│   ├── export.rs          → GET /api/export (CSV/JSON download)
│   ├── schemas.rs         → GET /api/schemas
│   ├── slack_commands.rs  → Slack slash command + event handlers
│   ├── slack_actions.rs   → Interactive component callbacks
│   ├── ws.rs              → WebSocket /ws/live
│   └── health.rs          → GET /api/health
│
├── middleware/
│   ├── auth.rs            → Bearer token (constant-time via subtle)
│   └── slack_verify.rs    → HMAC-SHA256 signing secret
│
└── frontend/              → Embedded web UI (compile-time assembled)
    ├── mod.rs             → include_str!(OUT_DIR/index.html), serve_index()
    ├── shell_*.html       → HTML wrapper fragments
    ├── styles/*.css       → 7 CSS files (numbered for concat order)
    └── js/*.js            → 6 JS files (numbered for init order)
```

---

## Key Design Decisions

### Why actix-web over Axum?

This project follows the **same architecture as GitWorkflowDashboard**:
- Proven embedded-frontend pattern with `build.rs` + `include_str!()`
- `Arc<RwLock<T>>` shared state across actix workers
- `actix-ws` for WebSocket, `actix-governor` for rate limiting
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
ollama pull llama3.2                    # Local LLM
mongod --dbpath /tmp/mongo              # MongoDB

# Run
cargo run -- --mongo-db-name myapp
# Open http://localhost:3000
```

### Docker Compose
```yaml
services:
  bot:
    build: .
    environment:
      MONGO_URI: mongodb://mongo:27017
      MONGO_DB_NAME: myapp
      OLLAMA_URL: http://ollama:11434
    ports: ["3000:3000"]

  ollama:
    image: ollama/ollama
    volumes: ["ollama_data:/root/.ollama"]

  mongo:
    image: mongo:7
    volumes: ["mongo_data:/data/db"]
```

### Production
- Single binary (~10MB with strip + LTO)
- `debian:bookworm-slim` Docker image (~15MB total)
- Ollama as sidecar or on GPU-enabled host
- MongoDB via connection string (Atlas or self-hosted)
- Optional: `DASHBOARD_TOKEN` for auth, `SLACK_BOT_TOKEN` for Slack integration
