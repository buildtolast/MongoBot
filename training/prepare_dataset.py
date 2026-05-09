"""
prepare_dataset.py — Fetch feedback records from the SlackMongoBot API and write
an OpenAI fine-tune JSONL dataset.

Usage:
    python prepare_dataset.py \
        --bot-url http://localhost:3000 \
        --feedback-types liked,corrected \
        --output dataset.jsonl \
        [--auth-token TOKEN] \
        [--min-records 50]

The /api/feedback/export endpoint accepts a single ?type= value per request.
When multiple feedback types are requested this script makes one HTTP call per
type and concatenates the results.
"""

import argparse
import json
import sys
import urllib.request
import urllib.error
from typing import Optional


# ---------------------------------------------------------------------------
# Pure validation helpers (importable by tests without side-effects)
# ---------------------------------------------------------------------------

def validate_record(line: str) -> bool:
    """Return True if line is a valid OpenAI fine-tune JSONL record.

    Expected shape:
        {"messages": [{"role": "user", "content": "..."}, {"role": "assistant", "content": "..."}]}
    """
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return False

    if not isinstance(obj, dict):
        return False
    messages = obj.get("messages")
    if not isinstance(messages, list) or len(messages) < 2:
        return False

    roles = [m.get("role") for m in messages if isinstance(m, dict)]
    if "user" not in roles or "assistant" not in roles:
        return False

    # Every message must have a non-empty role and a content key
    for msg in messages:
        if not isinstance(msg, dict):
            return False
        if not isinstance(msg.get("role"), str) or not msg["role"]:
            return False
        if "content" not in msg:
            return False

    return True


def compute_stats(records: list[dict]) -> dict:
    """Return summary statistics for a list of validated record dicts."""
    total = len(records)
    if total == 0:
        return {
            "total": 0,
            "avg_user_chars": 0,
            "avg_assistant_chars": 0,
            "estimated_tokens": 0,
        }

    user_chars = 0
    assistant_chars = 0
    for rec in records:
        for msg in rec.get("messages", []):
            content = msg.get("content", "")
            if msg.get("role") == "user":
                user_chars += len(content)
            elif msg.get("role") == "assistant":
                assistant_chars += len(content)

    total_chars = user_chars + assistant_chars
    return {
        "total": total,
        "avg_user_chars": round(user_chars / total, 1),
        "avg_assistant_chars": round(assistant_chars / total, 1),
        # 1 token ~ 4 chars; stated as a rough estimate
        "estimated_tokens": round(total_chars / 4),
    }


# ---------------------------------------------------------------------------
# HTTP helpers
# ---------------------------------------------------------------------------

def _build_request(url: str, auth_token: Optional[str]) -> urllib.request.Request:
    headers: dict[str, str] = {"Accept": "application/x-jsonlines"}
    if auth_token:
        headers["Authorization"] = f"Bearer {auth_token}"
    return urllib.request.Request(url, headers=headers)


def fetch_lines_for_type(
    bot_url: str,
    feedback_type: str,
    auth_token: Optional[str],
) -> list[str]:
    """Fetch raw JSONL lines for a single feedback type from the export endpoint.

    Returns a list of raw line strings (may include invalid ones — caller validates).
    Raises SystemExit on HTTP errors.
    """
    url = f"{bot_url.rstrip('/')}/api/feedback/export?format=jsonl&type={feedback_type}"
    req = _build_request(url, auth_token)
    try:
        with urllib.request.urlopen(req) as resp:
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        print(
            f"ERROR: HTTP {exc.code} fetching type '{feedback_type}': {exc.reason}",
            file=sys.stderr,
        )
        sys.exit(1)
    except urllib.error.URLError as exc:
        print(f"ERROR: Could not connect to {bot_url}: {exc.reason}", file=sys.stderr)
        sys.exit(1)

    return [line for line in body.splitlines() if line.strip()]


# ---------------------------------------------------------------------------
# Core pipeline
# ---------------------------------------------------------------------------

def run(
    bot_url: str,
    feedback_types: list[str],
    output_path: str,
    auth_token: Optional[str],
    min_records: int,
) -> int:
    """Fetch, validate, write, and report.  Returns 0 on success, 1 on write failure."""
    raw_lines: list[str] = []
    for ftype in feedback_types:
        lines = fetch_lines_for_type(bot_url, ftype, auth_token)
        raw_lines.extend(lines)

    total_fetched = len(raw_lines)

    valid_records: list[dict] = []
    for idx, line in enumerate(raw_lines, start=1):
        if validate_record(line):
            valid_records.append(json.loads(line))
        else:
            print(
                f"WARNING: skipping invalid record at position {idx}: {line[:80]!r}",
                file=sys.stderr,
            )

    # Write output
    try:
        with open(output_path, "w", encoding="utf-8") as fh:
            for rec in valid_records:
                fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
    except OSError as exc:
        print(f"ERROR: Failed to write {output_path}: {exc}", file=sys.stderr)
        return 1

    stats = compute_stats(valid_records)

    # Summary report
    print(f"Records fetched  : {total_fetched}")
    print(f"Records validated: {stats['total']}")
    print(f"Records skipped  : {total_fetched - stats['total']}")
    print(f"Avg user chars   : {stats['avg_user_chars']}")
    print(f"Avg assistant chars: {stats['avg_assistant_chars']}")
    print(
        f"Estimated tokens : {stats['estimated_tokens']} "
        "(rough estimate: 1 token ~ 4 chars)"
    )
    print(f"Output written to: {output_path}")

    if stats["total"] == 0:
        print(
            "WARNING: 0 valid records written. "
            "Ensure the bot has feedback data and the types are correct.",
            file=sys.stderr,
        )
    elif stats["total"] < min_records:
        print(
            f"WARNING: only {stats['total']} records collected "
            f"(threshold: {min_records}). "
            "Fine-tuning quality may be limited with this few examples.",
            file=sys.stderr,
        )

    return 0


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Fetch feedback from SlackMongoBot and write an OpenAI fine-tune JSONL dataset.",
    )
    parser.add_argument(
        "--bot-url",
        required=True,
        help="Base URL of the running bot (e.g. http://localhost:3000)",
    )
    parser.add_argument(
        "--feedback-types",
        default="liked,corrected",
        help="Comma-separated list of feedback types to export (default: liked,corrected)",
    )
    parser.add_argument(
        "--output",
        default="dataset.jsonl",
        help="Output file path (default: dataset.jsonl)",
    )
    parser.add_argument(
        "--auth-token",
        default=None,
        help="Bearer token for /api/* (set DASHBOARD_TOKEN on the bot if auth is enabled)",
    )
    parser.add_argument(
        "--min-records",
        type=int,
        default=50,
        help="Warn (not error) if fewer than this many records are collected (default: 50)",
    )

    args = parser.parse_args()
    feedback_types = [t.strip() for t in args.feedback_types.split(",") if t.strip()]
    if not feedback_types:
        print("ERROR: --feedback-types must contain at least one value.", file=sys.stderr)
        sys.exit(1)

    exit_code = run(
        bot_url=args.bot_url,
        feedback_types=feedback_types,
        output_path=args.output,
        auth_token=args.auth_token,
        min_records=args.min_records,
    )
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
