# Query Generation Audit Report

**Date:** 2026-04-04
**Scope:** Comparison of implementation (`executor.rs`, `query_advisor.rs`, `llm.rs`, `safety.rs`, `mongo.rs`, `models.rs`) against `mongodb_query_reference.md` sections 1-14.

---

## Section 1: Comparison Operators

### $eq (Implicit)

**COMPLIANT.** The executor uses implicit equality in `build_single_condition` -- when `FilterOperator::Eq`, the raw BSON value is returned directly rather than wrapping in `{ "$eq": value }`. This follows the reference's recommendation to prefer the implicit form.

### $ne

**COMPLIANT.** `$ne` is supported via `FilterOperator::Ne` in `build_single_condition` (executor.rs:187). The query advisor warns about `$ne` performance with code `NE_OPERATOR` (query_advisor.rs:144-149). The warning text correctly advises restructuring to positive matches.

### $gt, $gte, $lt, $lte

**COMPLIANT.** All four range operators are implemented (executor.rs:188-191). No issues.

### $in

**PARTIALLY COMPLIANT.** `$in` is implemented correctly (executor.rs:192-195) and the LLM prompt lists it as an available operator. However:

- **GAP: No LARGE_IN_ARRAY warning.** The reference (Section 13, Mistake 4, and Section 14 rule `LARGE_IN_ARRAY`) recommends flagging `$in` with >200 values. Neither `query_advisor.rs` nor `safety.rs` checks for large `$in` arrays.
  - **Fix needed:** Add a check in `query_advisor.rs::analyze_query()` that iterates filters, and when `FilterOperator::In` is found, checks if `filter.value` is an array with length >200. Emit a warning with code `LARGE_IN_ARRAY`.

### $nin

**COMPLIANT.** Supported and warned about (query_advisor.rs:154-159, code `NIN_OPERATOR`).

---

## Section 2: Logical Operators

### $and (implicit)

**PARTIALLY COMPLIANT.** Multiple filters are combined as implicit AND via `build_filter` (executor.rs:219-265), which inserts each filter as a separate key in the document. This is correct for the common case.

- **GAP: No explicit $and support.** The reference notes that explicit `$and` is required when multiple conditions target the same field with the same operator (e.g., two range conditions on the same field). The current `build_filter` overwrites previous values if two top-level filters target the same field (the second `doc.insert` replaces the first). This is a silent correctness bug for queries like "price > 10 AND price < 100" when both come as separate filters on field "price".
  - **Fix needed:** In `executor.rs::build_filter()`, detect when multiple top-level filters target the same field. When this happens, merge conditions into the same BSON sub-document (e.g., `{ "price": { "$gt": 10, "$lt": 100 } }`) or wrap in an explicit `$and` array.

### $or

**GAP.** The reference documents `$or` as a fundamental logical operator. The `FilterOperator` enum in `models.rs` does not include an `Or` variant. There is no way for the LLM to express `$or` queries. The LLM prompt does not mention `$or`. Users asking "show me orders that are either shipped or delivered" cannot be served correctly.

- **Fix needed:** This is a significant expressiveness gap. To address it properly would require either:
  1. Adding `$or` support to the `ParsedQuery` model (e.g., a `logical_groups` field), or
  2. Allowing the LLM to output raw MongoDB filter documents that are passed through after safety validation.

### $not, $nor

**GAP.** Not supported in the filter model. The reference warns these are performance anti-patterns anyway, so omission is defensible. However, the safety checker does not flag `$not` or `$nor` if they somehow appeared in raw query output.

- **Severity: Low.** Acceptable to omit given the performance warnings. No action required unless broader query expressiveness is desired.

---

## Section 3: Element Operators

### $exists

**GAP.** The `FilterOperator` enum does not include an `Exists` variant. The reference notes `$exists: true` can use sparse indexes, and `$exists: false` is expensive. Users cannot query for documents where a field is present or absent.

- **Fix needed (low priority):** Add `FilterOperator::Exists` and support it in `build_single_condition`. Add a query_advisor check warning about `$exists: false` performance.

### $type

**GAP.** Not supported. The reference documents BSON type checking. This is a niche operator; omission is acceptable for a natural-language query bot.

- **No action required.**

---

## Section 4: Evaluation Operators

### $regex

