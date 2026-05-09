"""
evaluate.py — Before/after evaluation script for the v0.5.x feedback-driven
fine-tuning pipeline.

Purpose
-------
Submits the same set of natural-language test queries to two LLM models
(the original baseline and the fine-tuned variant), compares the parsed
query structures and result counts, and produces a Markdown + JSON report
on whether the fine-tune improved or regressed quality.

When to run
-----------
After fine-tuning is complete and the new model has been imported into
Ollama (e.g. `ollama create qwen3-coder-30b-mongobot -f Modelfile`).
This script IS the acceptance test for Phase 3 of the v0.5.2 roadmap.

Example invocation
------------------
    python training/evaluate.py \\
        --before qwen3-coder:30b \\
        --after qwen3-coder-30b-mongobot \\
        --bot-url http://localhost:3000 \\
        --output-prefix evaluation_2026-05-09 \\
        --auth-token mysecrettoken

Questions source
----------------
By default a hardcoded list of 20 representative queries is used, covering
find, count, distinct, aggregate, lookup, group, filter, sort, and projection
patterns.  Pass --questions-file to supply a JSON array of strings instead.

The script also attempts to parse tests/integration_tests.rs (one level above
the training/ directory) to augment the hardcoded list with queries extracted
from the integration test suite.  Parsing failures are silently ignored and
the hardcoded list is used as-is.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
import time
from pathlib import Path
from typing import Any

import requests

# ---------------------------------------------------------------------------
# Hardcoded baseline question list (20 queries, representative coverage)
# ---------------------------------------------------------------------------
HARDCODED_QUESTIONS: list[str] = [
    # find
    "find all movies",
    "list all actors",
    "show all reviews",
    "find all Drama movies",
    "find movies with rating greater than 8.5",
    "find movies released before the year 2000",
    "find movies directed by Christopher Nolan",
    "find American actors",
    "find all female actors",
    "find reviews with a score of 5",
    # count
    "how many movies are there",
    "count all actors",
    "how many Sci-Fi movies are there",
    "how many male actors are there",
    # distinct
    "what are the distinct genres of movies",
    "what are the unique languages in movies",
    "list the distinct reviewers in reviews",
    # aggregate / group / sort / projection
    "average rating per genre",
    "top 5 movies sorted by rating descending",
    "show me only the title and year of all movies",
]


def _extract_queries_from_rust_tests(repo_root: Path) -> list[str]:
    """
    Attempt to parse tests/integration_tests.rs for query("...") call strings.
    Returns an empty list on any error — callers must not rely on this.
    """
    test_file = repo_root / "tests" / "integration_tests.rs"
    if not test_file.exists():
        return []
    try:
        text = test_file.read_text(encoding="utf-8")
        # Match:  query("...") or query_with_model("...",
        pattern = re.compile(r'query(?:_with_model(?:_and_provider)?)?\("([^"]+)"')
        seen: set[str] = set()
        results: list[str] = []
        for m in pattern.finditer(text):
            q = m.group(1)
            if q not in seen:
                seen.add(q)
                results.append(q)
        return results
    except Exception:
        return []


def load_questions(questions_file: str | None, repo_root: Path) -> list[str]:
    """Return the list of NL questions to evaluate."""
    if questions_file:
        path = Path(questions_file)
        questions: list[str] = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(questions, list):
            raise ValueError(f"{questions_file} must contain a JSON array of strings")
        return questions

    rust_queries = _extract_queries_from_rust_tests(repo_root)
    if rust_queries:
        # Merge: rust queries first, then hardcoded ones not already present
        seen = set(rust_queries)
        merged = list(rust_queries)
        for q in HARDCODED_QUESTIONS:
            if q not in seen:
                merged.append(q)
        return merged

    return list(HARDCODED_QUESTIONS)


# ---------------------------------------------------------------------------
# HTTP helpers
# ---------------------------------------------------------------------------

def _build_headers(auth_token: str | None) -> dict[str, str]:
    headers = {"Content-Type": "application/json"}
    if auth_token:
        headers["Authorization"] = f"Bearer {auth_token}"
    return headers


def query_model(
    bot_url: str,
    question: str,
    model: str,
    provider: str,
    auth_token: str | None,
    timeout: int,
) -> dict[str, Any]:
    """
    POST /api/query and return the parsed JSON body.
    Raises requests.RequestException on network/HTTP errors.
    Returns a dict with an injected '_http_status' key.
    """
    payload = {
        "query": question,
        "model": model,
        "provider": provider,
        "preview": False,
    }
    resp = requests.post(
        f"{bot_url.rstrip('/')}/api/query",
        json=payload,
        headers=_build_headers(auth_token),
        timeout=timeout,
    )
    result: dict[str, Any] = resp.json() if resp.content else {}
    result["_http_status"] = resp.status_code
    return result


# ---------------------------------------------------------------------------
# Percentile helper (no scipy)
# ---------------------------------------------------------------------------

def percentile(values: list[float], pct: float) -> float:
    """Nearest-rank percentile.  pct is 0-100."""
    if not values:
        return 0.0
    sorted_vals = sorted(values)
    idx = int(len(sorted_vals) * pct / 100)
    idx = min(idx, len(sorted_vals) - 1)
    return sorted_vals[idx]


# ---------------------------------------------------------------------------
# Comparison logic
# ---------------------------------------------------------------------------

def compare_responses(
    before: dict[str, Any],
    after: dict[str, Any],
) -> dict[str, Any]:
    """
    Produce a per-question comparison dict.
    Fields:
      before_success, after_success, result_count_before, result_count_after,
      collection_match, operation_match, latency_before_ms, latency_after_ms,
      before_parsed_query, after_parsed_query, mismatch_reasons
    """
    def _success(r: dict[str, Any]) -> bool:
        return r.get("_http_status") == 200 and bool(r.get("success"))

    def _data_count(r: dict[str, Any]) -> int | None:
        data = r.get("data")
        if isinstance(data, list):
            return len(data)
        return None

    def _parsed(r: dict[str, Any]) -> dict[str, Any] | None:
        return r.get("parsed_query")

    def _latency(r: dict[str, Any]) -> float | None:
        timing = r.get("execution_timing")
        if isinstance(timing, dict):
            v = timing.get("total_ms")
            if v is not None:
                return float(v)
        return None

    b_ok = _success(before)
    a_ok = _success(after)
    b_pq = _parsed(before)
    a_pq = _parsed(after)

    collection_match: bool | None = None
    operation_match: bool | None = None
    mismatch_reasons: list[str] = []

    if not b_ok:
        mismatch_reasons.append("before: request failed")
    if not a_ok:
        mismatch_reasons.append("after: request failed")

    if b_pq is None:
        mismatch_reasons.append("before: parsed_query is null")
    if a_pq is None:
        mismatch_reasons.append("after: parsed_query is null")

    if b_pq is not None and a_pq is not None:
        b_col = b_pq.get("collection")
        a_col = a_pq.get("collection")
        collection_match = b_col == a_col
        if not collection_match:
            mismatch_reasons.append(
                f"collection: before={b_col!r} after={a_col!r}"
            )

        b_op = b_pq.get("operation")
        a_op = a_pq.get("operation")
        operation_match = b_op == a_op
        if not operation_match:
            mismatch_reasons.append(
                f"operation: before={b_op!r} after={a_op!r}"
            )

    b_count = _data_count(before)
    a_count = _data_count(after)
    if b_count is not None and a_count is not None and b_count != a_count:
        mismatch_reasons.append(
            f"result_count: before={b_count} after={a_count}"
        )

    return {
        "before_success": b_ok,
        "after_success": a_ok,
        "result_count_before": b_count,
        "result_count_after": a_count,
        "collection_match": collection_match,
        "operation_match": operation_match,
        "latency_before_ms": _latency(before),
        "latency_after_ms": _latency(after),
        "before_parsed_query": b_pq,
        "after_parsed_query": a_pq,
        "mismatch_reasons": mismatch_reasons,
    }


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------

def build_report(
    questions: list[str],
    per_question: list[dict[str, Any]],
    before_model: str,
    after_model: str,
) -> dict[str, Any]:
    n = len(questions)
    b_pass = sum(1 for r in per_question if r["before_success"])
    a_pass = sum(1 for r in per_question if r["after_success"])

    full_match = sum(
        1 for r in per_question
        if r["collection_match"] is True and r["operation_match"] is True
    )

    b_latencies = [r["latency_before_ms"] for r in per_question if r["latency_before_ms"] is not None]
    a_latencies = [r["latency_after_ms"] for r in per_question if r["latency_after_ms"] is not None]

    return {
        "before_model": before_model,
        "after_model": after_model,
        "total_questions": n,
        "before_pass_rate": round(b_pass / n, 4) if n else 0.0,
        "after_pass_rate": round(a_pass / n, 4) if n else 0.0,
        "before_pass_count": b_pass,
        "after_pass_count": a_pass,
        "match_rate": round(full_match / n, 4) if n else 0.0,
        "match_count": full_match,
        "before_latency_p50_ms": percentile(b_latencies, 50),
        "before_latency_p95_ms": percentile(b_latencies, 95),
        "after_latency_p50_ms": percentile(a_latencies, 50),
        "after_latency_p95_ms": percentile(a_latencies, 95),
        "per_question": [
            {"question": q, **cmp}
            for q, cmp in zip(questions, per_question)
        ],
    }


def render_markdown(report: dict[str, Any]) -> str:
    b = report["before_model"]
    a = report["after_model"]
    n = report["total_questions"]

    lines: list[str] = [
        "# Fine-Tune Evaluation Report",
        "",
        f"| | Before (`{b}`) | After (`{a}`) |",
        "|---|---|---|",
        f"| Pass rate | {report['before_pass_rate']:.1%} ({report['before_pass_count']}/{n}) "
        f"| {report['after_pass_rate']:.1%} ({report['after_pass_count']}/{n}) |",
        f"| Collection+operation match rate | | {report['match_rate']:.1%} ({report['match_count']}/{n}) |",
        f"| Latency p50 (ms) | {report['before_latency_p50_ms']:.0f} | {report['after_latency_p50_ms']:.0f} |",
        f"| Latency p95 (ms) | {report['before_latency_p95_ms']:.0f} | {report['after_latency_p95_ms']:.0f} |",
        "",
    ]

    mismatches = [
        r for r in report["per_question"]
        if r["mismatch_reasons"]
    ]

    if not mismatches:
        lines.append("All queries matched on collection and operation.")
    else:
        lines += [
            f"## Mismatches ({len(mismatches)} of {n})",
            "",
        ]
        for r in mismatches:
            lines += [
                f"### `{r['question']}`",
                "",
                f"- Reasons: {'; '.join(r['mismatch_reasons'])}",
            ]
            if r.get("before_parsed_query"):
                lines.append(
                    f"- Before parsed_query: `{json.dumps(r['before_parsed_query'])}`"
                )
            if r.get("after_parsed_query"):
                lines.append(
                    f"- After parsed_query: `{json.dumps(r['after_parsed_query'])}`"
                )
            lines.append("")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Evaluate before/after fine-tune quality against the SlackMongoBot API.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--before", required=True, help="Baseline model name (e.g. qwen3-coder:30b)")
    parser.add_argument("--after", required=True, help="Fine-tuned model name")
    parser.add_argument("--bot-url", default="http://localhost:3000", help="Bot base URL")
    parser.add_argument("--questions-file", default=None, help="Path to JSON array of question strings")
    parser.add_argument("--output-prefix", default=None, help="Prefix for .json/.md output files")
    parser.add_argument("--auth-token", default=None, help="Bearer token for the bot API")
    parser.add_argument("--provider", default="ollama", help="LLM provider name (default: ollama)")
    parser.add_argument("--timeout", type=int, default=60, help="Per-query timeout in seconds (default: 60)")
    return parser.parse_args()


def _abort(msg: str, code: int = 1) -> None:
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(code)


def main() -> None:
    args = parse_args()

    # Resolve repo root: training/ is one level below repo root
    repo_root = Path(__file__).resolve().parent.parent

    # Load questions
    try:
        questions = load_questions(args.questions_file, repo_root)
    except Exception as exc:
        _abort(f"Failed to load questions: {exc}")

    print(f"Loaded {len(questions)} questions.")
    print(f"Before model : {args.before}")
    print(f"After model  : {args.after}")
    print(f"Bot URL      : {args.bot_url}")
    print()

    # Verify connectivity
    try:
        r = requests.get(
            f"{args.bot_url.rstrip('/')}/api/health",
            timeout=10,
            headers=_build_headers(args.auth_token),
        )
        r.raise_for_status()
    except requests.ConnectionError:
        _abort(f"Cannot connect to bot at {args.bot_url}. Is it running?")
    except Exception as exc:
        _abort(f"Health check failed: {exc}")

    per_question_results: list[dict[str, Any]] = []
    before_failures = 0
    after_failures = 0
    threshold = max(1, len(questions) // 2)  # >50% failure threshold

    for i, question in enumerate(questions, 1):
        print(f"[{i:3}/{len(questions)}] {question!r}", end=" ... ", flush=True)

        # Query before model
        try:
            b_resp = query_model(
                args.bot_url, question, args.before, args.provider,
                args.auth_token, args.timeout,
            )
        except requests.Timeout:
            print("TIMEOUT (before)")
            b_resp = {"_http_status": 0, "success": False, "error": "timeout"}
            before_failures += 1
        except requests.RequestException as exc:
            print(f"ERROR (before): {exc}")
            b_resp = {"_http_status": 0, "success": False, "error": str(exc)}
            before_failures += 1

        if before_failures > threshold:
            _abort(
                f"Before model '{args.before}' failed on more than 50% of queries. "
                "Is the model loaded in Ollama?"
            )

        # Query after model
        try:
            a_resp = query_model(
                args.bot_url, question, args.after, args.provider,
                args.auth_token, args.timeout,
            )
        except requests.Timeout:
            print("TIMEOUT (after)")
            a_resp = {"_http_status": 0, "success": False, "error": "timeout"}
            after_failures += 1
        except requests.RequestException as exc:
            print(f"ERROR (after): {exc}")
            a_resp = {"_http_status": 0, "success": False, "error": str(exc)}
            after_failures += 1

        if after_failures > threshold:
            _abort(
                f"After model '{args.after}' failed on more than 50% of queries. "
                "Is the model loaded in Ollama?"
            )

        cmp = compare_responses(b_resp, a_resp)
        per_question_results.append(cmp)

        status = "ok" if not cmp["mismatch_reasons"] else f"MISMATCH({len(cmp['mismatch_reasons'])})"
        print(status)

    report = build_report(questions, per_question_results, args.before, args.after)

    # Determine output prefix
    prefix = args.output_prefix
    if prefix is None:
        from datetime import date
        prefix = f"evaluation_{date.today().isoformat()}"

    json_path = Path(f"{prefix}.json")
    md_path = Path(f"{prefix}.md")

    json_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
    md_path.write_text(render_markdown(report), encoding="utf-8")

    print()
    print(f"Before pass rate : {report['before_pass_rate']:.1%} ({report['before_pass_count']}/{report['total_questions']})")
    print(f"After pass rate  : {report['after_pass_rate']:.1%} ({report['after_pass_count']}/{report['total_questions']})")
    print(f"Match rate       : {report['match_rate']:.1%} ({report['match_count']}/{report['total_questions']})")
    print(f"Latency p50      : before={report['before_latency_p50_ms']:.0f}ms  after={report['after_latency_p50_ms']:.0f}ms")
    print(f"Latency p95      : before={report['before_latency_p95_ms']:.0f}ms  after={report['after_latency_p95_ms']:.0f}ms")
    print()
    print(f"Report written to: {json_path}, {md_path}")

    # Exit with non-zero if after regressed vs before
    if report["after_pass_rate"] < report["before_pass_rate"]:
        print(
            f"\nREGRESSION: after pass rate ({report['after_pass_rate']:.1%}) "
            f"< before ({report['before_pass_rate']:.1%})",
            file=sys.stderr,
        )
        sys.exit(1)

    print("No regression detected.")


if __name__ == "__main__":
    main()
