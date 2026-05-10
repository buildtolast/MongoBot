# NextSteps — Query Feedback + In-Context Learning + Fine-Tuning Pipeline

> **STATUS: COMPLETED — landed in commits `9642b65` (v0.5.0+v0.5.1) and `304e54f` (v0.5.2) on 2026-05-09.**
> Phase 1 (feedback capture), Phase 2 (in-context few-shot retrieval), and Phase 3
> (LoRA training pipeline) are all in main. See top-level `README.md` for usage and
> `training/README.md` for the fine-tuning walkthrough. The original plan below is
> retained for historical reference.

---

> **HISTORICAL:** This document is the approved design for the next three commits.
> It will be executed in sequence: Phase 1 -> Phase 2 -> Phase 3.
> Nothing in this file has been implemented yet.

## Context

Users need to mark queries as correct (👍) / incorrect (👎), feed that back into the model, and — when enough data is collected — actually train a new local model. Three phases, three commits:

1. **Phase 1:** Capture feedback (explicit + implicit) into a MongoDB dataset
2. **Phase 2:** At query time, inject top-K similar past examples into the prompt — "the model sees how it got similar questions right before"
3. **Phase 3:** A working Python pipeline that reads the dataset, LoRA fine-tunes a base model, and imports the result into Ollama as a new model

This gives users *immediate* quality gains (Phase 2) AND a path to a permanently improved local model (Phase 3).

---

# Phase 1 — Feedback capture

## Data model

MongoDB collection `query_feedback` (same database, schemaless, indexed on `{feedback, timestamp}`):

```json
{
  "_id": ObjectId,
  "timestamp": "2026-04-14T...",
  "nl_query": "find all active users",
  "provider": "ollama",
  "model": "qwen3-coder:30b",
  "raw_llm_response": "{...}",
  "parsed_query": {...},
  "user_edited_query": {...} | null,
  "mongo_query": "db.users.find({...})",
  "feedback": "liked" | "disliked" | "corrected" | "abandoned",
  "note": "optional freeform" | null,
  "executed": true,
  "result_count": 16,
  "schema_snapshot": "users(name:string,...)"
}
```

## Files modified

| File | Changes |
|---|---|
| `src/domain/models.rs` | `QueryFeedback`, `FeedbackRequest`, `FeedbackType`; `ConfirmRequest.original_parsed: Option<ParsedQuery>` |
| `src/handlers/feedback.rs` | **New** — `submit_feedback`, `get_feedback_stats`, `export_feedback` |
| `src/handlers/mod.rs` | Add `pub mod feedback;` |
| `src/handlers/query.rs` | `handle_execute` auto-captures `Corrected` when `original_parsed` differs from `parsed` |
| `src/main.rs` | 3 new routes under `/api/feedback/*` |
| `src/domain/safety.rs` | Block `query_feedback` from being queried by users (prevents noise/recursion) |

## Endpoints

1. `POST /api/feedback` — explicit submission. Body: FeedbackRequest. Returns `{id}`.
2. `GET /api/feedback/stats` — `{total, by_type, by_provider, by_model}`.
3. `GET /api/feedback/export?format=jsonl|json|dpo-jsonl&type=liked,corrected`
   - `jsonl` → OpenAI fine-tune format (one JSON per line with `messages` array)
   - `json` → raw records
   - `dpo-jsonl` → `{prompt, chosen, rejected}` pairs (only Corrected entries)

## Frontend

| File | Changes |
|---|---|
| `src/frontend/shell_mid.html` | `.feedback-row` in `#results-header` with 👍 / 👎 buttons + inline note textarea (hidden until 👎 clicked) |
| `src/frontend/js/02_api.js` | `submitFeedback(body)` wrapper |
| `src/frontend/js/06_app.js` | Click handlers; auto-capture on edit-execute and reject; toast on success |
| `src/frontend/js/04_render.js` | `showToast(msg, type)` helper; show/hide `.feedback-row` in `renderResults` |
| `src/frontend/styles/04_results_table.css` | `.feedback-row`, `.feedback-btn`, `.feedback-note` |

## Auto-capture triggers

- 👍 clicked → `feedback_type: liked`, toast "Saved as positive example"
- 👎 clicked → inline note textarea appears → submit → `disliked` (or `corrected` if parsed JSON was edited)
- Approve-mode: user edits parsed JSON then clicks Approve → frontend sends `original_parsed` in `ConfirmRequest` → server diffs and auto-inserts `corrected` record. Silent toast.
- Approve-mode: user clicks Reject → silent POST `abandoned`, no toast

## Tests (Phase 1)

- `test_feedback_type_serialization` (serde round-trip)
- `test_diff_parsed_queries` (detect original != edited)
- `test_export_jsonl_format` (OpenAI format validation)
- `feedback_like_roundtrip` integration test
- `feedback_export_jsonl` integration test

---

# Phase 2 — In-context few-shot injection

## How it works

Before building the LLM prompt, retrieve the top-K (default K=5) most similar liked/corrected examples from `query_feedback` and inject them into the system prompt.