**COMPLIANT.** `FilterOperator::Regex` is supported (executor.rs:206-211). The `$options: "i"` flag is always applied. The query advisor warns about unanchored regex (code `REGEX_NO_ANCHOR`, query_advisor.rs:117-136). The `Contains` operator properly escapes regex special characters via `regex_escape()`.

- **GAP: No REGEX_CASE_INSENSITIVE info.** The reference (Section 14, rule `REGEX_CASE_INSENSITIVE`) recommends an info-level notice when the `i` flag is used, since it prevents index prefix optimization. The current code always applies `"$options": "i"` but does not inform the user about the index implication.
  - **Fix needed:** Add an info-level check in `query_advisor.rs` for `FilterOperator::Regex` noting that the `i` flag disables index prefix optimization. Alternatively, only apply `"i"` for `Contains` (which is inherently case-insensitive) and let `Regex` respect the pattern as-is.

- **GAP: No NOT_WITH_REGEX detection.** The reference (Section 14, rule `NOT_WITH_REGEX`) flags `$not` combined with `$regex` as an error. Since `$not` is not supported in the model, this cannot currently occur, but there is no guard against it appearing in raw aggregation pipelines.

### $expr

**GAP.** Not supported. The reference strongly recommends `$expr` over `$where` for comparing two fields in the same document. The safety checker blocks `$expr` in field names (safety.rs:175), which is appropriate for preventing injection, but there is no legitimate path to use `$expr` for cross-field comparisons.

- **Fix needed (medium priority):** For more advanced queries, consider allowing the LLM to generate `$expr`-based conditions in aggregation pipelines, with appropriate safety validation.

### $where

**COMPLIANT.** The safety checker correctly blocks `$where` in field names (safety.rs:175). This aligns with the reference's strong recommendation to avoid `$where`.

---

## Section 5: Array Operators

### $elemMatch

**COMPLIANT.** The executor automatically uses `$elemMatch` when multiple filters target sub-fields of the same array (executor.rs:245-261). This correctly addresses the reference's warning about dot notation matching across different array elements (Section 11, Mistake 8). The query advisor also informs users when `$elemMatch` is being used (code `ELEMMATCH_USED`, query_advisor.rs:297-321).

The LLM prompt instructs: "For array fields use dot notation... when filtering on multiple sub-fields of the same array, list each as a separate filter." This guides the LLM to produce the right structure that triggers automatic `$elemMatch` grouping. Well designed.

### $all

**GAP.** Not supported. The reference shows `$all` for matching arrays containing all specified values. Users asking "find articles tagged with both mongodb and database" cannot be served.

- **Fix needed (low priority):** Add `FilterOperator::All` and support it in `build_single_condition`.

### $size

**GAP.** Not supported. The reference notes `$size` only supports exact match (no ranges), which limits utility. Low priority.

---

## Section 6: Aggregation Pipeline Stages

### $match

**COMPLIANT.** The executor places `$match` as the first stage in aggregation pipelines (executor.rs:98), matching the reference's best practice.

- **GAP: No MATCH_NOT_FIRST warning.** The reference (Section 14, rule `MATCH_NOT_FIRST`) recommends warning when `$match` is not at the beginning of a pipeline. Since the executor auto-constructs pipelines with `$match` first, this cannot occur in auto-generated pipelines. However, if raw pipeline support is ever added, this check would be needed.

### $group

**GAP.** The aggregation pipeline builder in the executor (executor.rs:93-125) only supports `$match`, `$sort`, `$limit`, and `$project`. There is no `$group` stage support. Users asking "what is the average order total by customer" or "count orders per status" cannot be served with proper aggregation.

- **Fix needed (high priority):** The aggregation pipeline is very limited. Consider either:
  1. Extending `ParsedQuery` to include a `group_by` field and `accumulators` list, or
  2. Allowing the LLM to output raw pipeline stages with safety validation.

### $sort

**COMPLIANT.** Supported in both find and aggregate operations. The query advisor warns about sort without index (code `SORT_NO_INDEX`).

- **GAP: No 100MB memory limit warning.** The reference notes `$sort` has a 100MB memory limit. No warning is emitted for potentially large in-memory sorts.
  - **Fix needed (low priority):** When sort is used on an unindexed field AND no limit is set (or limit is very high), add a warning about the 100MB memory limit and suggest `allowDiskUse: true`.

### $project

**COMPLIANT.** Projections are supported in both find (executor.rs:35-43) and aggregate (executor.rs:107-113). The query advisor warns about missing projections (code `MISSING_PROJECTION`).

