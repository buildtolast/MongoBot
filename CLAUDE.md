# SlackMongoBot — CLAUDE.md

## Project Overview
NL-to-MongoDB query bot with Web UI and Slack integration. Actix-web backend with embedded vanilla JS/HTML frontend. Ollama (local LLM) for NL parsing. Single binary deployment.

## Build & Run
```bash
cargo build                        # debug
cargo build --release              # release
cargo run -- --mongo-db-name myapp # run with required args
cargo test                         # tests
```

Environment variables mirror CLI flags (MONGO_URI, MONGO_DB_NAME, OLLAMA_MODEL, etc.)

## Project Structure
```
src/
  main.rs           — server bootstrap, middleware, route registration
  lib.rs            — module exports
  config.rs         — CLI args (clap), env var merging
  error.rs          — DashboardError with sanitized HTTP responses
  domain/           — business logic and data types
  infra/            — infrastructure (MongoDB client, LLM client)
  handlers/         — HTTP request handlers
  middleware/        — auth, CSRF, rate limiting
  frontend/
    mod.rs          — includes compiled index.html via include_str!()
    shell_*.html    — HTML wrapper fragments
    styles/         — CSS files (numbered for concat order)
    js/             — JS files (numbered for init order)
schemas/            — YAML collection schema definitions
tests/              — integration tests
build.rs            — assembles frontend into $OUT_DIR/index.html
```

## Key Patterns
- Shared state: `web::Data<AppState>` across actix workers
- Frontend: `build.rs` assembles shell HTML + CSS + JS into `$OUT_DIR/index.html`, embedded via `include_str!()`
- Error handling: `thiserror` + sanitized HTTP responses (internal errors logged, safe messages sent to client)
- LLM: Ollama primary via `ollama-rs`, cloud fallback stubs
- Safety: field whitelist from schema YAML, operation whitelist, injection detection
- Auth: optional Bearer token, constant-time comparison via `subtle::ConstantTimeEq`
- Schema loading: YAML files in `schemas/` define collection fields and types for LLM context

## Dependencies (key)
- `actix-web 4`, `actix-ws 0.3` — HTTP server + WebSocket
- `mongodb 3.2` — MongoDB driver (tokio runtime)
- `ollama-rs 0.2` — Local LLM integration
- `reqwest 0.12` with `rustls-tls` (no OpenSSL)
- `clap 4` (derive) — CLI argument parsing
- `tracing` + `tracing-actix-web` — structured logging
- `hmac`, `sha2`, `subtle` — webhook verification + auth
- `serde_yaml_ng 0.10` — schema YAML parsing
- `dotenvy 0.15` — .env file loading

## Security
- Auth middleware guards `/api/*` routes (optional Bearer token)
- Slack webhook HMAC-SHA256 verification
- MongoDB query safety: operation whitelist, field whitelist, injection detection
- MAX_RESULT_ROWS caps query output size
- Sensitive fields (tokens) redacted from logs

## Testing
```bash
cargo test                     # all tests
cargo test config              # filter by name
cargo test -- --nocapture      # show stdout
```

## Docker
Multi-stage build — `rust:1.88-bookworm` compile stage, `debian:bookworm-slim` runtime.

```bash
docker build -t slack-mongo-bot .
docker compose up              # bot + ollama + mongo
```
