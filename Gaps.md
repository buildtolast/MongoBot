# Gaps — Project Review (2026-04-20)

Cross-checked the codebase against `MEMORY.md`, `ARCHITECTURE.md`, `CLAUDE.md`, and `NextSteps.md`. Findings below, with an actionable step-by-step checklist at the end.

---

## Summary

- **7,110 lines** of Rust across 28 files, **124 unit + 40 integration tests**, `cargo clippy` mostly clean (10 style-level warnings).
- **Core product is implemented and working** per CLAUDE.md description.
- **Documentation has drifted** — `ARCHITECTURE.md` describes an earlier (pre-v0.4 and pre-llama.cpp) state.
- **NextSteps.md (v0.5.x) is 0% implemented** — none of Phase 1/2/3 has landed.

---

## 1. Documentation Drift — `ARCHITECTURE.md` is stale

| Doc claim | Reality | Where |
|---|---|---|
| "4 LLM providers" (stated throughout) | **5 providers** — `LlamaCpp` added in commit `41b4ea1` | `src/config.rs:14`, `src/infra/llm.rs:75` |
| "Query Advisor: 14 rules" | **25 rules / 21 check functions** | `src/domain/query_advisor.rs` |
| Middleware stack lists `actix-governor` for rate limiting | **Not a dependency** — not in `Cargo.toml`, not imported | `Cargo.toml` |
| Lists `GET /api/models` only | `/api/providers` also exists and is route-registered | `src/main.rs` |
| Default DB "~15 movies, 10 actors, 15 reviews" | 14+ collections, 1746 transactions, Atlas sample datasets present | runtime |
| Module map omits `/api/feedback/*` plan | Planned in NextSteps — still absent | `src/handlers/` |

## 2. NextSteps.md — Zero Implementation

The v0.5.0 / v0.5.1 / v0.5.2 plan was approved but not executed. Evidence:

- No `src/handlers/feedback.rs` file
- No `QueryFeedback` / `FeedbackRequest` structs in `src/domain/models.rs`
- No `/api/feedback/*` routes in `src/main.rs`
- No `training/` directory at project root
- No `embed_text()` in `src/infra/llm.rs`
- No feedback UI buttons in `src/frontend/`

All three phases (feedback capture, few-shot injection, LoRA pipeline) are **pending**.

## 3. Slack Integration — Partial

- `src/handlers/slack_actions.rs` is a **12-line stub** that just returns 200 OK. ARCHITECTURE.md promises "Modals / Interactive component callbacks" — not implemented.
- 5 fields in `src/handlers/slack_commands.rs:12-20` are marked `#[allow(dead_code)]` — parsing struct with unused branches.
- No Slack integration tests.

## 4. WebSocket — Declared but Inert

`src/handlers/ws.rs`:
- `_state: web::Data<AppState>` is unused (underscore-prefixed, ws.rs:12)
- No broadcast channel from query pipeline to WS clients
- ARCHITECTURE.md claims "Real-time query progress updates" — nothing publishes progress events

Either wire it up or remove the feature from docs.

## 5. Known Dead Code from Memory

- `SafetyError::UnknownCollection` is defined but never used. Queries against non-existent collections silently return 0 results instead of erroring. **Still unaddressed.**

## 6. Test Coverage Gaps

| Aspect | State |
|---|---|
| Unit tests | 124 (broad coverage — good) |
| Integration tests | 1 file (`tests/integration_tests.rs`, 479 lines, 40 tests) — requires live Ollama + MongoDB |
| Mocking | None — no `mockall`, no trait-based seams for `LlmClient` / `MongoPool` |
| Property-based | None |
| Slack handler tests | None |
| WS handler tests | None |
| Middleware tests | None (auth, slack_verify) |
| Handler-layer tests | Absent — handler logic only covered via live-service integration tests |

**Impact:** Any refactor of `LlmClient` or `MongoPool` has no mocked coverage. Integration suite requires ~1s/test against a local LLM.

## 7. Error Handling / Panic Surface

- **33 `.unwrap()` calls in production code** (97 total – 64 are inside `#[cfg(test)]` in `executor.rs`):
  - 23 in `src/infra/llm.rs`
  - 4 in `src/domain/export.rs`
  - 3 in `src/domain/models.rs`
  - 1 each in `middleware/slack_verify.rs`, `handlers/query.rs`, `domain/safety.rs`
- Global rule: "never `unwrap()` in production code". Audit and convert to `?` with context via `anyhow::Context`.

## 8. Clippy Warnings (10 total, trivial)

- 6× `map_or(true, ...)` → `is_none_or(...)`
- 2× `impl` can be derived (manual `Default` etc.)
- 1× redundant closure in `src/handlers/query.rs:260`
- 1× `assert_eq!(…, true)` → `assert!` in `src/domain/executor.rs:1050`

## 9. Dependency Hygiene

- No `cargo audit` / `cargo deny` in CI or run.sh
- No `rust-toolchain.toml` pinning
- No MSRV declared in `Cargo.toml` (Dockerfile pins Rust 1.88)
- Both `anyhow` and `thiserror` present — correct pattern, but confirm `anyhow` doesn't leak into domain/library modules

