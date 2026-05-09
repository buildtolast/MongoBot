"""
Apple Silicon LoRA fine-tuning script for the NL-to-MongoDB query bot.

Uses the mlx-lm package (https://github.com/ml-explore/mlx-lm) to run
LoRA fine-tuning on Apple Silicon hardware via Apple's MLX framework.
The script splits an OpenAI fine-tune format JSONL dataset into train/
validation splits, writes them to disk, then delegates to the mlx_lm.lora
CLI for the actual training loop. Adapter weights are saved under --output.

Hardware requirements:
  - Apple Silicon (M2 or newer) is mandatory; MLX does not run on x86.
  - 7B-14B models: M2/M3/M4 with 32GB+ unified memory recommended.
  - 30B models: M4 Max with 64GB+ unified memory recommended.

This script is one of two fine-tune entry points:
  - finetune_mlx.py     (this file)  — Apple Silicon via mlx-lm
  - finetune_unsloth.py             — NVIDIA GPU via Unsloth + HuggingFace

Example invocation:
    python finetune_mlx.py \\
        --model Qwen/Qwen3-Coder-30B \\
        --dataset dataset.jsonl \\
        --epochs 3 \\
        --lora-rank 16 \\
        --lora-alpha 32 \\
        --batch-size 4 \\
        --learning-rate 1e-4 \\
        --output ./adapters
"""

import argparse
import json
import random
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def check_mlx_lm() -> None:
    """Exit with a clear message if mlx_lm is not installed."""
    try:
        import mlx_lm  # noqa: F401
    except ImportError:
        print(
            "ERROR: mlx_lm is not installed.\n"
            "Install it with:  pip install mlx-lm\n"
            "MLX requires Apple Silicon (M1/M2/M3/M4). It will not run on x86.",
            file=sys.stderr,
        )
        sys.exit(1)


