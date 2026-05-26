# Project Context Snapshot

> This file is a high-density summary of the project state, intended for quick context loading by LLMs.

## Overview
**Slack-Mongo Query Bot**: A NL-to-MongoDB query engine with a Web UI and Slack integration.

## Tech Stack
- **Backend**: Rust, `actix-web 4`, `mongodb 3.2`, `tokio`
- **Frontend**: Vanilla JS, CSS, HTML (embedded in binary via `build.rs`)
- **LLM**: Ollama (default), LM Studio, Anthropic Claude, OpenAI GPT
- **Database**: MongoDB

## Key Features
- **NLP to MongoDB**: Converts natural language to structured `ParsedQuery`.
- **Full Aggregation Support**: `$group`, `$lookup`, `$unwind`, `$bucket`, `$facet`.
- **Safety Engine**: Whitelisting, injection detection, and complexity limits.
- **Query Advisor**: Real-time performance and best-practice feedback.
- **DPO Feedback Loop**: Captures user corrections to improve the LLM parser.
- **Runtime Schema Discovery**: Auto-introspects MongoDB collections.

## Current Status
- **Build Status**: ✅ All 124 unit tests pass. Compilation errors resolved.
- **Recent Work**: Fixed compilation errors in the User Feedback Loop implementation.

## How to Run
- **Local**: `cargo run -- --mongo-db-name movies`
- **Docker**: `./run.sh --docker`

## Project Structure
- `src/domain/`: Core logic (parser, executor, safety, advisor, models)
- `src/infra/`: External clients (mongo, llm, slack)
- `src/handlers/`: HTTP endpoints
- `src/frontend/`: Embedded UI source
- `schemas/`: YAML schema definitions
