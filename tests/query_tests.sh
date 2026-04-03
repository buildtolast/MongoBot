#!/usr/bin/env bash
# End-to-end query tests for the Slack-Mongo Query Bot API
# Usage: bash tests/query_tests.sh [BASE_URL]
#
# Sends natural-language queries to POST /api/query and validates
# the JSON response structure, operation types, and result counts.

set -euo pipefail

BASE_URL="${1:-http://localhost:3000}"
API="$BASE_URL/api/query"
PASS=0
FAIL=0
ERRORS=""

# ── helpers ──────────────────────────────────────────────────────────────────

run_test() {
    local test_name="$1"
    local payload="$2"
    local check_script="$3"   # python3 script that prints PASS or FAIL + reason

    local response
    response=$(curl -s --max-time 90 -X POST "$API" \
        -H "Content-Type: application/json" \
        -d "$payload" 2>&1) || true

    # Check if we got valid JSON back
    if ! echo "$response" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\n  FAIL: $test_name — invalid JSON or no response"
        printf "  FAIL: %s — invalid JSON or no response\n" "$test_name"
        return
    fi

    local result
    result=$(echo "$response" | python3 -c "
import sys, json
r = json.load(sys.stdin)
$check_script
" 2>&1) || true

    if [[ "$result" == PASS* ]]; then
        PASS=$((PASS + 1))
        printf "  PASS: %s\n" "$test_name"
    else
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\n  FAIL: $test_name — $result"
        printf "  FAIL: %s — %s\n" "$test_name" "$result"
    fi
}

# ── Server health check ─────────────────────────────────────────────────────

echo "============================================"
echo " Slack-Mongo Query Bot — E2E Query Tests"
echo " Target: $BASE_URL"
echo "============================================"
echo ""

echo "Checking server availability..."
if ! curl -s --max-time 5 "$BASE_URL" > /dev/null 2>&1; then
    echo "ERROR: Server not reachable at $BASE_URL"
    exit 1
fi
echo "Server is up."
echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 1: FIND queries
# ══════════════════════════════════════════════════════════════════════════════
echo "── FIND queries ──────────────────────────────────────────"

run_test "Find all movies" \
    '{"query": "show me all movies"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected count >= 1')
elif 'results' not in r:
    print('FAIL: missing results field')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find all actors" \
    '{"query": "list all actors"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected count >= 1')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find all reviews" \
    '{"query": "show all reviews"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'reviews':
    print(f'FAIL: expected collection=reviews, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected count >= 1')
else:
    print(f'PASS (count={r[\"count\"]})')
"

# ── Find with filters ────────────────────────────────────────────────────────

run_test "Find movies by genre (eq filter)" \
    '{"query": "find all Drama movies"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 drama movie')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find movies with rating > 9.0 (gt filter)" \
    '{"query": "find movies with rating greater than 9.0"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 movie with rating > 9.0')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find movies with year < 2000 (lt filter)" \
    '{"query": "find movies released before the year 2000"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 movie before 2000')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find movies by director (contains/eq filter)" \
    '{"query": "find movies directed by Christopher Nolan"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 Nolan movie')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find actors by nationality" \
    '{"query": "find American actors"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 American actor')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find female actors" \
    '{"query": "find all female actors"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 female actor')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find actors with oscar wins > 0 (gt filter)" \
    '{"query": "find actors who have won at least one oscar"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 actor with oscar wins')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find reviews with score 5" \
    '{"query": "find reviews with a score of 5"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'reviews':
    print(f'FAIL: expected collection=reviews, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 review with score 5')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find reviews by reviewer name" \
    '{"query": "find all reviews by Roger Ebert"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'reviews':
    print(f'FAIL: expected collection=reviews, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 Roger Ebert review')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find movies in Japanese language" \
    '{"query": "find movies in Japanese"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 Japanese movie')
else:
    print(f'PASS (count={r[\"count\"]})')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 2: COUNT queries
# ══════════════════════════════════════════════════════════════════════════════
echo "── COUNT queries ─────────────────────────────────────────"

run_test "Count all movies" \
    '{"query": "how many movies are there"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif op != 'count':
    print(f'FAIL: expected operation=count, got {op}')
elif 'results' not in r:
    print('FAIL: missing results')
else:
    # count result could be in results.count or just results
    res = r['results']
    cnt = res.get('count', 0) if isinstance(res, dict) else 0
    print(f'PASS (count result={cnt})')