- **GAP: PROJECT_EARLY not checked.** The reference (Section 14) warns about placing `$project` early in a pipeline. The executor places `$project` last in its auto-generated pipeline, which is correct. But no advisory check exists if this pattern were violated.

### $limit / $skip

**PARTIALLY COMPLIANT.** `$limit` is supported. `$skip` is not implemented.

- **GAP: No SKIP_LARGE_OFFSET warning.** The reference (Section 14, rule `SKIP_LARGE_OFFSET`) recommends flagging large `$skip` values and suggesting range-based pagination. Since `$skip` is not supported, this is moot for now.
- **GAP: No range-based pagination support.** The reference recommends cursor-based pagination for large datasets. The query advisor mentions it in the `LARGE_LIMIT` warning text, which is appropriate.

### $unwind, $lookup, $facet, $bucket

**GAP.** None of these stages are supported in the aggregation pipeline builder. This significantly limits aggregation expressiveness.

- **Fix needed (medium priority for $lookup and $unwind):** These are common aggregation patterns. Without them, cross-collection joins and array flattening are not possible.

### $unwind + $group anti-pattern

**GAP: No UNWIND_GROUP_ANTI check.** The reference (Section 14, rule `UNWIND_GROUP_ANTI`) recommends detecting `$unwind` followed by `$group` and suggesting array operators instead. Since neither stage is currently supported, this is not applicable yet.

---

## Section 7: Index Best Practices

### ESR Rule

**GAP.** The reference documents the ESR (Equality-Sort-Range) rule for compound index design. The `analyze_index_usage` function in executor.rs (line 412-520) checks whether query fields match existing indexes using leftmost prefix matching, which is correct. However:

- **GAP: No ESR_VIOLATION check.** The reference (Section 14, rule `ESR_VIOLATION`) recommends checking if the compound index field order follows ESR. The current `analyze_index_usage` only checks if fields appear in an index, not whether the index order is optimal for the query pattern.
  - **Fix needed:** In `executor.rs::analyze_index_usage()`, when a matching compound index is found, verify that equality filter fields appear before the sort field, and range filter fields appear after the sort field. Emit an info if violated.

### Covered Queries

**GAP.** The reference describes covered queries (queries answered entirely from the index). No check exists to detect or suggest covered query opportunities. Low priority.

### Index Selectivity

**GAP.** The reference recommends placing the most selective (highest cardinality) field first in compound indexes. No check exists. Low priority -- this is an advanced optimization that requires runtime statistics.

---

## Section 8: Query Optimization

### Projections

**COMPLIANT.** The query advisor warns about missing projections (code `MISSING_PROJECTION`, query_advisor.rs:102-113). The LLM prompt does not explicitly instruct the model to include projections, but the JSON format shows a `projection` field.

- **GAP (minor):** The LLM prompt could be more explicit about recommending projections. Consider adding to the prompt rules: "Include a projection array with only the fields needed to answer the question."

### Limit and BatchSize

**COMPLIANT.** Default limit of 100 is set via `default_limit()` in models.rs:46. The LLM prompt states "Default limit 100". The query advisor warns about large limits (code `LARGE_LIMIT`, threshold 5000) and unbounded queries (code `UNBOUNDED_QUERY`). The safety checker enforces a hard maximum via `max_rows` (safety.rs:53-57). The MongoDB find operation sets `max_time(Duration::from_secs(30))` as a server-side timeout (mongo.rs:67).

### Hint for Index Selection

**GAP.** Not supported, and the reference says to use sparingly. No action needed.

---

## Section 9: Write Operation Safety

### Empty Filter Writes

**COMPLIANT.** The safety checker rejects updates without filters (safety.rs:86-91) and deletes without filters (safety.rs:103-109). The query advisor also flags these as errors (codes `UPDATE_NO_FILTER`, `DELETE_NO_FILTER`).

### updateMany/deleteMany Warnings

**COMPLIANT.** The query advisor warns about `updateMany` (code `UPDATE_MANY`) and `deleteMany` (code `DELETE_MANY`) operations (query_advisor.rs:325-375).

### $set Wrapping for Updates

**COMPLIANT.** The `mongo.rs::update_documents()` method wraps the update document in `$set` automatically (mongo.rs:217). This prevents accidental `replaceOne` behavior as described in the reference's Replace vs Update section.