## Retrieval strategy

**Primary (Ollama available):** embedding similarity.
- On feedback insert, compute embedding of `nl_query` via `POST {OLLAMA_URL}/api/embeddings` with model `nomic-embed-text` (small, fast, universally available via Ollama)
- Store the embedding as `Vec<f32>` in the MongoDB record (field `embedding`)
- On new query, embed the NL text, find top-K by cosine similarity
- Falls back gracefully to keyword search if embedding endpoint fails

**Fallback (no embeddings available):** simple text search.
- MongoDB text index on `nl_query`
- `$text: { $search: <nl_query> }` with `$meta: "textScore"` sorting

**Choice:** try embeddings first (controlled by `ENABLE_FEEDBACK_RETRIEVAL=true` env var, default on). Fall back silently to text search on any error.

## Files modified (Phase 2)

| File | Changes |
|---|---|
| `src/domain/models.rs` | `QueryFeedback.embedding: Option<Vec<f32>>` |
| `src/infra/llm.rs` | New `embed_text(text: &str, embedding_url: &str)` method calling Ollama's `/api/embeddings` |
| `src/infra/mongo.rs` | `find_similar_feedback(embedding: &[f32], k: usize)` — cosine sim in Rust (MongoDB doesn't do vector search natively without Atlas) |
| `src/infra/llm.rs` | `build_prompt_with_examples(query, schema, examples)` — extends `build_prompt` to include a "Examples from prior successful queries:" section |
| `src/handlers/query.rs` | Call `find_similar_feedback()` before `parse_query()`, pass examples in |
| `src/handlers/feedback.rs` | On insert, embed `nl_query` and store |
| `src/config.rs` | `embedding_model: String` (default `nomic-embed-text`), `embedding_url: String` (defaults to `OLLAMA_URL`), `feedback_top_k: usize` (default 5) |

## Cosine similarity in Rust (simple)

```rust
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb + 1e-10)
}
```

For small feedback collections (< 10K records) this is fast enough without a vector index. Above 10K, switch to MongoDB Atlas Vector Search or FAISS — out of scope.

## Prompt extension

Current prompt is ~10 lines (src/infra/llm.rs). With examples:

```
You are a MongoDB query translator. Return ONLY valid JSON...

Collections:
<schema>

Examples from prior successful queries (same database):
Q: "find active users"
A: {"collection":"users","filters":[{"field":"active","operator":"eq","value":true}]}

Q: "count managers by department"
A: {"operation":"aggregate","group_by":{"field":"department","accumulators":[...]}}

Required: ...

Query: <user_query>
```

Examples are injected only when `query_feedback` collection has ≥ 1 matching record.

## Tests (Phase 2)

- `test_cosine_similarity` — basic unit test
- `test_build_prompt_with_examples` — verifies examples are rendered correctly
- `test_graceful_degrade_on_embed_failure` — mocks embedding failure, confirms fallback to text search
- Integration test: seed a feedback record, submit a similar query, verify `raw_llm_response` matches the seeded example's schema (shows model was influenced)

---

# Phase 3 — Actual model fine-tuning pipeline

## Deliverable: `training/` directory with working scripts

```
training/
├── README.md                  — full walkthrough + hardware notes
├── requirements.txt           — pip dependencies
├── prepare_dataset.py         — downloads /api/feedback/export, formats for training
├── finetune_mlx.py            — Apple Silicon (MLX-LM)    — primary path
├── finetune_unsloth.py        — NVIDIA (Unsloth)          — CUDA path
├── export_to_ollama.sh        — merges LoRA, converts to GGUF, imports to Ollama
├── evaluate.py                — runs before/after comparison on integration test queries
└── modelfile.template         — Ollama Modelfile template for import
```

## Pipeline end-to-end

```bash
# 1. Export dataset from the running bot
cd training
python prepare_dataset.py \
  --bot-url http://localhost:3000 \
  --feedback-types liked,corrected \
  --output dataset.jsonl

# 2. Fine-tune (Apple Silicon)
python finetune_mlx.py \
  --model Qwen/Qwen3-Coder-30B \
  --dataset dataset.jsonl \
  --epochs 3 \
  --lora-rank 16 \
  --output ./adapters

# OR (NVIDIA CUDA)
python finetune_unsloth.py \
  --model unsloth/qwen3-coder-30b-bnb-4bit \
  --dataset dataset.jsonl \
  --epochs 3 \
  --output ./lora_adapter

# 3. Merge + convert + import into Ollama
./export_to_ollama.sh \
  --base Qwen/Qwen3-Coder-30B \
  --adapter ./adapters \
  --name qwen3-coder-30b-mongobot \
  --quant q4_k_m

# 4. Evaluate
python evaluate.py \
  --before qwen3-coder:30b \
  --after qwen3-coder-30b-mongobot \
  --bot-url http://localhost:3000
```

## Key script details

### `prepare_dataset.py` (Python)
- Calls `GET {BOT_URL}/api/feedback/export?format=jsonl&type={types}`
- Writes JSONL file
- Validates each record has `messages` with system/user/assistant roles
- Reports stats: total records, avg tokens per record, breakdown by feedback type

### `finetune_mlx.py` (Apple Silicon)
- Uses `mlx-lm` package (`pip install mlx-lm`)
- Calls `mlx_lm.lora` programmatically with LoRA config
- Works on M2/M3/M4 Max with 32GB+ unified memory for 7-14B models, 64GB+ for 30B
- Saves LoRA adapter to `./adapters/`

### `finetune_unsloth.py` (NVIDIA)
- Uses `unsloth` + `trl` packages
- 4-bit QLoRA for memory efficiency
- Works on RTX 3090/4090 (24GB) for 13B, A6000 (48GB) for 30B
- Saves LoRA adapter to `./lora_adapter/`

### `export_to_ollama.sh` (shell)
1. Merges LoRA adapter into base weights via `mlx_lm.fuse` or `unsloth.merge_and_unload`
2. Converts merged HF model → GGUF via `llama.cpp/convert_hf_to_gguf.py`
3. Quantizes GGUF to specified level (default q4_k_m)
4. Creates Ollama Modelfile from template
5. Runs `ollama create <name> -f Modelfile`
6. Verifies import: `ollama list | grep <name>`

### `evaluate.py`
- Loads the 40-question integration test set (hardcoded or reads from `tests/integration_tests.rs` via regex)
- Submits each query twice: once to `--before` model, once to `--after`
- Compares parsed query structure + result count
- Reports: pass rate before vs after, latency before vs after, diffs on mismatched queries
- Output: JSON report + markdown summary

## `training/README.md` contents

- Prerequisites: Python 3.10+, ≥50 feedback records, GPU (or Apple Silicon with 32GB+)
- Hardware table

| Model size | Min VRAM | Realistic hardware |
|---|---|---|
| 7B | 16 GB | RTX 4060 Ti / M2 Max 32GB |
| 13-14B | 24 GB | RTX 3090/4090 / M3 Max 64GB |
| 27-30B | 40 GB | A6000 / M4 Max 128GB / rented A100 |
| 70B | 80 GB | H100 / rented |

- MLX quickstart (Mac)
- Unsloth quickstart (Linux CUDA)
- Cloud GPU rental notes (vast.ai, runpod.io): ~$1-3/hr for A100/H100
- Troubleshooting: OOM errors, divergence, overfitting on small datasets
- Cost estimate: ~$5-20 for a 30B fine-tune on rented H100 with 500 examples

## Files added (Phase 3)

All under `training/` — no changes to Rust code.

## Tests (Phase 3)

Not unit-testable (requires GPU + model download). Instead:
- `training/test_prepare_dataset.py` — smoke test of dataset formatting (pytest)
- `training/README.md` — documented manual test procedure
- `evaluate.py` IS the test — run it after fine-tuning to prove the model improved

---

# Commit sequence

```
Commit A (Phase 1) — Feedback capture
  src/domain/models.rs, src/handlers/feedback.rs, src/handlers/mod.rs,
  src/handlers/query.rs, src/main.rs, src/domain/safety.rs,
  src/frontend/** (buttons, toast, styling),
  tests/integration_tests.rs (+2 tests)
  README changelog v0.5.0

Commit B (Phase 2) — In-context few-shot
  src/domain/models.rs (embedding field),
  src/infra/llm.rs (embed_text, build_prompt_with_examples),
  src/infra/mongo.rs (find_similar_feedback),
  src/handlers/query.rs (retrieve + inject),
  src/handlers/feedback.rs (embed on insert),
  src/config.rs (embedding config)
  README changelog v0.5.1

Commit C (Phase 3) — Training pipeline
  training/** (new directory, 6 files)
  README — link to training/README.md
  changelog v0.5.2
```

---

# Verification

**Phase 1:**
1. `cargo test --lib` → 124 + 3 = 127 passing
2. Docker rebuild, 👍 a query, check `curl /api/feedback/stats`
3. Edit JSON in approve mode, click Approve, verify `corrected` count +1
4. `curl '/api/feedback/export?format=jsonl' > ds.jsonl`, validate OpenAI shape

**Phase 2:**
1. Seed 3 feedback records covering different collections
2. Submit a similar query, inspect `raw_llm_response` and the system prompt via tracing — verify examples were injected
3. Compare result quality before/after with the integration suite

**Phase 3:**
1. Collect ≥50 corrected examples (dogfood with real usage)
2. Run the full pipeline: prepare → finetune → export → evaluate
3. Check `evaluate.py` output shows ≥ baseline pass rate
4. Confirm new model appears in `ollama list` and works when selected in UI

---

# Out of scope

- Vector search at >10K feedback records (requires Atlas/FAISS)
- Auto-triggered training when threshold reached (manual for now)
- Multi-turn conversations (dataset is single-turn Q→A)
- Fine-tuning Anthropic/OpenAI models (possible via their APIs but outside this plan)