## 10. Observability Gaps

- No metrics endpoint (`/metrics`, no `prometheus` crate)
- No correlation/request IDs surfaced to clients (tracing provides trace IDs but they're server-side only)
- No structured error telemetry beyond `tracing::error!`

## Positive Highlights

- Clean clippy (only style-level warnings), **no `unsafe` blocks**, no TODO/FIXME markers.
- Strong unit-test density (124 tests for 6K LOC).
- Clean separation of `domain/` / `infra/` / `handlers/` — no framework leakage into domain logic.
- Shared `call_openai_compatible()` across LM Studio / llama.cpp / OpenAI — good deduplication.
- Resilient LLM JSON parser handles multiple output formats.
- Constant-time auth via `subtle::ConstantTimeEq`.

---

# Step-by-Step Checklist

Ordered by effort/impact. Check off as you go.

## Phase A — Quick wins (1–2 hours total)

- [ ] **A1. Fix clippy warnings** (10 warnings, trivial)
  - [ ] Replace `map_or(true, ...)` with `is_none_or(...)` in `src/domain/safety.rs:152,153` and 4 other sites
  - [ ] Convert manual `Default` impls to `#[derive(Default)]` (2 sites)
  - [ ] Remove redundant closure in `src/handlers/query.rs:260` → pass `bson_doc_to_json` directly
  - [ ] Change `assert_eq!(…, true)` → `assert!(…)` in `src/domain/executor.rs:1050`
  - [ ] Verify: `cargo clippy --all-targets -- -D warnings` is clean
  - [ ] Commit: `chore: fix clippy style warnings`

- [ ] **A2. Refresh `ARCHITECTURE.md`** to match reality
  - [ ] "4 LLM providers" → "5 providers" (add LlamaCpp row to tables/diagrams)
  - [ ] "14 rules" → "25 rules" for Query Advisor
  - [ ] Remove `actix-governor` from middleware stack (it's not a dependency)
  - [ ] Add `GET /api/providers` to the endpoint table
  - [ ] Remove or qualify "Real-time query progress updates" claim for WebSocket
  - [ ] Update seed data counts (14+ collections, not 3)
  - [ ] Commit: `docs: sync ARCHITECTURE.md with current implementation`

- [ ] **A3. Fix `SafetyError::UnknownCollection` dead-code gap**
  - [ ] In `src/domain/safety.rs`, return `SafetyError::UnknownCollection` when `parsed.collection` is not in schema keys
  - [ ] Add unit test: `test_rejects_unknown_collection`
  - [ ] Add integration test: queries targeting `bogus_collection` return 400, not 0 results
  - [ ] Update `MEMORY.md` to remove the "Known gap" note
  - [ ] Commit: `fix(safety): reject queries targeting unknown collections`

## Phase B — Tech debt cleanup (half-day)

- [ ] **B1. Audit 33 production `unwrap()` calls**
  - [ ] Run: `grep -n "\.unwrap()" src/**/*.rs` (outside `#[cfg(test)]`)
  - [ ] `src/infra/llm.rs` — 23 sites: mostly JSON extraction. Replace with `?` + `anyhow::Context`
  - [ ] `src/domain/export.rs` — 4 sites: CSV serialization; convert to `ExportError`
  - [ ] `src/domain/models.rs` — 3 sites
  - [ ] `src/middleware/slack_verify.rs` — 1 site (header parsing)
  - [ ] `src/handlers/query.rs` — 1 site
  - [ ] `src/domain/safety.rs` — 1 site
  - [ ] Verify: `cargo test` still green
  - [ ] Commit: `refactor: replace production unwrap() with Result propagation`

- [ ] **B2. Decide on WebSocket — wire up or remove**
  - Option A (wire up):
    - [ ] Add `tokio::sync::broadcast::channel` to `AppState`
    - [ ] Publish progress events from `handlers/query.rs` (schema loaded, LLM parsed, safety checked, executing, done)
    - [ ] Subscribe in `handlers/ws.rs` and forward to client
    - [ ] Add integration test
  - Option B (remove):
    - [ ] Delete `src/handlers/ws.rs` and route registration
    - [ ] Delete `actix-ws` dependency
    - [ ] Delete `src/frontend/js/03_websocket.js`
    - [ ] Remove from ARCHITECTURE.md, README.md, CLAUDE.md
  - [ ] Commit: `feat(ws): broadcast query progress` OR `chore: remove inert WebSocket handler`

- [ ] **B3. Finish or remove Slack modal stub**
  - [ ] If keeping: implement interactive block actions (approve/reject buttons)
  - [ ] If removing: delete `slack_actions.rs` + route + ARCHITECTURE.md entry
  - [ ] Remove the five `#[allow(dead_code)]` attributes in `slack_commands.rs:12-20` — either use or delete
  - [ ] Commit: `fix(slack): complete modal handler` OR `chore: remove unused Slack action stub`

## Phase C — Testing & observability (half to full day)

- [ ] **C1. Introduce mockable seams**
  - [ ] Add `LlmClient` trait in `src/infra/llm.rs`; current struct becomes `HttpLlmClient`
  - [ ] Add `MongoRepo` trait in `src/infra/mongo.rs`; current struct becomes `MongoPool`
  - [ ] Wire via `Box<dyn Trait + Send + Sync>` in `AppState`
  - [ ] Add `mockall = "0.13"` to dev-dependencies
  - [ ] Commit: `refactor: introduce LlmClient and MongoRepo traits for mocking`

- [ ] **C2. Add handler-layer unit tests (no live services required)**
  - [ ] `tests/handler_query_tests.rs` — mock LLM + Mongo, cover happy/error paths
  - [ ] `tests/middleware_auth_tests.rs` — Bearer present/missing/invalid
  - [ ] `tests/middleware_slack_verify_tests.rs` — HMAC signature valid/invalid/replay
  - [ ] Commit: `test: add handler and middleware unit tests with mocks`

- [ ] **C3. Add CI quality gates**
  - [ ] Add `cargo audit` and `cargo deny` to `run.sh --build`
  - [ ] Add `rust-toolchain.toml` with channel = "1.88" (matches Dockerfile)
  - [ ] Add MSRV to `Cargo.toml`: `rust-version = "1.88"`
  - [ ] Commit: `chore: add cargo-audit, cargo-deny, and pin toolchain`

- [ ] **C4. Observability basics**
  - [ ] Add `/metrics` endpoint (Prometheus format, `metrics` + `metrics-exporter-prometheus` crates)
  - [ ] Emit counters: queries_total, llm_parse_failures_total, safety_rejections_total
  - [ ] Emit histogram: query_duration_seconds with provider/model labels
  - [ ] Surface `x-request-id` header on responses
  - [ ] Commit: `feat(obs): add prometheus metrics and request IDs`

## Phase D — Roadmap (NextSteps.md v0.5.x)

Plan is fully specified in `NextSteps.md`. Execute in order:

- [ ] **D1. Phase 1 — Feedback capture (v0.5.0)**
  - [ ] `src/domain/models.rs`: `QueryFeedback`, `FeedbackRequest`, `FeedbackType`
  - [ ] `src/handlers/feedback.rs`: `submit_feedback`, `get_feedback_stats`, `export_feedback`
  - [ ] `src/handlers/mod.rs`: `pub mod feedback;`
  - [ ] `src/handlers/query.rs`: auto-capture `Corrected` on edit-and-execute
  - [ ] `src/main.rs`: three new `/api/feedback/*` routes
  - [ ] `src/domain/safety.rs`: block `query_feedback` collection from user queries
  - [ ] Frontend: `.feedback-row` with 👍 / 👎, inline note textarea, toasts
  - [ ] Integration tests: `feedback_like_roundtrip`, `feedback_export_jsonl`
  - [ ] README changelog v0.5.0
  - [ ] Commit: `feat: phase 1 — feedback capture with explicit + implicit triggers`

- [ ] **D2. Phase 2 — In-context few-shot (v0.5.1)**
  - [ ] `src/domain/models.rs`: `QueryFeedback.embedding: Option<Vec<f32>>`
  - [ ] `src/infra/llm.rs`: `embed_text()` via Ollama `/api/embeddings` + `build_prompt_with_examples()`
  - [ ] `src/infra/mongo.rs`: `find_similar_feedback()` with cosine similarity in Rust
  - [ ] `src/handlers/query.rs`: retrieve top-K similar before `parse_query()`
  - [ ] `src/handlers/feedback.rs`: embed `nl_query` on insert
  - [ ] `src/config.rs`: `embedding_model`, `embedding_url`, `feedback_top_k`
  - [ ] Tests: `test_cosine_similarity`, `test_build_prompt_with_examples`, `test_graceful_degrade_on_embed_failure`
  - [ ] README changelog v0.5.1
  - [ ] Commit: `feat: phase 2 — in-context few-shot via embedding retrieval`

- [ ] **D3. Phase 3 — Training pipeline (v0.5.2)**
  - [ ] `training/README.md` with hardware table + walkthrough
  - [ ] `training/requirements.txt`
  - [ ] `training/prepare_dataset.py` — pulls `/api/feedback/export`
  - [ ] `training/finetune_mlx.py` — Apple Silicon path
  - [ ] `training/finetune_unsloth.py` — NVIDIA CUDA path
  - [ ] `training/export_to_ollama.sh` — merge → GGUF → Ollama
  - [ ] `training/evaluate.py` — before/after comparison on 40-question suite
  - [ ] `training/modelfile.template`
  - [ ] `training/test_prepare_dataset.py` — pytest smoke test
  - [ ] README changelog v0.5.2
  - [ ] Commit: `feat: phase 3 — LoRA fine-tuning + Ollama import pipeline`

---

## Verification After Each Phase

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test integration_tests      # requires live Ollama + MongoDB
./run.sh --docker                         # rebuild and smoke-test UI
```