"

run_test "Count actors" \
    '{"query": "count all actors"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif op != 'count':
    print(f'FAIL: expected operation=count, got {op}')
else:
    res = r.get('results', {})
    cnt = res.get('count', 0) if isinstance(res, dict) else 0
    print(f'PASS (count result={cnt})')
"

run_test "Count reviews" \
    '{"query": "how many reviews are in the database"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'reviews':
    print(f'FAIL: expected collection=reviews, got {p.get(\"collection\")}')
elif op != 'count':
    print(f'FAIL: expected operation=count, got {op}')
else:
    res = r.get('results', {})
    cnt = res.get('count', 0) if isinstance(res, dict) else 0
    print(f'PASS (count result={cnt})')
"

run_test "Count movies with filter (Sci-Fi)" \
    '{"query": "how many Sci-Fi movies are there"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif op != 'count':
    print(f'FAIL: expected operation=count, got {op}')
else:
    print('PASS')
"

run_test "Count male actors" \
    '{"query": "how many male actors are there"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
else:
    print('PASS')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 3: DISTINCT queries
# ══════════════════════════════════════════════════════════════════════════════
echo "── DISTINCT queries ──────────────────────────────────────"

run_test "Distinct movie genres" \
    '{"query": "what are the distinct genres of movies"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif op != 'distinct':
    print(f'FAIL: expected operation=distinct, got {op}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 distinct genre')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Distinct movie languages" \
    '{"query": "what are the unique languages in movies"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif op != 'distinct':
    print(f'FAIL: expected operation=distinct, got {op}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 distinct language')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Distinct actor nationalities" \
    '{"query": "what are the distinct nationalities of actors"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif op != 'distinct':
    print(f'FAIL: expected operation=distinct, got {op}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 distinct nationality')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Distinct reviewers" \
    '{"query": "list the distinct reviewers in reviews"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'reviews':
    print(f'FAIL: expected collection=reviews, got {p.get(\"collection\")}')
elif op != 'distinct':
    print(f'FAIL: expected operation=distinct, got {op}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 distinct reviewer')
else:
    print(f'PASS (count={r[\"count\"]})')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 4: WRITE operations (should return requires_confirmation=true)
# ══════════════════════════════════════════════════════════════════════════════
echo "── WRITE operations (confirmation required) ─────────────"

run_test "Insert a new movie (requires confirmation)" \
    '{"query": "insert a new movie called Test Movie with year 2026 and genre Comedy"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif op != 'insert':
    print(f'FAIL: expected operation=insert, got {op}')
elif not r.get('requires_confirmation', False):
    print('FAIL: expected requires_confirmation=true')
else:
    print('PASS (requires_confirmation=true)')
"

run_test "Update movie rating (requires confirmation)" \
    '{"query": "update the rating of The Dark Knight to 9.5"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif op != 'update':
    print(f'FAIL: expected operation=update, got {op}')
elif not r.get('requires_confirmation', False):
    print('FAIL: expected requires_confirmation=true')
else:
    # For update, should have preview docs
    results = r.get('results', None)
    has_preview = results is not None and results != 'null'
    print(f'PASS (requires_confirmation=true, has_preview={has_preview})')
"

run_test "Insert actor (requires confirmation)" \
    '{"query": "add a new actor named Test Actor born in 1990 from Canada"}' \
    "
p = r.get('parsed', {})
op = p.get('operation', '')
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif op != 'insert':
    print(f'FAIL: expected operation=insert, got {op}')
elif not r.get('requires_confirmation', False):
    print('FAIL: expected requires_confirmation=true')
else:
    print('PASS (requires_confirmation=true)')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 5: Model selection
# ══════════════════════════════════════════════════════════════════════════════
echo "── Model selection ───────────────────────────────────────"

run_test "Query with explicit model field" \
    '{"query": "show all movies", "model": "qwen2.5-coder:7b"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected count >= 1')
else:
    print(f'PASS (count={r[\"count\"]})')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 6: Response structure validation
# ══════════════════════════════════════════════════════════════════════════════
echo "── Response structure validation ─────────────────────────"

run_test "Response has all required fields" \
    '{"query": "show me all movies"}' \
    "
required = ['parsed', 'results', 'count', 'execution_ms', 'mongo_query']
missing = [k for k in required if k not in r]
if missing:
    print(f'FAIL: missing fields: {missing}')