### Insert Validation

**COMPLIANT.** The safety checker requires a non-empty document for inserts (safety.rs:73-83).

### Write Confirmation Flow

**COMPLIANT.** The `QueryResponse` model includes `requires_confirmation: bool` (models.rs:253), enabling a two-step write flow.

### bulkWrite

**GAP.** The reference (Section 9, Section 13 Mistake 12, Section 14 rule `WRITE_LOOP`) strongly recommends `bulkWrite` for batch operations. The system only supports single-document writes (insertOne, updateOne/Many, deleteOne/Many). No `bulkWrite` support exists.

- **Fix needed (low priority):** Not critical for a Slack bot that processes one query at a time, but worth noting for future batch operations.

### Write Concern

**GAP.** The reference (Section 9, Section 13 Mistake 13, Section 14 rule `W0_CRITICAL_DATA`) discusses write concern configuration. The implementation uses default write concern (w:1) for all operations. No ability to specify write concern.

- **Fix needed (low priority):** For a Slack query bot, default write concern is appropriate. No action required unless the bot is used for critical financial data.

### Replace vs Update

**COMPLIANT.** The system always uses `$set`-based updates via `update_documents` (mongo.rs:217), never `replaceOne`. This inherently prevents the reference's Mistake 14.

---

## Section 10: Date Query Patterns

### BSON Date Storage and Conversion

**COMPLIANT.** The `json_to_bson` function in executor.rs (line 268-303) detects ISO 8601 date strings and converts them to BSON DateTime via `try_parse_datetime`. Supported formats include:
- Full ISO 8601: "2026-01-01T00:00:00Z"
- Date-only: "2026-01-01"
- Bare years: "2010" (converted to Jan 1 midnight)
- With timezone offsets: "2026-01-01T00:00:00+00:00"

Additionally, `coerce_value_for_field` (executor.rs:526-548) detects date-like field names and converts numeric years to BSON DateTime.

The LLM prompt correctly instructs: "For date fields always use ISO 8601 format... never bare years."

### Date Range Queries

**PARTIALLY COMPLIANT.** Range operators ($gt, $gte, $lt, $lte) work correctly with date values that get converted to BSON DateTime. However:

- **GAP: No DATE_AS_STRING warning.** The reference (Section 14, rule `DATE_AS_STRING`) recommends warning when date comparisons are performed on string fields. The query advisor does not check whether a field known to be a string type is being compared with date-like values, or vice versa. The `coerce_value_for_field` function helps mitigate this by auto-converting, but if a field stores dates as strings in MongoDB, the advisor should warn.
  - **Fix needed:** In `query_advisor.rs`, add a check: if a filter targets a field whose schema type is "string" but the value looks like a date (or the field name suggests a date), emit a warning with code `DATE_AS_STRING`.

### Aggregation Date Operators

**GAP.** No support for `$year`, `$month`, `$dateToString`, etc. in the aggregation pipeline. This limits date-based grouping and analysis queries.

---

## Section 11: Array Query Patterns

### $elemMatch Auto-Detection

**COMPLIANT.** As covered in Section 5, the executor automatically groups multi-field array conditions into `$elemMatch`. The LLM prompt guides the model to use dot notation for array sub-fields.

### Positional Operators for Array Updates

**GAP.** The `$`, `$[]`, and `$[identifier]` positional operators are not supported. Updates to specific array elements are not possible.

- **Fix needed (low priority):** This is an advanced feature. For the current scope of the bot, this is acceptable.

---

## Section 12: Text Search

### $text Queries

**GAP.** No `$text` search support exists. The query advisor's `TEXT_SEARCH_HINT` (query_advisor.rs:274-293) correctly suggests that text indexes may perform better than regex for large text fields, but there is no path to actually use `$text` queries.

- **Fix needed (medium priority):** Add a `FilterOperator::Text` or a dedicated `text_search` field to `ParsedQuery`. This would enable efficient full-text search for users asking questions like "search for articles about mongodb."

---

## Section 13: Common Mistakes

