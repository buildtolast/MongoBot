# SlackMongoBot — CLAUDE.md

> **Active roadmap:** see [`NextSteps.md`](NextSteps.md) for the approved v0.5.0 / v0.5.1 / v0.5.2 plan — feedback capture, in-context few-shot learning, and LoRA fine-tuning pipeline.

## Project Overview
NL-to-MongoDB query bot with Web UI and Slack integration. Actix-web backend with embedded vanilla JS/HTML frontend. 5 LLM providers: Ollama (default), LM Studio, llama.cpp, Anthropic Claude, OpenAI GPT — switchable at runtime from the UI. NL parsing via direct HTTP calls (reqwest). Single binary deployment. Default database: `movies` (14+ collections auto-discovered via runtime introspection). Full aggregate pipeline support ($group, $lookup, $unwind, $bucket, $facet). Raw pipeline/filter passthrough for native MongoDB queries. 31 source files, 19 frontend files.

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
- LLM: 5 providers — Ollama (default), LM Studio (OpenAI-compatible), llama.cpp (`llama-server`, OpenAI-compatible), Anthropic Claude, OpenAI GPT. LM Studio, llama.cpp, and OpenAI share `call_openai_compatible()` implementation
- Safety: field whitelist from schema, operation whitelist, injection detection, $expr safety validation, accumulator whitelist
- Query Advisor: `domain/query_advisor.rs` — 25 validation rules (unbounded, missing index, $ne/$nin, regex, aggregate, lookup, $or, raw passthrough, destructive ops, etc.)
- Aggregate pipeline: `ParsedQuery` has typed fields for `group_by`, `lookup`, `unwind`, `bucket`, `facet`. Executor builds full pipeline in order: `$match` → `$lookup` → `$unwind` → `$group` → `$bucket` → `$facet` → `$sort` → `$limit` → `$project`
- Filter operators: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`, `nin`, `contains`, `regex`, `exists`, `all`, `size`, `type`. Plus `$or` (via `or_filters`), `$text` (via `text_search`), `$expr` (via `expr`)
- Preview mode: approval toggle in UI (defaults to ON); `POST /api/query` with `preview: true` returns parsed query without executing; user can edit the parsed JSON before confirming; `POST /api/execute` executes the edited query for both reads and writes
- Settings panel: gear icon opens modal showing all env config with secrets redacted (via `GET /api/config`)
- Date coercion: `coerce_value_for_field()` and `try_parse_datetime()` in executor — ISO strings and year numbers auto-converted to BSON DateTime
- Schema-aware `$elemMatch`: array element sub-field filters auto-wrapped in `$elemMatch`, nested object fields use dot notation. Schema threaded through `execute()` -> `build_filter()` -> `build_and_filter()`
- Raw passthrough: `raw_pipeline` (native aggregate stages) and `raw_filter` (native filter documents) bypass structured intermediary for complex queries
- LLM transparency: `raw_llm_response` field on `QueryResponse` captures raw LLM output; collapsible UI panel for comparison
- Filter merging: multiple conditions on the same field merged into single BSON sub-document (range queries)
- CRUD: insert/update with confirmation dialog + backup download before execution
- Auth: optional Bearer token, constant-time comparison via `subtle::ConstantTimeEq`
- Schema loading: auto-discovered from MongoDB at runtime (runtime introspection with array element sampling); refreshable via `POST /api/schemas/refresh`
- Model + provider selection: `GET /api/providers` lists 5 providers, `GET /api/models?provider=<name>` lists models for a specific provider; queries can specify `model` and `provider` override fields

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
- No `ollama-rs` — all 5 LLM providers use direct HTTP via reqwest

## Endpoints
- `GET /` — embedded frontend
- `GET /api/health` — health check (MongoDB + LLM)
- `GET /api/models` — list available models (accepts `?provider=<name>` for any of the 5 providers)
- `GET /api/providers` — list all 5 LLM providers with the configured default marked
- `GET /api/version` — build metadata
- `GET /api/config` — current configuration with secrets redacted (LLM, MongoDB, server, safety, Slack sections)
- `POST /api/query` — NL query (supports `preview: true` for approval mode + editable query)
- `POST /api/execute` — execute a confirmed query (reads + writes, accepts edited ParsedQuery)
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
cargo test                     # all 121 unit + 40 integration tests
cargo test --lib               # unit tests only (fast, no server needed)
cargo test config              # filter by name
cargo test -- --nocapture      # show stdout
```

## Docker
Multi-stage build — `rust:1.88-bookworm` compile stage, `debian:bookworm-slim` runtime.

```bash
docker build -t slack-mongo-bot .
docker compose up              # bot + mongo (Ollama runs on host)
```
