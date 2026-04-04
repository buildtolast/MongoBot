# SlackMongoBot — CLAUDE.md

## Project Overview
NL-to-MongoDB query bot with Web UI and Slack integration. Actix-web backend with embedded vanilla JS/HTML frontend. Ollama (local LLM) for NL parsing via direct HTTP calls (reqwest). Single binary deployment. Default database: `movies` (14+ collections auto-discovered via runtime introspection). 72 unit tests.

## Build & Run
```bash
cargo build                        # debug
cargo build --release              # release
cargo run -- --mongo-db-name movies # run with required args
cargo test                         # tests
```

Environment variables mirror CLI flags (MONGO_URI, MONGO_DB_NAME, OLLAMA_MODEL, etc.)

## Project Structure
```
src/
  main.rs           — server bootstrap, middleware, route registration
  lib.rs            — module exports
  config.rs         — CLI args (clap), env var merging
  error.rs          — AppError with sanitized HTTP responses
  domain/           — business logic: models, parser, executor, safety, query_advisor, export
  infra/            — infrastructure (MongoDB client, LLM client, Slack client)
  handlers/         — HTTP handlers: query, execute, export, schemas, slack, ws, health
  middleware/       — auth (Bearer token), Slack webhook verification
  frontend/
    mod.rs          — includes compiled index.html via include_str!()
    shell_*.html    — HTML wrapper fragments
    styles/         — 7 CSS files (numbered for concat order)
    js/             — 6 JS files (numbered for init order)
docs/               — mongodb_query_reference.md, query_audit_report.md
tests/              — integration tests
build.rs            — assembles frontend into $OUT_DIR/index.html
```

## Key Patterns
- Shared state: `web::Data<AppState>` across actix workers
- Frontend: `build.rs` assembles shell HTML + CSS + JS into `$OUT_DIR/index.html`, embedded via `include_str!()`
- Error handling: `thiserror` + sanitized HTTP responses (internal errors logged, safe messages sent to client)
- LLM: Ollama primary via direct HTTP calls (reqwest), cloud fallback stubs
- Safety: field whitelist from schema, operation whitelist, injection detection
- Query Advisor: `domain/query_advisor.rs` — 14 validation rules (unbounded, missing index, $ne/$nin, regex, etc.)
- Preview mode: approval toggle in UI; `POST /api/query` with `preview: true` returns parsed query without executing; `POST /api/execute` confirms
- Date coercion: `coerce_value_for_field()` and `try_parse_datetime()` in executor — ISO strings and year numbers auto-converted to BSON DateTime
- `$elemMatch`: array element sub-field filters auto-wrapped in `$elemMatch` by executor
- Filter merging: multiple conditions on the same field merged into single BSON sub-document (range queries)
- CRUD: insert/update with confirmation dialog + backup download before execution
- Auth: optional Bearer token, constant-time comparison via `subtle::ConstantTimeEq`
- Schema loading: auto-discovered from MongoDB at runtime (runtime introspection with array element sampling); refreshable via `POST /api/schemas/refresh`
- Model selection: `GET /api/models` lists available Ollama models; queries can specify `model` override

## Dependencies (key)
- `actix-web 4`, `actix-ws 0.3` — HTTP server + WebSocket
- `actix-cors 0.7` — CORS preflight handling
- `mongodb 3.2` — MongoDB driver (tokio runtime)
- `reqwest 0.12` with `rustls-tls` — HTTP client (also used for direct Ollama API calls)
- `clap 4` (derive) — CLI argument parsing
- `tracing` + `tracing-actix-web` — structured logging
- `hmac`, `sha2`, `subtle` — webhook verification + auth
- `serde_yaml_ng 0.10` — schema YAML parsing
- `dotenvy 0.15` — .env file loading
- `chrono 0.4` — date parsing and coercion
- `dashmap 6.1` — lock-free concurrent schema cache
- No `ollama-rs` — removed in v0.1.1, replaced with direct HTTP

## Endpoints
- `GET /` — embedded frontend
- `GET /api/health` — health check (MongoDB + Ollama)
- `GET /api/models` — list available Ollama models
- `GET /api/version` — build metadata
- `POST /api/query` — NL query (supports `preview: true` for approval mode)
- `POST /api/execute` — execute a confirmed query (CRUD flow)
- `GET /api/export` — CSV/JSON file download
- `GET /api/schemas` — list collection schemas
- `POST /api/schemas/refresh` — re-introspect MongoDB collections
- `POST /api/slack/commands` — Slack slash command
- `POST /api/slack/events` — Slack event handler
- `POST /api/slack/actions` — Slack interactive actions
- `WS /ws/live` — WebSocket for real-time updates

## Security
- Auth middleware guards `/api/*` routes (optional Bearer token)
- Slack webhook HMAC-SHA256 verification
- MongoDB query safety: operation whitelist, field whitelist, injection detection
- CRUD operations require confirmation before execution
- MAX_RESULT_ROWS caps query output size
- Sensitive fields (tokens) redacted from logs

## Testing
```bash
cargo test                     # all 72 tests
cargo test config              # filter by name
cargo test -- --nocapture      # show stdout
```

## Docker
Multi-stage build — `rust:1.88-bookworm` compile stage, `debian:bookworm-slim` runtime.

```bash
docker build -t slack-mongo-bot .
docker compose up              # bot + mongo (Ollama runs on host)
```
