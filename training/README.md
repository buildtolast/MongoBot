# Training pipeline

This directory contains scripts to fine-tune a local LLM on the bot's `query_feedback` data and import the result into Ollama as a new model. The pipeline expects the bot to be running (or at least to have produced a feedback export) and at least 50 corrected/liked feedback records to produce a useful adapter.

## Prerequisites

- Python 3.10+ (`python3 --version`)
- ≥50 records in `query_feedback` (`curl http://localhost:3000/api/feedback/stats`)
- One of:
  - Apple Silicon Mac, M2 or newer, 32GB+ unified memory (use `finetune_mlx.py`)
  - Linux/Windows with NVIDIA GPU, 24GB+ VRAM (use `finetune_unsloth.py`)
  - Rented cloud GPU (vast.ai, runpod.io — A100/H100 ~$1-3/hr)
- `ollama` installed and running locally (for the final import step)
- `git` and `make` for building llama.cpp (used by `export_to_ollama.sh`)

## Hardware requirements

| Model size | Min memory | Realistic local hardware |
|---|---|---|
| 7B   | 16 GB | RTX 4060 Ti / M2 Max 32GB |
| 13-14B | 24 GB | RTX 3090 / RTX 4090 / M3 Max 64GB |
| 27-30B | 40 GB | A6000 / M4 Max 128GB / rented A100 |
| 70B  | 80 GB | H100 / rented |

For most users we recommend starting with a 7B model (qwen3:7b or qwen2.5-coder:7b) to validate the pipeline end-to-end before scaling up.

## End-to-end pipeline

```bash
cd training
pip install -r requirements.txt
# (and pip install mlx-lm OR pip install unsloth trl datasets — see Per-platform setup below)

# 1. Export the dataset from the running bot
python prepare_dataset.py \
  --bot-url http://localhost:3000 \
  --feedback-types liked,corrected \
  --output dataset.jsonl

# 2a. Fine-tune on Apple Silicon
python finetune_mlx.py \
  --model Qwen/Qwen3-Coder-30B \
  --dataset dataset.jsonl \
  --epochs 3 \
  --lora-rank 16 \
  --output ./adapters

# 2b. OR fine-tune on NVIDIA
python finetune_unsloth.py \
  --model unsloth/qwen3-coder-30b-bnb-4bit \
  --dataset dataset.jsonl \
  --epochs 3 \
  --lora-rank 16 \
  --output ./lora_adapter

# 3. Merge LoRA, convert to GGUF, import into Ollama
./export_to_ollama.sh \
  --base Qwen/Qwen3-Coder-30B \
  --adapter ./adapters \
  --name qwen3-coder-30b-mongobot \
  --quant q4_k_m

# 4. Evaluate the new model against the original
python evaluate.py \
  --before qwen3-coder:30b \
  --after qwen3-coder-30b-mongobot \
  --bot-url http://localhost:3000 \
  --output-prefix eval_$(date +%Y-%m-%d)

# 5. Switch the bot to use the new model via the model dropdown in the UI,
#    or via env var OLLAMA_MODEL=qwen3-coder-30b-mongobot before restart.
```

## Per-platform setup

### Apple Silicon (MLX)
- `pip install mlx-lm`
- Verify: `python -c "import mlx_lm; print(mlx_lm.__version__)"`
- Models load directly from HuggingFace; first run downloads the base model (~30GB for 30B variants).

### NVIDIA (Unsloth)
- `pip install unsloth trl datasets bitsandbytes`
- Verify: `python -c "import unsloth, trl; print(unsloth.__version__, trl.__version__)"`
- 4-bit QLoRA is enabled by default to fit larger models on consumer GPUs.

### Cloud GPU
- vast.ai or runpod.io: rent an A100 (40GB) or H100 (80GB) at ~$1-3/hour.
- A 30B fine-tune with 500 records typically takes 1-3 hours, so $5-20 per run.
- SSH in, clone this repo, follow the NVIDIA path.

## Troubleshooting

**Out of memory during training**
- Reduce `--batch-size` (try 1-2)
- Reduce `--max-seq-length` to 2048 (Unsloth)
- Use a smaller base model (7B instead of 30B)
- Increase `--gradient-accumulation-steps` (Unsloth)

**Loss not decreasing**
- Dataset may be too small (<50 records is unreliable)
- Learning rate too low: try `--learning-rate 5e-4`
- Check that the assistant content in the dataset is valid JSON; bad targets prevent learning

**Model overfits to a few examples**
- Common with small datasets. Reduce `--epochs` to 1-2.
- Add more diverse feedback before retraining.

**Ollama import fails on `ollama create`**
- Ensure ollama is running: `ollama list`
- Check the GGUF file exists and is non-empty
- Look for permission issues on the workdir

## Cost estimates

| Setup | Per fine-tune cost |
|---|---|
| Local Apple Silicon (electricity only) | ~$0.10 |
| Local NVIDIA RTX 4090 (electricity only) | ~$0.30 |
| Rented A100 (40GB), 1-2 hours | $2-6 |
| Rented H100 (80GB), 30B model, 1-2 hours | $5-15 |

## Tests

```bash
pytest test_prepare_dataset.py
```

This is a smoke test of the dataset formatting only — it does not run the fine-tune or hit the bot. Comprehensive evaluation happens via `evaluate.py` after a fine-tune.

## File map

- `prepare_dataset.py` — pulls feedback from the bot, writes JSONL
- `finetune_mlx.py` — Apple Silicon fine-tune
- `finetune_unsloth.py` — NVIDIA CUDA fine-tune
- `export_to_ollama.sh` — merge adapter, convert to GGUF, import into Ollama
- `evaluate.py` — before/after comparison on a representative query set
- `modelfile.template` — Ollama Modelfile used by `export_to_ollama.sh`
- `requirements.txt` — base Python dependencies
- `test_prepare_dataset.py` — pytest smoke test
- `README.md` — this file
