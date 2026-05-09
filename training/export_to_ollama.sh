#!/usr/bin/env bash
# export_to_ollama.sh — Merge a LoRA adapter into a base model and import it into Ollama.
#
# Supports adapters produced by:
#   - finetune_mlx.py     (Apple Silicon / mlx-lm)
#   - finetune_unsloth.py (NVIDIA / PEFT + HuggingFace)
#
# Usage:
#   ./export_to_ollama.sh \
#     --base Qwen/Qwen3-Coder-30B \
#     --adapter ./adapters \
#     --name qwen3-coder-30b-mongobot \
#     [--quant q4_k_m] \
#     [--workdir /tmp/mongobot-export] \
#     [--llamacpp-dir ./llama.cpp] \
#     [--mode auto|mlx|hf] \
#     [--keep-workdir] \
#     [--dry-run]

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE_FILE="$SCRIPT_DIR/modelfile.template"
CURRENT_STEP=""

# ---------------------------------------------------------------------------
# Error trap
# ---------------------------------------------------------------------------
trap 'echo "" >&2; echo "ERROR: script failed at step: ${CURRENT_STEP:-startup}" >&2' ERR

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
step() {
    CURRENT_STEP="$*"
    echo "" >&2
    echo ">>> $*" >&2
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

run() {
    if [[ "${DRY_RUN:-false}" == "true" ]]; then
        echo "  [dry-run] $*" >&2
    else
        "$@"
    fi
}

usage() {
    cat >&2 <<'EOF'
Usage: export_to_ollama.sh --base <model> --adapter <path> --name <name> [OPTIONS]

Required:
  --base <model>          HuggingFace model ID or local path (e.g. Qwen/Qwen3-Coder-30B)
  --adapter <path>        Directory containing LoRA adapter files
  --name <name>           Ollama model name to create (e.g. qwen3-coder-30b-mongobot)

Optional:
  --quant <type>          Quantization type (default: q4_k_m)
  --workdir <path>        Working directory for intermediates (default: ./_export_workspace)
  --llamacpp-dir <path>   Path to llama.cpp checkout (default: ./llama.cpp)
  --mode auto|mlx|hf      Adapter format (default: auto-detect)
  --keep-workdir          Keep merged weights and intermediates after success
  --dry-run               Print commands without executing them
  --help                  Show this help and exit
EOF
    exit 0
}

# ---------------------------------------------------------------------------
# Step 1: Argument parsing
# ---------------------------------------------------------------------------
BASE=""
ADAPTER=""
NAME=""
QUANT="q4_k_m"
WORKDIR=""
LLAMACPP_DIR="./llama.cpp"
MODE="auto"
KEEP_WORKDIR=false
DRY_RUN=false

# Handle --help before anything else so it always works
for arg in "$@"; do
    [[ "$arg" == "--help" || "$arg" == "-h" ]] && usage
done

while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)        BASE="$2";         shift 2 ;;
        --adapter)     ADAPTER="$2";      shift 2 ;;
        --name)        NAME="$2";         shift 2 ;;
        --quant)       QUANT="$2";        shift 2 ;;
        --workdir)     WORKDIR="$2";      shift 2 ;;
        --llamacpp-dir) LLAMACPP_DIR="$2"; shift 2 ;;
        --mode)        MODE="$2";         shift 2 ;;
        --keep-workdir) KEEP_WORKDIR=true; shift ;;
        --dry-run)     DRY_RUN=true;      shift ;;
        *) die "Unknown option: $1. Run with --help for usage." ;;
    esac
done

[[ -z "$BASE" ]]    && die "--base is required"
[[ -z "$ADAPTER" ]] && die "--adapter is required"
[[ -z "$NAME" ]]    && die "--name is required"

[[ -z "$WORKDIR" ]] && WORKDIR="$(pwd)/_export_workspace"
WORKDIR="$(realpath "$WORKDIR" 2>/dev/null || echo "$WORKDIR")"

case "$MODE" in
    auto|mlx|hf) ;;
    *) die "--mode must be one of: auto, mlx, hf" ;;
esac

# ---------------------------------------------------------------------------
# Step 2: Dependency checks
# ---------------------------------------------------------------------------
step "Step 1: Checking dependencies"

command -v python3 >/dev/null 2>&1 || die "python3 not found. Install Python 3.9+."
command -v ollama  >/dev/null 2>&1 || die "ollama not found. Install from https://ollama.com"

