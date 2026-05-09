"""
test_prepare_dataset.py — Pytest smoke tests for the local helper functions in
prepare_dataset.py.  No network calls are made.
"""

import json
import pytest

from prepare_dataset import validate_record, compute_stats


# ---------------------------------------------------------------------------
# validate_record
# ---------------------------------------------------------------------------

def _make_line(user: str = "find all movies", assistant: str = '{"collection":"movies"}') -> str:
    return json.dumps({
        "messages": [
            {"role": "user", "content": user},
            {"role": "assistant", "content": assistant},
        ]
    })


class TestValidateRecord:
    def test_valid_record_returns_true(self):
        assert validate_record(_make_line()) is True

    def test_missing_messages_key_returns_false(self):
        line = json.dumps({"data": [{"role": "user", "content": "hi"}]})
        assert validate_record(line) is False

    def test_wrong_role_names_returns_false(self):
        line = json.dumps({
            "messages": [
                {"role": "human", "content": "find movies"},
                {"role": "bot", "content": "{}"},
            ]
        })
        assert validate_record(line) is False

    def test_non_json_line_returns_false(self):
        assert validate_record("not json at all") is False

    def test_empty_messages_list_returns_false(self):
        line = json.dumps({"messages": []})
        assert validate_record(line) is False

    def test_single_message_returns_false(self):
        line = json.dumps({"messages": [{"role": "user", "content": "hi"}]})
        assert validate_record(line) is False

    def test_messages_missing_content_key_returns_false(self):
        line = json.dumps({
            "messages": [
                {"role": "user"},
                {"role": "assistant", "content": "{}"},
            ]
        })
        assert validate_record(line) is False

    def test_empty_string_returns_false(self):
        assert validate_record("") is False

    def test_extra_fields_in_record_are_allowed(self):
        obj = {
            "messages": [
                {"role": "user", "content": "query"},
                {"role": "assistant", "content": "{}"},
            ],
            "metadata": {"type": "liked"},
        }
        assert validate_record(json.dumps(obj)) is True


# ---------------------------------------------------------------------------
# compute_stats
# ---------------------------------------------------------------------------

class TestComputeStats:
    def test_empty_list_returns_zeros(self):
        stats = compute_stats([])
        assert stats["total"] == 0
        assert stats["avg_user_chars"] == 0
        assert stats["avg_assistant_chars"] == 0
        assert stats["estimated_tokens"] == 0

    def test_single_record_char_counts(self):
        records = [json.loads(_make_line(user="abcd", assistant="efgh"))]
        stats = compute_stats(records)
        assert stats["total"] == 1
        assert stats["avg_user_chars"] == 4.0
        assert stats["avg_assistant_chars"] == 4.0
        # 8 chars total / 4 = 2 tokens
        assert stats["estimated_tokens"] == 2

    def test_token_estimate_is_chars_divided_by_four(self):
        # 40 user chars + 40 assistant chars = 80 total -> 20 tokens
        records = [json.loads(_make_line(user="a" * 40, assistant="b" * 40))]
        stats = compute_stats(records)
        assert stats["estimated_tokens"] == 20

    def test_average_across_multiple_records(self):
        r1 = json.loads(_make_line(user="aa", assistant="bbbb"))    # 2 user, 4 asst
        r2 = json.loads(_make_line(user="cccc", assistant="dd"))    # 4 user, 2 asst
        stats = compute_stats([r1, r2])
        assert stats["total"] == 2
        assert stats["avg_user_chars"] == 3.0
        assert stats["avg_assistant_chars"] == 3.0
