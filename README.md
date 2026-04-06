# Slack-Mongo Query Bot

NL-to-MongoDB query bot with Web UI and Slack integration. Ask questions in plain English, get structured MongoDB query results. Full aggregate pipeline support, 4 LLM providers, 101 unit tests, 30+ commits of iteration.

<!-- screenshot -->

## Quick Start

**Prerequisites:**
- Rust 1.75+
- MongoDB (running on localhost:27017 or remote)
- One of: [Ollama](https://ollama.com) (default), [LM Studio](https://lmstudio.ai), Anthropic API key, or OpenAI API key

**Install and run:**

```bash
git clone https://github.com/your-org/slack-mongo-bot.git
cd slack-mongo-bot
cp .env.example .env        # edit as needed
cargo run -- --mongo-db-name movies
# Open http://localhost:3000
```

**Database:** 14+ collections auto-discovered from MongoDB at runtime via schema introspection. The `movies` sample database ships with 3 seed collections (`movies`, `actors`, `reviews`), but the bot discovers all collections in the configured database automatically.

## Features

**Query & Execution:**
- Natural language query bar -- type questions like "find active users from last week"
- Full aggregate pipeline: `$group`, `$lookup`, `$unwind`, `$bucket`, `$facet` stages
- Filter operators: `$or`, `$exists`, `$all`, `$size`, `$type`, `$text`, `$expr` (cross-field comparisons)
- Query Advisor with 21 validation rules (unbounded queries, missing indexes, aggregate warnings, etc.)
- Approval toggle -- preview parsed queries before execution (preview mode)
- Editable query: modify the parsed JSON before execution in approval mode (edits applied to reads + writes)
- CRUD support: insert and update operations with confirmation dialog + backup download
- Abort button for canceling in-progress queries
- MongoDB query viewer panel showing actual shell commands (includes pipeline stages)
- Index analysis showing which indexes are used per query
- Date auto-conversion (ISO strings, year numbers automatically converted to BSON DateTime)
- `$elemMatch` for array element queries (auto-grouped in executor)
- Same-field filter merging for range queries (e.g. price > 10 AND price < 100)

**LLM Providers (4):**
- **Ollama** (default) -- local LLM, no API costs, data stays local, `format: "json"` enforced
- **LM Studio** -- local, OpenAI-compatible API at `localhost:1234`
- **Anthropic Claude** -- cloud, requires `CLOUD_LLM_API_KEY`
- **OpenAI GPT** -- cloud, requires `CLOUD_LLM_API_KEY`

LM Studio and OpenAI share a `call_openai_compatible()` implementation. Set `LLM_PROVIDER` to switch providers.

**UI & Export:**
- Results table with sortable columns and pagination
- CSV and JSON export (file download)
- Schema browser sidebar showing collections and fields
- Schema refresh button (detect new collections at runtime without restart)
- Live model selection (switch models without restart via `/api/models`)
- Settings panel (gear icon) -- shows all env config with secrets redacted via `/api/config`
- Dark theme UI with responsive layout

**Integration:**
- Slack `/mongo` slash command and `@query-bot` mentions
- 4 LLM providers: Ollama, LM Studio, Anthropic Claude, OpenAI GPT
- Safety checker: operation whitelist, field whitelist, injection detection

**Documentation:**
- `docs/mongodb_query_reference.md` -- comprehensive MongoDB query syntax reference
- `docs/query_audit_report.md` -- audit of implementation vs. reference (gaps and compliance)

## Configuration

All settings can be set via environment variables or CLI flags. Copy `.env.example` to `.env` to get started.

| Variable | Default | Description |
|---|---|---|
| `MONGO_URI` | `mongodb://localhost:27017` | MongoDB connection string |
| `MONGO_DB_NAME` | **(required)** | Database to query |
| `LLM_PROVIDER` | `ollama` | LLM backend (`ollama`, `lmstudio`, `anthropic`, `openai`) |
| `OLLAMA_URL` | `http://localhost:11434` | Ollama API endpoint |
| `OLLAMA_MODEL` | `qwen3-coder:30b` | Ollama model for NL parsing (recommended) |
| `LMSTUDIO_URL` | `http://localhost:1234` | LM Studio API endpoint (OpenAI-compatible) |
| `LMSTUDIO_MODEL` | -- | LM Studio model name (auto-detected if not set) |
| `CLOUD_LLM_API_KEY` | -- | API key for Anthropic or OpenAI |
| `CLOUD_LLM_MODEL` | -- | Model name for cloud providers |
| `ANTHROPIC_API_URL` | `https://api.anthropic.com` | Anthropic API URL (override for proxies) |
| `OPENAI_API_URL` | `https://api.openai.com` | OpenAI API URL (override for proxies/Azure) |
| `SLACK_BOT_TOKEN` | -- | Slack bot token (optional) |
| `SLACK_SIGNING_SECRET` | -- | Slack webhook signing secret (optional) |
| `DASHBOARD_TOKEN` | -- | Bearer token to protect `/api/*` routes (optional) |
| `HOST` | `0.0.0.0` | Server bind host |
| `PORT` | `3000` | HTTP server port |
| `MAX_RESULT_ROWS` | `10000` | Maximum documents returned per query |
| `ALLOWED_OPERATIONS` | `find,count,distinct,aggregate,insert,update` | Comma-separated operation whitelist (add `delete` to enable) |

Slack configuration is optional. The web UI works standalone without it.

## Architecture

The server is built on actix-web 4 (with actix-cors for CORS preflight handling) and an embedded vanilla JS/HTML frontend assembled at compile time via `build.rs` (single binary, no Node.js). Natural language queries are parsed by one of 4 LLM providers (Ollama default, LM Studio, Anthropic Claude, OpenAI GPT), validated by a safety checker and query advisor (21 rules), and executed against MongoDB. Full aggregate pipeline support (`$group`, `$lookup`, `$unwind`, `$bucket`, `$facet`) with structured type-safe models. Both the Web UI and Slack bot share the same query processing pipeline. Default model: `qwen3-coder:30b` (recommended), `qwen3:8b` (fast alternative).

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed diagrams, data flows, and design decisions.

## Docker

```bash
docker compose up
```

The compose stack includes the bot and MongoDB 7. **Ollama runs on the host machine**, not in Docker. The bot connects to Ollama via `host.docker.internal:11434` (macOS/Windows Docker Desktop). Make sure Ollama is running on your host before starting the stack.

```bash
# On the host machine (not in Docker):
ollama pull qwen3-coder:30b
ollama serve

# Then start the Docker stack:
docker compose up
```

## Project Structure

```
src/
  main.rs           -- server bootstrap, route registration
  lib.rs            -- module exports
  config.rs         -- CLI args (clap) + env var merging, LlmProvider enum
  error.rs          -- error types with sanitized HTTP responses
  domain/           -- business logic: models, parser, executor, safety, query_advisor, export
  infra/            -- external clients: MongoDB, LLM (4 providers), Slack API
  handlers/         -- HTTP handlers: query, execute, export, schemas, slack, websocket, health, config
  middleware/       -- auth (Bearer token) + Slack webhook verification
  frontend/         -- embedded web UI (shell HTML + CSS + JS, assembled by build.rs)
docs/               -- mongodb_query_reference.md, query_audit_report.md
tests/              -- integration tests
```

## Changelog

### v0.4.0: Full Aggregate Pipeline + Missing Operators (2026-04-05)

- [x] **Aggregate `$group` stage** — GROUP BY with accumulators (`$sum`, `$avg`, `$min`, `$max`, `$first`, `$last`, `$push`, `$addToSet`, `$count`)
- [x] **Aggregate `$lookup` stage** — cross-collection joins with foreign key matching
- [x] **Aggregate `$unwind` stage** — flatten array fields into separate documents
- [x] **Aggregate `$bucket` stage** — range grouping with configurable boundaries
- [x] **Aggregate `$facet` stage** — parallel sub-pipelines (raw JSON passthrough)
- [x] **Full pipeline builder** — assembles stages in correct order: `$match` → `$lookup` → `$unwind` → `$group` → `$bucket` → `$facet` → `$sort` → `$limit` → `$project`
- [x] **`$or` operator** — logical OR conditions across multiple filter branches
- [x] **`$exists` operator** — check if a field is present in documents
- [x] **`$all` operator** — match arrays containing all specified values
- [x] **`$size` operator** — match arrays with exact length
- [x] **`$type` operator** — filter by BSON data type
- [x] **`$text` operator** — full-text search via `text_search` field
- [x] **`$expr` operator** — safe cross-field comparisons (e.g. `total > budget`)
- [x] **Accumulator whitelist** — safety checker validates `$group` accumulator operators
- [x] **`$expr` safety validation** — only allows simple comparisons, blocks `$function`/`$accumulator`
- [x] **5 new query advisor rules** — `AGGREGATE_NO_MATCH`, `LOOKUP_PERF`, `UNWIND_ARRAY`, `OR_QUERY`, `GROUP_HIGH_CARDINALITY`
- [x] **LLM prompt expanded** — all new operators, pipeline stages, `update_many`/`delete_many` guidance
- [x] **Resilient parser updated** — extracts `group_by`, `lookup`, `unwind`, `facet`, `bucket`, `or_filters`, `text_search`, `expr` from LLM output
- [x] **Shell command display** — `build_mongo_shell_command()` renders all new pipeline stages
- [x] **Fix: editable query now works for reads** — approved read queries use the edited parsed JSON via `/api/execute` instead of re-querying the LLM
- [x] **`/api/execute` returns full diagnostics** — index analysis + query advice for all operations, not just writes
- [x] **29 new unit tests** (72 → 101 total), all 37 integration tests pass
- [x] **Backward compatible** — all new `ParsedQuery` fields use `#[serde(default)]`, existing queries unaffected

### v0.3.0: Multi-Provider LLM + UI Polish (2026-04-04)

- [x] 4 LLM providers: Ollama (default), LM Studio, Anthropic Claude, OpenAI GPT
- [x] Shared `call_openai_compatible()` for OpenAI/LM Studio (same API format)
- [x] Settings panel: gear icon, `/api/config` endpoint (secrets redacted), modal with grouped config
- [x] Editable query: contentEditable parsed JSON + mongo command in approval mode
- [x] 7 themes: Dark (default), Dim, Light, Nord, Midnight, Warm, Solarized
- [x] Theme selector dropdown in top bar, persists to localStorage
- [x] UI polish: gradient logo, colored export buttons, accent gear icon, example query pills

### v0.2.0: Query Intelligence + CRUD (2026-04-03/04)

- [x] Query Advisor: 14 validation rules (unbounded, limit, projection, regex, $ne/$nin, case, index, write safety)
- [x] Approval toggle: preview mode with approve/reject flow
- [x] CRUD: insert + update with confirmation + mandatory backup download
- [x] `$elemMatch` auto-grouping for multi-condition array queries
- [x] Same-field filter merging for range queries
- [x] Date auto-conversion: ISO strings + year numbers → BSON DateTime
- [x] Runtime schema discovery: introspects all MongoDB collections
- [x] Live model selection: `/api/models` + per-query model override
- [x] 37 Rust integration tests (configurable model via `TEST_MODEL`)
- [x] MongoDB query reference docs (35KB) + audit report

### v0.1.1: Bug Fixes + Movies DB (2026-04-02/03)

- [x] Replaced ollama-rs with direct HTTP via reqwest
- [x] Added actix-cors for CORS preflight handling
- [x] Fixed frontend field mismatches, spinner, CORS preflight
- [x] Switched to movies DB with sample data

### v0.1.0: Initial Implementation (2026-04-01)

- [x] Full project scaffold: actix-web 4, 43 source files, 7 modules
- [x] Ollama LLM integration, MongoDB execution, safety checker
- [x] Dark-themed embedded Web UI, CSV/JSON export, Slack handlers
- [x] run.sh, Docker, 40 unit tests

## License

MIT