| Mistake | Status | Notes |
|---------|--------|-------|
| 1. Missing limit | **COMPLIANT** | Default limit 100, advisor warns for unbounded queries |
| 2. Collection scan | **PARTIALLY COMPLIANT** | `analyze_index_usage` detects unindexed fields, but no explicit COLLSCAN check |
| 3. Regex without anchor | **COMPLIANT** | Advisor code `REGEX_NO_ANCHOR` |
| 4. Large $in arrays | **GAP** | No `LARGE_IN_ARRAY` check |
| 5. $where usage | **COMPLIANT** | Safety checker blocks `$where` |
| 6. Fetching entire docs | **COMPLIANT** | Advisor code `MISSING_PROJECTION` |
| 7. Wrong compound index order | **GAP** | No ESR violation check |
| 8. Dot notation vs $elemMatch | **COMPLIANT** | Auto-$elemMatch grouping in executor |
| 9. updateMany/deleteMany without filter | **COMPLIANT** | Safety checker + advisor both flag |
| 10. Dates as strings | **GAP** | No `DATE_AS_STRING` warning |
| 11. $unwind + $group anti-pattern | **GAP** | Not applicable (stages not supported) |
| 12. Sequential writes vs bulkWrite | **GAP** | No bulkWrite support |
| 13. Write concern for critical data | **GAP** | No write concern configuration |
| 14. replaceOne vs updateOne | **COMPLIANT** | Always uses `$set` |
| 15. Negation on unindexed fields | **COMPLIANT** | Advisor warns for $ne and $nin |

---

## Section 14: Validation Rules Summary

Cross-reference of all rules from the reference against implementation:

| Rule ID | Ref Severity | Implemented? | Implementation Location | Notes |
|---------|-------------|--------------|------------------------|-------|
| `NO_LIMIT` | warning | Yes | query_advisor.rs `UNBOUNDED_QUERY` | Triggers when no filters AND limit > 1000 |
| `COLLSCAN` | error | Partial | executor.rs `analyze_index_usage` | Reports unindexed fields but does not use the term "COLLSCAN" |
| `REGEX_NO_ANCHOR` | warning | Yes | query_advisor.rs `REGEX_NO_ANCHOR` | Exact match |
| `REGEX_CASE_INSENSITIVE` | info | **No** | -- | Not checked; code always applies `"i"` flag |
| `NOT_WITH_REGEX` | error | N/A | -- | `$not` not supported, so cannot occur |
| `WHERE_USAGE` | error | Yes | safety.rs `check_injection` | Blocks `$where` in field names |
| `LARGE_IN_ARRAY` | warning | **No** | -- | Not checked |
| `NEGATION_OPERATOR` | info | Yes | query_advisor.rs `NE_OPERATOR`, `NIN_OPERATOR` | Implemented as warnings (stricter than ref's info) |
| `EMPTY_FILTER_WRITE` | error | Yes | safety.rs + query_advisor.rs | Both check this |
| `MISSING_PROJECTION` | info | Yes | query_advisor.rs `MISSING_PROJECTION` | Exact match |
| `DATE_AS_STRING` | warning | **No** | -- | Not checked |
| `DOT_NOTATION_ARRAY` | warning | Mitigated | executor.rs auto-$elemMatch | Auto-corrected rather than warned about |
| `UNWIND_GROUP_ANTI` | info | N/A | -- | Stages not supported |
| `SORT_NO_INDEX` | warning | Yes | query_advisor.rs `SORT_NO_INDEX` | Exact match |
| `LOOKUP_NO_INDEX` | warning | N/A | -- | $lookup not supported |
| `ESR_VIOLATION` | info | **No** | -- | Not checked |
| `REPLACE_VS_UPDATE` | warning | Mitigated | mongo.rs always uses `$set` | Anti-pattern prevented by design |
| `WRITE_LOOP` | warning | N/A | -- | Not applicable (single-query bot) |
| `SKIP_LARGE_OFFSET` | info | N/A | -- | $skip not supported |
| `UNBOUNDED_ARRAY` | warning | **No** | -- | Not checked |
| `W0_CRITICAL_DATA` | warning | N/A | -- | No write concern configuration |
| `PROJECT_EARLY` | info | Mitigated | executor.rs places `$project` last | Correct by construction |
| `MATCH_NOT_FIRST` | warning | Mitigated | executor.rs places `$match` first | Correct by construction |
| `SIZE_RANGE` | info | N/A | -- | `$size` not supported |

---

## LLM Prompt Review

**File:** `src/infra/llm.rs`, function `build_prompt` (line 134-155).

### Strengths

1. Clearly specifies the JSON output format with required fields.
2. Instructs use of ISO 8601 dates, not bare years.
3. Instructs use of dot notation for array fields with separate filters per sub-field (enabling auto-$elemMatch).
4. Specifies default limit of 100.
5. Instructs "Use only fields from schema" which prevents hallucinated field names.
6. Instructs "Prefer indexed fields for filters/sort."

### Gaps

1. **GAP: No `$or` guidance.** The prompt does not mention `$or` or how to express disjunctive conditions.

2. **GAP: No projection guidance.** The prompt shows `"projection": []` in the format but does not instruct the LLM to include only needed fields. Adding "Include only the fields necessary to answer the question in the projection array" would improve query efficiency.

3. **GAP: No aggregation guidance.** The prompt does not explain when to use `"operation": "aggregate"` vs `"find"`. It does not describe how to express group-by, sum, average, or other aggregation patterns. The aggregate operation exists in the enum but the LLM has no instructions on when or how to use it.

4. **GAP: Missing `delete` in operation list.** The prompt says `"operation": "find|count|distinct|insert|update"` but omits `"delete"` and `"aggregate"`. The `QueryOperation` enum includes both. The LLM may not know to use these operations.
   - **Fix needed:** Update the operation list in the prompt to: `"find|count|distinct|aggregate|insert|update|delete"`.

5. **GAP: No `update_many`/`delete_many` guidance.** The prompt does not tell the LLM when to set `"update_many": true` or `"delete_many": true`. Defaults to false (single-document operations), which is safe but may not match user intent when they say "delete all expired sessions."

6. **GAP: No `regex` operator in prompt.** The prompt lists operators as `"eq|ne|gt|gte|lt|lte|in|contains"` but omits `"regex"`, `"nin"`. The model supports these operators in the enum but the LLM may not know to use them.
   - **Fix needed:** Update the operator list to: `"eq|ne|gt|gte|lt|lte|in|nin|contains|regex"`.

---

## Priority Summary

### FIX NEEDED (Active issues)

| # | Issue | File | Priority |
|---|-------|------|----------|
| 1 | Same-field filter overwrite in `build_filter` (silent $and bug) | executor.rs | **High** |
| 2 | LLM prompt omits `delete`, `aggregate` from operation list | llm.rs | **High** |
| 3 | LLM prompt omits `nin`, `regex` from operator list | llm.rs | **High** |
| 4 | No `LARGE_IN_ARRAY` warning for $in > 200 values | query_advisor.rs | **Medium** |
| 5 | No `DATE_AS_STRING` warning | query_advisor.rs | **Medium** |
| 6 | No `REGEX_CASE_INSENSITIVE` info | query_advisor.rs | **Low** |

### GAP (Missing features recommended by reference)

| # | Feature | Impact | Priority |
|---|---------|--------|----------|
| 1 | No `$or` logical operator support | Users cannot express OR conditions | **High** |
| 2 | Limited aggregation ($group, $unwind, $lookup missing) | Cannot serve analytics queries | **High** |
| 3 | No `$text` search support | Cannot leverage text indexes | **Medium** |
| 4 | No `$expr` support for cross-field comparisons | Cannot compare two fields | **Medium** |
| 5 | No `$exists` operator | Cannot check field presence | **Low** |
| 6 | No `$all` array operator | Cannot match "contains all" | **Low** |
| 7 | No ESR rule validation | Cannot advise on compound index order | **Low** |
| 8 | No projection guidance in LLM prompt | LLM often returns empty projections | **Low** |
| 9 | No `update_many`/`delete_many` guidance in LLM prompt | Defaults to single-doc operations | **Low** |

### COMPLIANT (No action needed)

- Comparison operators ($eq, $ne, $gt, $gte, $lt, $lte, $in, $nin)
- Filter-to-BSON conversion with date coercion
- $elemMatch auto-grouping for array sub-field queries
- Regex escaping for `contains` operator
- Unanchored regex warning
- Negation operator warnings ($ne, $nin)
- Sort-without-index warning
- Missing projection advisory
- Unbounded query warning
- Large limit warning
- Text search hint (regex -> text index suggestion)
- Write safety: empty filter rejection, updateMany/deleteMany warnings
- $set wrapping prevents replaceOne accidents
- $where blocked by safety checker
- Injection pattern detection
- Field validation against schema
- Operation allowlist in safety checker
- Limit enforcement (hard max)
- Query timeout (30s max_time in MongoDB)
- Default limit of 100
- $match-first and $project-last in auto-generated pipelines