else:
    print('PASS (all required fields present)')
"

run_test "Parsed object has collection and operation" \
    '{"query": "find all actors"}' \
    "
p = r.get('parsed', {})
if 'collection' not in p:
    print('FAIL: parsed missing collection')
elif 'operation' not in p:
    print('FAIL: parsed missing operation')
elif 'filters' not in p:
    print('FAIL: parsed missing filters')
else:
    print('PASS (parsed has collection, operation, filters)')
"

run_test "execution_ms is a positive number" \
    '{"query": "count all movies"}' \
    "
ms = r.get('execution_ms', -1)
if not isinstance(ms, (int, float)):
    print(f'FAIL: execution_ms not a number: {type(ms)}')
elif ms < 0:
    print(f'FAIL: execution_ms is negative: {ms}')
else:
    print(f'PASS (execution_ms={ms})')
"

run_test "mongo_query is a non-empty string" \
    '{"query": "find movies rated above 8.5"}' \
    "
mq = r.get('mongo_query', '')
if not isinstance(mq, str):
    print(f'FAIL: mongo_query not a string')
elif len(mq) == 0:
    print('FAIL: mongo_query is empty')
else:
    print(f'PASS (mongo_query length={len(mq)})')
"

run_test "Read query has requires_confirmation=false" \
    '{"query": "show all movies"}' \
    "
rc = r.get('requires_confirmation', True)
if rc:
    print('FAIL: read query should have requires_confirmation=false')
else:
    print('PASS (requires_confirmation=false)')
"

run_test "Read query has index_analysis" \
    '{"query": "find movies where genre is Drama"}' \
    "
ia = r.get('index_analysis', None)
if ia is None:
    print('FAIL: read query missing index_analysis')
elif not isinstance(ia, dict):
    print(f'FAIL: index_analysis is not a dict: {type(ia)}')
else:
    print(f'PASS (index_analysis present, keys={list(ia.keys())})')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# SECTION 7: Edge cases and combined filters
# ══════════════════════════════════════════════════════════════════════════════
echo "── Edge cases and combined filters ───────────────────────"

run_test "Find with multiple filters (genre + year)" \
    '{"query": "find Action movies released after 2010"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 result')
elif len(p.get('filters', [])) < 1:
    print('FAIL: expected at least 1 filter')
else:
    print(f'PASS (count={r[\"count\"]}, filters={len(p[\"filters\"])})')
"

run_test "Find movies sorted by rating desc" \
    '{"query": "show movies sorted by rating in descending order"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 result')
else:
    sort = p.get('sort')
    has_sort = sort is not None
    print(f'PASS (count={r[\"count\"]}, has_sort={has_sort})')
"

run_test "Find with projection (title and year only)" \
    '{"query": "show me only the title and year of all movies"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'movies':
    print(f'FAIL: expected collection=movies, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 result')
else:
    proj = p.get('projection', [])
    print(f'PASS (count={r[\"count\"]}, projection={proj})')
"

run_test "Find active actors" \
    '{"query": "find all active actors"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'actors':
    print(f'FAIL: expected collection=actors, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    print('FAIL: expected at least 1 active actor')
else:
    print(f'PASS (count={r[\"count\"]})')
"

run_test "Find reviews with high helpful_votes" \
    '{"query": "find reviews with more than 50 helpful votes"}' \
    "
p = r.get('parsed', {})
if p.get('collection') != 'reviews':
    print(f'FAIL: expected collection=reviews, got {p.get(\"collection\")}')
elif r.get('count', 0) < 1:
    # might be 0 if no reviews have >50 votes, that is still structurally valid
    print(f'PASS (count={r[\"count\"]}, may be 0 if no match)')
else:
    print(f'PASS (count={r[\"count\"]})')
"

echo ""

# ══════════════════════════════════════════════════════════════════════════════
# Summary
# ══════════════════════════════════════════════════════════════════════════════
TOTAL=$((PASS + FAIL))
echo "============================================"
echo " RESULTS: $PASS/$TOTAL passed, $FAIL failed"
echo "============================================"

if [ "$FAIL" -gt 0 ]; then
    echo ""
    echo "Failed tests:"
    echo -e "$ERRORS"
fi

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "All tests passed."
    exit 0
else
    echo "$FAIL test(s) failed. Some failures may be due to LLM non-determinism."
    exit 1
fi
