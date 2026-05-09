"""
finetune_unsloth.py — QLoRA fine-tuning for the NL-to-MongoDB model via unsloth + trl.

Reads a JSONL dataset in OpenAI fine-tune format (produced by prepare_dataset.py) and
trains a LoRA adapter on top of a 4-bit quantised base model using unsloth's optimised
CUDA kernels. The final adapter is saved to --output; the base weights are not modified.

Hardware requirements
---------------------
Model size   Recommended GPU         VRAM
-----------  ----------------------  ----
7B           RTX 4060 Ti 16 GB       16 GB
13B          RTX 3090 / 4090 24 GB   24 GB
30B          A6000 48 GB             48 GB
30B (alt)    Rented A100 / H100      40-80 GB

Example invocation
------------------
    python finetune_unsloth.py \\
        --model unsloth/qwen3-coder-30b-bnb-4bit \\
        --dataset dataset.jsonl \\
        --epochs 3 \\
        --lora-rank 16 \\
        --lora-alpha 32 \\
        --batch-size 2 \\
        --learning-rate 2e-4 \\
        --max-seq-length 4096 \\
        --output ./lora_adapter

Notes
-----
- Paired with finetune_mlx.py for Apple Silicon (MLX back-end, no CUDA required).
- Requires NVIDIA GPU with CUDA; exits with an actionable message if none is detected.
- Packages: pip install unsloth trl datasets
"""

import argparse
import json
import sys
import time
from pathlib import Path


# ---------------------------------------------------------------------------
# Argument parser (defined at module level so --help works without packages)
# ---------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="QLoRA fine-tuning via unsloth + trl for the NL-to-MongoDB model.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument(
        "--model",
        required=True,
        help="HuggingFace model ID or unsloth pre-quantised ID "
             "(e.g. unsloth/qwen3-coder-30b-bnb-4bit).",
    )
    p.add_argument("--dataset", required=True, help="Path to input JSONL dataset.")
    p.add_argument("--epochs", type=int, default=3, help="Number of training epochs.")
    p.add_argument("--lora-rank", type=int, default=16, dest="lora_rank",
                   help="LoRA rank (r).")
    p.add_argument("--lora-alpha", type=int, default=32, dest="lora_alpha",
                   help="LoRA alpha scaling factor.")
    p.add_argument("--batch-size", type=int, default=2, dest="batch_size",
                   help="Per-device training batch size.")
    p.add_argument("--learning-rate", type=float, default=2e-4, dest="learning_rate",
                   help="AdamW learning rate.")
    p.add_argument("--max-seq-length", type=int, default=4096, dest="max_seq_length",
                   help="Maximum token sequence length.")
    p.add_argument("--output", default="./lora_adapter",
                   help="Directory to save the trained LoRA adapter.")
    p.add_argument("--validation-split", type=float, default=0.1,
                   dest="validation_split",
                   help="Fraction of data held out for validation (0 to disable).")
    p.add_argument("--seed", type=int, default=42, help="Random seed.")
    p.add_argument("--gradient-accumulation-steps", type=int, default=4,
                   dest="gradient_accumulation_steps",
                   help="Gradient accumulation steps.")
    return p


# ---------------------------------------------------------------------------
# Dataset helpers
# ---------------------------------------------------------------------------

def validate_dataset(dataset_path: Path) -> int:
    """Return record count; print warnings; exit on fatal errors."""
    if not dataset_path.exists():
        print(f"error: dataset not found: {dataset_path}", file=sys.stderr)
        sys.exit(1)

    count = 0
    with dataset_path.open() as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError as exc:
                print(f"error: invalid JSON on line {lineno}: {exc}", file=sys.stderr)
                sys.exit(1)
            if "messages" not in obj:
                print(
                    f"error: line {lineno} missing 'messages' key — "
                    "expected OpenAI fine-tune format.",
                    file=sys.stderr,
                )
                sys.exit(1)
            count += 1

    if count == 0:
        print("error: dataset is empty — must contain at least 1 record.", file=sys.stderr)
        sys.exit(1)

    if count < 50:
        print(
            f"warning: only {count} records found; fine-tuning quality improves "
            "significantly with 50+ examples.",
            file=sys.stderr,
        )

    return count


def format_with_chat_template(example: dict, tokenizer) -> dict:  # type: ignore[type-arg]
    """Apply the tokenizer's chat template to a messages list."""
    text = tokenizer.apply_chat_template(
        example["messages"],
        tokenize=False,
        add_generation_prompt=False,
    )
    return {"text": text}


# ---------------------------------------------------------------------------
# Training
# ---------------------------------------------------------------------------

