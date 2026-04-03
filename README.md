# Slack-Mongo Query Bot

NL-to-MongoDB query bot with Web UI and Slack integration. Ask questions in plain English, get structured MongoDB query results.

<!-- screenshot -->

## Quick Start

**Prerequisites:**
- Rust 1.75+
- MongoDB (running on localhost:27017 or remote)
- [Ollama](https://ollama.com) running on the host machine with a model pulled (e.g. `ollama pull qwen3-coder:30b`)

**Install and run:**

```bash
git clone https://github.com/your-org/slack-mongo-bot.git
cd slack-mongo-bot
cp .env.example .env        # edit as needed
cargo run -- --mongo-db-name movies
# Open http://localhost:3000
```

**Sample database:** The project ships with a `movies` sample database containing 3 collections: `movies` (15 documents), `actors` (10 documents), and `reviews` (15 documents), all with indexes for efficient querying.

## Features

- Natural language query bar -- type questions like "find active users from last week"
- Results table with sortable columns and pagination
- CSV and JSON export (file download)
- Schema browser sidebar showing collections and fields
- Slack `/mongo` slash command and `@query-bot` mentions
- Ollama local LLM via direct HTTP API (no API costs, data stays local)
- Safety checker: operation whitelist, field whitelist, injection detection
- Dark theme UI with responsive layout

## Configuration

All settings can be set via environment variables or CLI flags. Copy `.env.example` to `.env` to get started.

| Variable | Default | Description |
|---|---|---|
| `MONGO_URI` | `mongodb://localhost:27017` | MongoDB connection string |
| `MONGO_DB_NAME` | **(required)** | Database to query |
| `OLLAMA_URL` | `http://localhost:11434` | Ollama API endpoint |
| `OLLAMA_MODEL` | `qwen3-coder:30b` | Ollama model for NL parsing |
| `LLM_PROVIDER` | `ollama` | LLM backend (`ollama` or `anthropic`) |
| `SLACK_BOT_TOKEN` | -- | Slack bot token (optional) |
| `SLACK_SIGNING_SECRET` | -- | Slack webhook signing secret (optional) |
| `DASHBOARD_TOKEN` | -- | Bearer token to protect `/api/*` routes (optional) |
| `PORT` | `3000` | HTTP server port |
| `MAX_RESULT_ROWS` | `10000` | Maximum documents returned per query |

Slack configuration is optional. The web UI works standalone without it.

## Architecture

The server is built on actix-web 4 (with actix-cors for CORS preflight handling) and an embedded vanilla JS/HTML frontend assembled at compile time via `build.rs` (single binary, no Node.js). Natural language queries are parsed by Ollama via direct HTTP calls using reqwest (no ollama-rs crate), validated by a safety checker, and executed against MongoDB. Both the Web UI and Slack bot share the same query processing pipeline.

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
  config.rs         -- CLI args (clap) + env var merging
  error.rs          -- error types with sanitized HTTP responses
  domain/           -- business logic: models, NL parser, query executor, safety checker, export
  infra/            -- external clients: MongoDB, Ollama LLM, Slack API
  handlers/         -- HTTP handlers: query, export, schemas, slack, websocket, health
  middleware/       -- auth (Bearer token) + Slack webhook verification
  frontend/         -- embedded web UI (shell HTML + CSS + JS, assembled by build.rs)
schemas/            -- YAML collection schema definitions
tests/              -- integration tests
```

## License

MIT