echo "  python3: $(python3 --version 2>&1)" >&2
echo "  ollama:  $(ollama --version 2>&1 | head -1)" >&2
echo "  base:    $BASE" >&2
echo "  adapter: $ADAPTER" >&2
echo "  name:    $NAME" >&2
echo "  quant:   $QUANT" >&2
echo "  workdir: $WORKDIR" >&2

[[ "$DRY_RUN" == "true" ]] && echo "  [dry-run mode enabled — no commands will be executed]" >&2

# ---------------------------------------------------------------------------
# Step 3: Adapter format detection
# ---------------------------------------------------------------------------
step "Step 2: Detecting adapter format"

if [[ "$DRY_RUN" == "false" ]]; then
    [[ -d "$ADAPTER" ]] || die "Adapter directory not found: $ADAPTER"
fi

if [[ "$MODE" == "auto" ]]; then
    if [[ "$DRY_RUN" == "true" ]]; then
        MODE="mlx"
        echo "  [dry-run] Skipping format detection — assuming mlx" >&2
    else
        # MLX: adapters.npz or *.safetensors without adapter_config.json
        # HF/PEFT: adapter_config.json (standard PEFT output)
        if [[ -f "$ADAPTER/adapter_config.json" ]]; then
            MODE="hf"
        elif ls "$ADAPTER"/*.npz 2>/dev/null | head -1 | grep -q .; then
            MODE="mlx"
        elif ls "$ADAPTER"/*.safetensors 2>/dev/null | head -1 | grep -q .; then
            # safetensors without adapter_config.json: treat as mlx (mlx-lm uses this)
            MODE="mlx"
        else
            die "Cannot detect adapter format in $ADAPTER. Use --mode mlx or --mode hf explicitly."
        fi
    fi
    echo "  Detected mode: $MODE" >&2
else
    echo "  Mode (explicit): $MODE" >&2
fi

# ---------------------------------------------------------------------------
# Step 4: Create workdir and merge LoRA into base weights
# ---------------------------------------------------------------------------
step "Step 3: Merging LoRA adapter into base weights"

run mkdir -p "$WORKDIR"
MERGED_DIR="$WORKDIR/merged"

if [[ "$MODE" == "mlx" ]]; then
    echo "  Using mlx_lm.fuse to merge adapter" >&2
    run python3 -m mlx_lm.fuse \
        --model "$BASE" \
        --adapter-path "$ADAPTER" \
        --save-path "$MERGED_DIR"
else
    echo "  Using PEFT merge_and_unload to merge adapter" >&2
    run python3 -c "
import sys
base_model = sys.argv[1]
adapter_path = sys.argv[2]
out_path = sys.argv[3]

try:
    from peft import PeftModel
    from transformers import AutoModelForCausalLM, AutoTokenizer
except ImportError as e:
    print(f'ERROR: Missing dependency: {e}', file=sys.stderr)
    print('Install with: pip install peft transformers', file=sys.stderr)
    sys.exit(1)

import torch
print('  Loading base model...', file=sys.stderr)
base = AutoModelForCausalLM.from_pretrained(
    base_model,
    torch_dtype=torch.float16,
    device_map='cpu',
    low_cpu_mem_usage=True,
)
tokenizer = AutoTokenizer.from_pretrained(base_model)
print('  Loading LoRA adapter...', file=sys.stderr)
model = PeftModel.from_pretrained(base, adapter_path)
print('  Merging and unloading...', file=sys.stderr)
merged = model.merge_and_unload()
merged.save_pretrained(out_path)
tokenizer.save_pretrained(out_path)
print('  Saved merged model to: ' + out_path, file=sys.stderr)
" "$BASE" "$ADAPTER" "$MERGED_DIR"
fi

# ---------------------------------------------------------------------------
# Step 5: Convert merged model to GGUF (f16)
# ---------------------------------------------------------------------------
step "Step 4: Converting merged model to GGUF (f16)"

LLAMACPP_CONVERT="$LLAMACPP_DIR/convert_hf_to_gguf.py"
LLAMACPP_QUANTIZE="$LLAMACPP_DIR/llama-quantize"

if [[ "$DRY_RUN" == "false" ]]; then
    if [[ ! -d "$LLAMACPP_DIR" ]]; then
        cat >&2 <<EOF

ERROR: llama.cpp directory not found at: $LLAMACPP_DIR

llama.cpp is required to convert the merged model to GGUF format.
Clone and build it with:

    git clone https://github.com/ggerganov/llama.cpp $LLAMACPP_DIR
    cd $LLAMACPP_DIR && make

This may take 5-15 minutes. Re-run this script once complete.
EOF
        exit 1
    fi
    [[ -f "$LLAMACPP_CONVERT" ]]  || die "convert_hf_to_gguf.py not found in $LLAMACPP_DIR — is llama.cpp built?"
    [[ -x "$LLAMACPP_QUANTIZE" ]] || die "llama-quantize not found or not executable in $LLAMACPP_DIR — run: cd $LLAMACPP_DIR && make"
fi

F16_GGUF="$WORKDIR/${NAME}.f16.gguf"

run python3 "$LLAMACPP_CONVERT" \
    "$MERGED_DIR" \
    --outfile "$F16_GGUF" \
    --outtype f16

# ---------------------------------------------------------------------------
# Step 6: Quantize GGUF
# ---------------------------------------------------------------------------
step "Step 5: Quantizing GGUF to $QUANT"

QUANT_GGUF="$WORKDIR/${NAME}.${QUANT}.gguf"

run "$LLAMACPP_QUANTIZE" "$F16_GGUF" "$QUANT_GGUF" "$QUANT"

# ---------------------------------------------------------------------------
# Step 7: Generate Modelfile
# ---------------------------------------------------------------------------
step "Step 6: Generating Ollama Modelfile"

MODELFILE="$WORKDIR/Modelfile"

if [[ "$DRY_RUN" == "false" ]]; then
    QUANT_GGUF_ABS="$(realpath "$QUANT_GGUF")"
else
    QUANT_GGUF_ABS="$QUANT_GGUF"
fi

if [[ "$DRY_RUN" == "true" ]]; then
    if [[ -f "$TEMPLATE_FILE" ]]; then
        echo "  [dry-run] Would render template $TEMPLATE_FILE -> $MODELFILE" >&2
    else
        echo "  [dry-run] Template not found — would write minimal Modelfile to $MODELFILE" >&2
    fi
elif [[ -f "$TEMPLATE_FILE" ]]; then
    echo "  Using template: $TEMPLATE_FILE" >&2
    sed \
        -e "s|{{GGUF_PATH}}|$QUANT_GGUF_ABS|g" \
        -e "s|{{MODEL_NAME}}|$NAME|g" \
        "$TEMPLATE_FILE" > "$MODELFILE"
else
    echo "  Template not found — generating minimal Modelfile" >&2
    cat > "$MODELFILE" <<MODELFILE_EOF
FROM $QUANT_GGUF_ABS

SYSTEM "You are $NAME, a MongoDB query assistant. Convert natural language to MongoDB queries."

PARAMETER num_ctx 4096
PARAMETER temperature 0.1
PARAMETER top_p 0.9
MODELFILE_EOF
fi

# ---------------------------------------------------------------------------
# Step 8: Import into Ollama
# ---------------------------------------------------------------------------
step "Step 7: Importing model into Ollama"

run ollama create "$NAME" -f "$MODELFILE"

if [[ "$DRY_RUN" == "false" ]]; then
    if ollama list | grep -q "^${NAME}"; then
        echo "  Model verified in ollama list: $NAME" >&2
    else
        die "Model import appeared to succeed but '$NAME' was not found in 'ollama list'"
    fi
else
    echo "  [dry-run] Would verify with: ollama list | grep $NAME" >&2
fi

# ---------------------------------------------------------------------------
# Step 9: Cleanup (keep GGUFs, optionally remove merged weights)
# ---------------------------------------------------------------------------
step "Step 8: Cleanup"

if [[ "$KEEP_WORKDIR" == "false" && "$DRY_RUN" == "false" ]]; then
    echo "  Removing merged weights (keeping GGUFs)..." >&2
    rm -rf "$MERGED_DIR"
else
    echo "  Keeping workdir: $WORKDIR" >&2
fi

# ---------------------------------------------------------------------------
# Success summary
# ---------------------------------------------------------------------------
echo "" >&2
echo ">>> SUCCESS" >&2
echo "" >&2
echo "  Model name : $NAME" >&2
if [[ "$DRY_RUN" == "false" && -f "$QUANT_GGUF" ]]; then
    GGUF_SIZE="$(du -sh "$QUANT_GGUF" 2>/dev/null | cut -f1)"
    echo "  GGUF size  : $GGUF_SIZE  ($QUANT_GGUF)" >&2
else
    echo "  GGUF path  : $QUANT_GGUF" >&2
fi
echo "" >&2
echo "  Example usage:" >&2
echo "    ollama run $NAME \"find all movies\"" >&2
echo "" >&2