def run_training(args: argparse.Namespace) -> None:
    # --- deferred heavy imports (keep --help fast) ---
    try:
        import torch
    except ImportError:
        print(
            "error: torch is not installed.\n"
            "Install: pip install torch --index-url https://download.pytorch.org/whl/cu121",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        from unsloth import FastLanguageModel
    except ImportError:
        print(
            "error: unsloth is not installed.\n"
            "Install: pip install unsloth trl datasets",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        from trl import SFTTrainer, SFTConfig
    except ImportError:
        print(
            "error: trl is not installed.\n"
            "Install: pip install unsloth trl datasets",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        import datasets as hf_datasets
    except ImportError:
        print(
            "error: datasets is not installed.\n"
            "Install: pip install unsloth trl datasets",
            file=sys.stderr,
        )
        sys.exit(1)

    # --- CUDA guard ---
    if not torch.cuda.is_available():
        print(
            "error: no CUDA GPU detected. unsloth requires an NVIDIA GPU.\n"
            "  - If running on a machine with a GPU, ensure the CUDA-enabled "
            "torch is installed.\n"
            "  - For Apple Silicon use finetune_mlx.py instead.",
            file=sys.stderr,
        )
        sys.exit(1)

    dataset_path = Path(args.dataset)
    record_count = validate_dataset(dataset_path)
    print(f"Dataset: {dataset_path} ({record_count} records)")

    output_path = Path(args.output)
    output_path.mkdir(parents=True, exist_ok=True)

    # --- load model + tokenizer ---
    print(f"Loading model: {args.model}  (4-bit QLoRA, max_seq={args.max_seq_length})")
    try:
        model, tokenizer = FastLanguageModel.from_pretrained(
            model_name=args.model,
            max_seq_length=args.max_seq_length,
            load_in_4bit=True,
        )
    except RuntimeError as exc:
        if "out of memory" in str(exc).lower():
            print(
                "error: GPU out of memory loading the model.\n"
                "  - Reduce --max-seq-length (e.g. 2048).\n"
                "  - Use a smaller model.\n"
                "  - Ensure no other processes are using VRAM.",
                file=sys.stderr,
            )
            sys.exit(1)
        raise

    # --- apply LoRA ---
    print(f"Applying LoRA: rank={args.lora_rank}, alpha={args.lora_alpha}")
    model = FastLanguageModel.get_peft_model(
        model,
        r=args.lora_rank,
        lora_alpha=args.lora_alpha,
        target_modules=[
            "q_proj", "k_proj", "v_proj", "o_proj",
            "gate_proj", "up_proj", "down_proj",
        ],
        use_gradient_checkpointing="unsloth",
        random_state=args.seed,
    )

    # --- load + format dataset ---
    print("Loading and formatting dataset...")
    raw = hf_datasets.load_dataset("json", data_files=str(dataset_path), split="train")
    formatted = raw.map(
        lambda ex: format_with_chat_template(ex, tokenizer),
        remove_columns=raw.column_names,
    )

    train_dataset = formatted
    eval_dataset = None
    if args.validation_split > 0.0:
        split = formatted.train_test_split(
            test_size=args.validation_split, seed=args.seed
        )
        train_dataset = split["train"]
        eval_dataset = split["test"]
        print(
            f"Train: {len(train_dataset)} records, "
            f"Eval: {len(eval_dataset)} records"
        )
    else:
        print(f"Train: {len(train_dataset)} records (no validation split)")

    # SFTTrainer writes checkpoints to a temp dir; final adapter goes to --output
    import tempfile
    tmp_dir = tempfile.mkdtemp(prefix="unsloth_checkpoints_")

    training_args = SFTConfig(
        output_dir=tmp_dir,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        gradient_accumulation_steps=args.gradient_accumulation_steps,
        learning_rate=args.learning_rate,
        seed=args.seed,
        eval_strategy="epoch" if eval_dataset is not None else "no",
        save_strategy="no",
        logging_steps=10,
        dataset_text_field="text",
        max_seq_length=args.max_seq_length,
        report_to="none",
    )

    trainer = SFTTrainer(
        model=model,
        tokenizer=tokenizer,
        train_dataset=train_dataset,
        eval_dataset=eval_dataset,
        args=training_args,
    )

    print(
        f"Starting training: epochs={args.epochs}, "
        f"batch_size={args.batch_size}, "
        f"grad_accum={args.gradient_accumulation_steps}, "
        f"lr={args.learning_rate}"
    )
    start_time = time.time()

    try:
        trainer.train()
    except RuntimeError as exc:
        if "out of memory" in str(exc).lower():
            print(
                "error: GPU out of memory during training.\n"
                "  - Reduce --batch-size (e.g. 1).\n"
                "  - Reduce --max-seq-length (e.g. 2048).\n"
                "  - Increase --gradient-accumulation-steps to compensate.",
                file=sys.stderr,
            )
            sys.exit(1)
        raise

    elapsed = time.time() - start_time

    # --- save adapter ---
    print(f"Saving LoRA adapter to: {output_path}")
    model.save_pretrained(str(output_path))
    tokenizer.save_pretrained(str(output_path))

    # --- final report ---
    log_history = trainer.state.log_history
    train_loss = next(
        (e["train_loss"] for e in reversed(log_history) if "train_loss" in e),
        None,
    )
    eval_loss = next(
        (e["eval_loss"] for e in reversed(log_history) if "eval_loss" in e),
        None,
    )

    print("\n--- Training complete ---")
    print(f"Adapter path:      {output_path.resolve()}")
    print(f"Training time:     {elapsed:.1f}s ({elapsed / 60:.1f} min)")
    if train_loss is not None:
        print(f"Final train loss:  {train_loss:.4f}")
    if eval_loss is not None:
        print(f"Final eval loss:   {eval_loss:.4f}")
    else:
        print("Final eval loss:   n/a (no validation split)")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    run_training(args)


if __name__ == "__main__":
    main()