def load_and_validate_dataset(dataset_path: Path) -> list[dict]:
    """Load JSONL dataset, validate structure, and warn on small size."""
    if not dataset_path.exists():
        print(f"ERROR: Dataset file not found: {dataset_path}", file=sys.stderr)
        sys.exit(1)

    records: list[dict] = []
    with dataset_path.open("r", encoding="utf-8") as fh:
        for lineno, line in enumerate(fh, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError as exc:
                print(
                    f"ERROR: Invalid JSON on line {lineno} of {dataset_path}: {exc}",
                    file=sys.stderr,
                )
                sys.exit(1)
            if "messages" not in obj:
                print(
                    f"ERROR: Line {lineno} is missing required 'messages' key.",
                    file=sys.stderr,
                )
                sys.exit(1)
            records.append(obj)

    if len(records) == 0:
        print("ERROR: Dataset contains no valid records.", file=sys.stderr)
        sys.exit(1)

    if len(records) < 50:
        print(
            f"WARNING: Dataset has only {len(records)} records. "
            "Fine-tune quality will likely be poor; 200+ records is recommended.",
            file=sys.stderr,
        )

    return records


def split_dataset(
    records: list[dict],
    validation_split: float,
    seed: int,
    output_dir: Path,
) -> tuple[Path, Path]:
    """Write train.jsonl and valid.jsonl under output_dir and return their paths."""
    rng = random.Random(seed)
    shuffled = records.copy()
    rng.shuffle(shuffled)

    n_valid = max(1, int(len(shuffled) * validation_split))
    n_train = len(shuffled) - n_valid

    if n_train < 1:
        print(
            "ERROR: Not enough records to create a non-empty training split. "
            "Reduce --validation-split or add more data.",
            file=sys.stderr,
        )
        sys.exit(1)

    train_path = output_dir / "train.jsonl"
    valid_path = output_dir / "valid.jsonl"

    def write_jsonl(path: Path, rows: list[dict]) -> None:
        with path.open("w", encoding="utf-8") as fh:
            for row in rows:
                fh.write(json.dumps(row) + "\n")

    write_jsonl(train_path, shuffled[:n_train])
    write_jsonl(valid_path, shuffled[n_train:])

    print(f"Dataset split: {n_train} train / {n_valid} validation records.")
    return train_path, valid_path


def run_training(
    model: str,
    data_dir: Path,
    adapter_path: Path,
    num_iters: int,
    batch_size: int,
    learning_rate: float,
    lora_rank: int,
    lora_alpha: int,
) -> tuple[int, str]:
    """
    Invoke mlx_lm.lora via subprocess.

    Returns (returncode, combined_output_text).
    """
    cmd = [
        sys.executable, "-m", "mlx_lm.lora",
        "--model", model,
        "--train",
        "--data", str(data_dir),
        "--iters", str(num_iters),
        "--batch-size", str(batch_size),
        "--learning-rate", str(learning_rate),
        "--lora-rank", str(lora_rank),
        "--adapter-path", str(adapter_path),
    ]

    print(f"Running: {' '.join(cmd)}\n")

    output_lines: list[str] = []
    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        assert proc.stdout is not None
        for line in proc.stdout:
            print(line, end="", flush=True)
            output_lines.append(line)
        proc.wait()
        return proc.returncode, "".join(output_lines)
    except FileNotFoundError:
        print(
            "ERROR: Could not launch Python interpreter or mlx_lm.lora module. "
            "Ensure mlx-lm is installed in the current environment.",
            file=sys.stderr,
        )
        sys.exit(1)
    except subprocess.CalledProcessError as exc:
        print(f"ERROR: mlx_lm.lora exited with code {exc.returncode}.", file=sys.stderr)
        sys.exit(exc.returncode)
    except KeyboardInterrupt:
        print("\nInterrupted by user. Waiting for subprocess to finish...", file=sys.stderr)
        if proc.poll() is None:
            proc.terminate()
            proc.wait()
        sys.exit(130)


def parse_final_loss(output: str) -> str | None:
    """
    Best-effort parse of the final training loss from mlx_lm output.

    mlx_lm typically prints lines like:
      Iter 100: Train loss 0.4321, ...
    We return the last loss value found.
    """
    pattern = re.compile(r"[Tt]rain\s+loss\s+([\d.]+)")
    matches = pattern.findall(output)
    return matches[-1] if matches else None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="LoRA fine-tuning for NL-to-MongoDB bot on Apple Silicon via mlx-lm.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("--model", required=True, help="HuggingFace model ID or local path.")
    parser.add_argument("--dataset", required=True, type=Path, help="Path to input JSONL dataset.")
    parser.add_argument("--epochs", type=int, default=3, help="Number of training epochs.")
    parser.add_argument("--lora-rank", type=int, default=16, help="LoRA rank (r).")
    parser.add_argument("--lora-alpha", type=int, default=32, help="LoRA alpha scaling factor.")
    parser.add_argument("--batch-size", type=int, default=4, help="Per-device batch size.")
    parser.add_argument("--learning-rate", type=float, default=1e-4, help="Learning rate.")
    parser.add_argument("--output", type=Path, default=Path("./adapters"), help="Output directory for LoRA adapter.")
    parser.add_argument("--validation-split", type=float, default=0.1, help="Fraction of data used for validation.")
    parser.add_argument("--seed", type=int, default=42, help="Random seed for dataset shuffle.")
    return parser.parse_args()


def main() -> None:
    args = parse_args()

    # Defer mlx_lm import check so --help always works without the package installed.
    check_mlx_lm()

    records = load_and_validate_dataset(args.dataset)
    num_records = len(records)

    args.output.mkdir(parents=True, exist_ok=True)

    # Write splits into a temp dir inside --output so they survive the run for inspection.
    splits_dir = args.output / "splits"
    splits_dir.mkdir(exist_ok=True)

    split_dataset(
        records=records,
        validation_split=args.validation_split,
        seed=args.seed,
        output_dir=splits_dir,
    )

    # mlx_lm.lora uses --iters rather than --epochs; approximate the conversion.
    n_train = max(1, num_records - max(1, int(num_records * args.validation_split)))
    num_iters = max(1, (n_train // args.batch_size) * args.epochs)
    print(f"Estimated iterations: {num_iters}  ({n_train} train records, batch {args.batch_size}, {args.epochs} epochs)")

    start_time = time.monotonic()

    returncode, output = run_training(
        model=args.model,
        data_dir=splits_dir,
        adapter_path=args.output,
        num_iters=num_iters,
        batch_size=args.batch_size,
        learning_rate=args.learning_rate,
        lora_rank=args.lora_rank,
        lora_alpha=args.lora_alpha,
    )

    elapsed = time.monotonic() - start_time
    elapsed_str = time.strftime("%H:%M:%S", time.gmtime(elapsed))

    if returncode != 0:
        print(
            f"\nERROR: Training failed with exit code {returncode}. "
            "See mlx_lm output above for details.",
            file=sys.stderr,
        )
        sys.exit(returncode)

    final_loss = parse_final_loss(output)
    loss_str = final_loss if final_loss is not None else "not found in output"

    print("\n--- Training complete ---")
    print(f"  Adapter path : {args.output.resolve()}")
    print(f"  Training time: {elapsed_str}")
    print(f"  Final loss   : {loss_str}")


if __name__ == "__main__":
    main()
