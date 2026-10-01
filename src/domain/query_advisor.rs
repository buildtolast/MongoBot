use crate::domain::models::{
    CollectionSchema, FilterOperator, IndexInfo, ParsedQuery, QueryOperation,
};

/// Query analysis result with warnings and optimization suggestions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryAdvice {
    /// Severity: "info", "warning", "error"
    pub severity: String,
    /// Short code for the advice type
    pub code: String,
    /// Human-readable message
    pub message: String,
}

impl QueryAdvice {
    fn info(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "info".to_string(),
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "warning".to_string(),
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "error".to_string(),
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// Analyze a parsed query for potential issues and optimization opportunities.
pub fn analyze_query(
    query: &ParsedQuery,
    schema: Option<&CollectionSchema>,
) -> Vec<QueryAdvice> {
    let mut advice = Vec::new();

    check_unbounded_query(query, &mut advice);
    check_large_limit(query, &mut advice);
    check_missing_projection(query, &mut advice);
    check_regex_without_anchor(query, &mut advice);
    check_not_equals_performance(query, &mut advice);
    check_case_sensitivity(query, schema, &mut advice);
    check_empty_filters_on_large_collection(query, schema, &mut advice);
    check_filter_no_index(query, schema, &mut advice);
    check_sort_without_index(query, schema, &mut advice);
    check_contains_on_large_field(query, &mut advice);
    check_multiple_array_filters(query, &mut advice);
    check_write_safety(query, &mut advice);
    check_distinct_without_index(query, schema, &mut advice);
    check_large_in_array(query, &mut advice);
    check_same_field_range(query, &mut advice);
    check_aggregate_no_match(query, &mut advice);
    check_lookup_performance(query, &mut advice);
    check_lookup_always_matches(query, &mut advice);
    check_unwind_warning(query, &mut advice);
    check_or_query(query, &mut advice);
    check_group_high_cardinality(query, &mut advice);
    check_raw_passthrough(query, &mut advice);
    check_destructive_pipeline_stages(query, &mut advice);

    advice
}

/// Warn if $in operator has more than 200 values (performance degrades).
fn check_large_in_array(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    for filter in &query.filters {
        if filter.operator == FilterOperator::In || filter.operator == FilterOperator::Nin {
            if let Some(arr) = filter.value.as_array() {
                if arr.len() > 200 {
                    advice.push(QueryAdvice {
                        severity: "warning".into(),
                        code: "LARGE_IN_ARRAY".into(),
                        message: format!(
                            "$in on '{}' has {} values. Large arrays in $in degrade performance — consider restructuring the query or using a lookup collection.",
                            filter.field, arr.len()
                        ),
                    });
                }
            }
        }
    }
}

/// Info when multiple filters target the same field (range query).
fn check_same_field_range(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    let mut field_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for f in &query.filters {
        *field_counts.entry(&f.field).or_default() += 1;
    }
    for (field, count) in field_counts {
        if count > 1 {
            advice.push(QueryAdvice {
                severity: "info".into(),
                code: "RANGE_QUERY".into(),
                message: format!(
                    "Multiple conditions on '{}' detected — this creates a range query. Ensure an index exists on this field for optimal performance.",
                    field
                ),
            });
        }
    }
}

/// Warn if a find/aggregate has no filters and no limit (or limit > 1000).
/// This risks scanning an entire collection.
fn check_unbounded_query(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    let is_read = matches!(
        query.operation,
        QueryOperation::Find | QueryOperation::Aggregate
    );
    if !is_read {
        return;
    }
    if query.filters.is_empty() && query.limit > 1000 {
        advice.push(QueryAdvice::warning(
            "UNBOUNDED_QUERY",
            format!(
                "Query on '{}' has no filters and a limit of {}. \
                 This may scan the entire collection. Add filters or reduce the limit.",
                query.collection, query.limit
            ),
        ));
    }
}

/// Warn if limit exceeds 5000 — suggest cursor-based pagination instead.
fn check_large_limit(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if query.limit > 5000 {
        advice.push(QueryAdvice::warning(
            "LARGE_LIMIT",
            format!(
                "Limit of {} is very high. Consider cursor-based pagination \
                 to reduce memory usage and response time.",
                query.limit
            ),
        ));
    }
}

/// Info if a find has an empty projection (returns all fields).
fn check_missing_projection(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if query.operation != QueryOperation::Find {
        return;
    }
    if query.projection.is_empty() && !query.filters.is_empty() {
        advice.push(QueryAdvice::info(
            "MISSING_PROJECTION",
            "No projection specified — all fields will be returned. \
             Adding a projection reduces data transfer and can improve performance.",
        ));
    }
}

/// Warn if a regex filter value does not start with '^'.
/// Without an anchored prefix MongoDB cannot use an index prefix scan.
fn check_regex_without_anchor(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    for filter in &query.filters {
        if filter.operator != FilterOperator::Regex {
            continue;
        }
        if let Some(pattern) = filter.value.as_str() {
            if !pattern.starts_with('^') {
                advice.push(QueryAdvice::warning(
                    "REGEX_NO_ANCHOR",
                    format!(
                        "Regex on '{}' does not start with '^'. \
                         An unanchored regex cannot use an index prefix and will \
                         trigger a collection scan. Anchor with '^' if possible.",
                        filter.field
                    ),
                ));
            }
        }
    }
}

/// Warn when $ne or $nin operators are used — they cannot use indexes efficiently
/// and typically require a full collection scan.
fn check_not_equals_performance(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    for filter in &query.filters {
        match filter.operator {
            FilterOperator::Ne => {
                advice.push(QueryAdvice::warning(
                    "NE_OPERATOR",
                    format!(
                        "Not-equals ($ne) on '{}' cannot use indexes efficiently \
                         and may cause a collection scan. Consider restructuring \
                         the query to use positive matches ($in, $eq) if possible.",
                        filter.field
                    ),
                ));
            }
            FilterOperator::Nin => {
                advice.push(QueryAdvice::warning(
                    "NIN_OPERATOR",
                    format!(
                        "Not-in ($nin) on '{}' cannot use indexes efficiently \
                         and may cause a collection scan. Consider using $in with \
                         the desired values instead.",
                        filter.field
                    ),
                ));
            }
            _ => {}
        }
    }
}

/// Warn if filtering string fields with exact equality — the value may not match case.
/// Only fires when schema confirms the field is a string type, or when no schema
/// is available but the filter value is a string.
fn check_case_sensitivity(
    query: &ParsedQuery,
    schema: Option<&CollectionSchema>,
    advice: &mut Vec<QueryAdvice>,
) {
    for filter in &query.filters {
        if filter.operator != FilterOperator::Eq {
            continue;
        }
        if !filter.value.is_string() {
            continue;
        }

        let is_string_field = schema
            .map(|s| {
                s.fields
                    .iter()
                    .any(|f| f.name == filter.field && f.field_type == "string")
            })
            .unwrap_or(true); // assume string when no schema

        if is_string_field {
            advice.push(QueryAdvice::info(
                "CASE_SENSITIVE_EQ",
                format!(
                    "Exact equality on string field '{}' is case-sensitive. \
                     If case-insensitive matching is intended, use 'contains' \
                     or 'regex' with the 'i' flag.",
                    filter.field
                ),
            ));
        }
    }
}

/// Warn if a read query has no filters on a collection with many fields (heuristic
/// for a large/wide collection where a full scan is expensive).
fn check_empty_filters_on_large_collection(
    query: &ParsedQuery,
    schema: Option<&CollectionSchema>,
    advice: &mut Vec<QueryAdvice>,
) {
    if !query.filters.is_empty() {
        return;
    }
    if query.operation.is_write() {
        return; // write safety is handled separately
    }

    let field_count = schema.map(|s| s.fields.len()).unwrap_or(0);
    if field_count >= 10 {
        advice.push(QueryAdvice::warning(
            "NO_FILTER_WIDE_COLLECTION",
            format!(
                "No filters on '{}' which has {} fields. \
                 Returning full documents from a wide collection increases \
                 data transfer. Add filters or a projection to limit the data.",
                query.collection, field_count
            ),
        ));
    }
}

/// Check filter fields against existing indexes using the ESR (Equality → Sort → Range) strategy.
///
/// ESR compound index key ordering:
///   1. Equality fields first  — `$eq`, `$in` (bounded discrete set)
///   2. Sort field next        — matches MongoDB's in-order traversal
///   3. Range fields last      — `$gt/$lt/$gte/$lte`, `$ne/$nin`, regex, contains
///
/// Three outcomes:
///   - All query fields are covered by an existing index in ESR order → no advice (index is fine).
///   - All fields are covered but in the wrong order → ESR_REWRITE (info): reorder, no new index.
///   - Fields are not covered by any existing index → FILTER_NO_INDEX (warning): suggest one
///     compound ESR index, not N individual single-field indexes.
fn check_filter_no_index(
    query: &ParsedQuery,
    schema: Option<&CollectionSchema>,
    advice: &mut Vec<QueryAdvice>,
) {
    if query.filters.is_empty() {
        return;
    }
    let schema = match schema {
        Some(s) => s,
        None => return,
    };

    // Only emit when schema has at least one real (non-_id) index — proves index info is populated.
    let non_id_indexes: Vec<&IndexInfo> = schema.indexes.iter()
        .filter(|i| i.name != "_id_")
        .collect();
    if non_id_indexes.is_empty() {
        return;
    }

    // ── Classify filter fields by ESR role ───────────────────────────────────────
    let mut eq_fields: Vec<&str> = Vec::new();
    let mut range_fields: Vec<&str> = Vec::new();

    for f in &query.filters {
        let field = f.field.as_str();
        match f.operator {
            FilterOperator::Eq | FilterOperator::In => {
                if !eq_fields.contains(&field) { eq_fields.push(field); }
            }
            FilterOperator::Gt | FilterOperator::Gte |
            FilterOperator::Lt | FilterOperator::Lte |
            FilterOperator::Ne | FilterOperator::Nin |
            FilterOperator::Regex | FilterOperator::Contains => {
                if !range_fields.contains(&field) { range_fields.push(field); }
            }
            _ => {} // Exists, All, Size, Type — not index-selective
        }
    }

    // Sort field sits between E and R in the compound key
    let sort_field = query.sort.as_ref().map(|s| s.field.as_str());

    // ── Build ideal ESR field order ───────────────────────────────────────────────
    let mut esr: Vec<&str> = Vec::new();
    for f in &eq_fields   { if !esr.contains(f) { esr.push(f); } }
    if let Some(sf) = sort_field {
        if !esr.contains(&sf) { esr.push(sf); }
    }
    for f in &range_fields { if !esr.contains(f) { esr.push(f); } }

    if esr.is_empty() {
        return;
    }

    // ── Check existing indexes ────────────────────────────────────────────────────
    // An index "covers" the query if its key list contains every ESR field (as a prefix or superset).
    let covering = non_id_indexes.iter().find(|idx| {
        let keys: Vec<&str> = idx.keys.iter().map(|k| k.field.as_str()).collect();
        esr.iter().all(|f| keys.contains(f))
    });

    match covering {
        Some(idx) => {
            // Index contains all fields — is it already in ESR order?
            let keys: Vec<&str> = idx.keys.iter().map(|k| k.field.as_str()).collect();
            let already_esr = keys.iter().take(esr.len())
                .zip(esr.iter())
                .all(|(a, b)| a == b);

            if !already_esr {
                // Existing index covers the query — no new index needed, just reorder
                advice.push(QueryAdvice::info(
                    "ESR_REWRITE",
                    format!(
                        "Index '{}' already covers all query fields. \
                         Reordering filters to ESR (Equality→Sort→Range: {}) \
                         lets MongoDB use it without creating a new index.",
                        idx.name,
                        esr.iter().map(|f| format!("'{f}'")).collect::<Vec<_>>().join(" → "),
                    ),
                ));
            }
            // Already in ESR order → no advice needed
        }
        None => {
            // No existing index covers the query — suggest one compound ESR index
            let esr_spec = esr.iter()
                .map(|f| format!("{f}: 1"))
                .collect::<Vec<_>>()
                .join(", ");
            advice.push(QueryAdvice::warning(
                "FILTER_NO_INDEX",
                format!(
                    "No index covers this query on '{}'. \
                     Consider a compound ESR (Equality→Sort→Range) index: \
                     db.{}.createIndex({{ {} }})",
                    schema.name, schema.name, esr_spec,
                ),
            ));
        }
    }
}

/// Warn if sorting on a field that has no supporting index.
fn check_sort_without_index(
    query: &ParsedQuery,
    schema: Option<&CollectionSchema>,
    advice: &mut Vec<QueryAdvice>,
) {
    let sort_field = match &query.sort {
        Some(s) => &s.field,
        None => return,
    };

    let schema = match schema {
        Some(s) => s,
        None => return, // can't check without schema
    };

    if schema.indexes.is_empty() {
        return; // already covered by index analysis
    }

    let sort_indexed = schema.indexes.iter().any(|idx| {
        idx.keys.iter().any(|k| k.field == *sort_field)
    });

    if !sort_indexed {
        advice.push(QueryAdvice::warning(
            "SORT_NO_INDEX",
            format!(
                "Sort on '{}' has no supporting index. MongoDB must load \
                 all matching documents into memory to sort. Consider adding \
                 an index on this field.",
                sort_field
            ),
        ));
    }
}

/// Info if using contains/regex on text fields — a text index may perform better.
fn check_contains_on_large_field(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    for filter in &query.filters {
        if matches!(filter.operator, FilterOperator::Contains | FilterOperator::Regex) {
            advice.push(QueryAdvice::info(
                "TEXT_SEARCH_HINT",
                format!(
                    "Using {op} on '{field}' performs a regex scan. \
                     For large text fields, a MongoDB text index with $text \
                     queries may provide significantly better performance.",
                    op = match filter.operator {
                        FilterOperator::Contains => "contains",
                        FilterOperator::Regex => "regex",
                        _ => unreachable!(),
                    },
                    field = filter.field
                ),
            ));
        }
    }
}

/// Info when multiple filters target fields under the same dotted prefix.
/// For array fields, the user may need $elemMatch (via raw_filter) for
/// same-element matching. For nested objects, dot notation works correctly.
fn check_multiple_array_filters(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    let mut prefix_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();

    for filter in &query.filters {
        if let Some(dot_pos) = filter.field.find('.') {
            let prefix = &filter.field[..dot_pos];
            *prefix_counts.entry(prefix).or_insert(0) += 1;
        }
    }

    for (prefix, count) in &prefix_counts {
        if *count >= 2 {
            advice.push(QueryAdvice::info(
                "DOTTED_PREFIX_GROUP",
                format!(
                    "Multiple filters on '{prefix}.*' ({count} conditions) use dot notation. \
                     If '{prefix}' is an array and you need same-element matching, use \
                     raw_filter with $elemMatch instead.",
                ),
            ));
        }
    }
}

/// Error if update/delete has no filters (would affect all documents).
/// Warn if update_many or delete_many is set (broader impact).
fn check_write_safety(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    match query.operation {
        QueryOperation::Update => {
            if query.filters.is_empty() {
                advice.push(QueryAdvice::error(
                    "UPDATE_NO_FILTER",
                    format!(
                        "Update on '{}' has no filters — this would modify \
                         every document in the collection. Add filters to target \
                         specific documents.",
                        query.collection
                    ),
                ));
            }
            if query.update_many {
                advice.push(QueryAdvice::warning(
                    "UPDATE_MANY",
                    format!(
                        "updateMany on '{}' will modify all matching documents. \
                         Ensure the filters are correct before confirming.",
                        query.collection
                    ),
                ));
            }
        }
        QueryOperation::Delete => {
            if query.filters.is_empty() {
                advice.push(QueryAdvice::error(
                    "DELETE_NO_FILTER",
                    format!(
                        "Delete on '{}' has no filters — this would remove \
                         every document in the collection. Add filters to target \
                         specific documents.",
                        query.collection
                    ),
                ));
            }
            if query.delete_many {
                advice.push(QueryAdvice::warning(
                    "DELETE_MANY",
                    format!(
                        "deleteMany on '{}' will remove all matching documents. \
                         Ensure the filters are correct before confirming.",
                        query.collection
                    ),
                ));
            }
        }
        _ => {}
    }
}

/// Warn if distinct is called on a field without an index.
fn check_distinct_without_index(
    query: &ParsedQuery,
    schema: Option<&CollectionSchema>,
    advice: &mut Vec<QueryAdvice>,
) {
    if query.operation != QueryOperation::Distinct {
        return;
    }

    // Determine the distinct target field (same logic as executor)
    let distinct_field = query
        .projection
        .first()
        .or_else(|| query.filters.first().map(|f| &f.field));

    let distinct_field = match distinct_field {
        Some(f) => f,
        None => return,
    };

    let schema = match schema {
        Some(s) => s,
        None => return,
    };

    let field_indexed = schema.indexes.iter().any(|idx| {
        idx.keys.iter().any(|k| k.field == *distinct_field)
    });

    if !field_indexed {
        advice.push(QueryAdvice::warning(
            "DISTINCT_NO_INDEX",
            format!(
                "distinct on '{}' has no supporting index. MongoDB must scan \
                 the entire collection to find unique values. An index on this \
                 field would allow a covered distinct scan.",
                distinct_field
            ),
        ));
    }
}

/// Warn when aggregate has $group but no $match — full collection scan before grouping.
fn check_aggregate_no_match(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if query.operation != QueryOperation::Aggregate {
        return;
    }
    if query.group_by.is_some() && query.filters.is_empty() && query.or_filters.is_empty() {
        advice.push(QueryAdvice::warning(
            "AGGREGATE_NO_MATCH",
            "Aggregate with $group but no $match — this will scan the entire collection before grouping. \
             Consider adding filters to reduce the working set.",
        ));
    }
}

/// Info when $lookup is used — cross-collection joins can be expensive.
fn check_lookup_performance(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if let Some(ref lookup) = query.lookup {
        advice.push(QueryAdvice::info(
            "LOOKUP_PERF",
            format!(
                "$lookup joins '{}' → '{}'. Cross-collection joins can be slow on large collections. \
                 Ensure '{}.{}' is indexed.",
                query.collection, lookup.from, lookup.from, lookup.foreign_field
            ),
        ));
    }
}

/// Warn when a raw pipeline filters a $lookup output with `$ne: null` or
/// `$exists: true`. $lookup always produces an array (empty when nothing
/// joins), so these conditions match every document. Use `{"$ne": []}` or
/// `{"<as>.0": {"$exists": true}}` instead.
fn check_lookup_always_matches(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    let Some(ref stages) = query.raw_pipeline else {
        return;
    };
    let mut join_outputs: Vec<&str> = Vec::new();
    for stage in stages {
        if let Some(as_name) = stage.pointer("/$lookup/as").and_then(|v| v.as_str()) {
            join_outputs.push(as_name);
            continue;
        }
        let Some(conditions) = stage.get("$match").and_then(|m| m.as_object()) else {
            continue;
        };
        for (field, cond) in conditions {
            if !join_outputs.contains(&field.as_str()) {
                continue;
            }
            let Some(ops) = cond.as_object() else { continue };
            let excludes_empty = ops.get("$ne").is_some_and(|v| v.as_array().is_some_and(|a| a.is_empty()))
                || ops.contains_key("$size")
                || ops.contains_key("$not");
            let never_null = ops.get("$ne").is_some_and(|v| v.is_null())
                || ops.get("$exists") == Some(&serde_json::Value::Bool(true));
            if never_null && !excludes_empty {
                advice.push(QueryAdvice::warning(
                    "LOOKUP_ALWAYS_MATCHES",
                    format!(
                        "'{field}' is a $lookup output, which is always an array (empty when nothing \
                         joins), so this $match keeps every document. Use {{\"{field}\": {{\"$ne\": []}}}} \
                         to keep only documents with a match."
                    ),
                ));
            }
        }
    }
}

/// Info when $unwind is used — array flattening can multiply document count.
fn check_unwind_warning(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if let Some(ref field) = query.unwind {
        advice.push(QueryAdvice::info(
            "UNWIND_ARRAY",
            format!(
                "$unwind on '{}' will create one output document per array element. \
                 Large arrays may cause significant result expansion.",
                field
            ),
        ));
    }
}

/// Info when $or is used — each branch may need its own index.
fn check_or_query(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if !query.or_filters.is_empty() {
        advice.push(QueryAdvice::info(
            "OR_QUERY",
            format!(
                "$or with {} branches — MongoDB evaluates each branch independently. \
                 Each branch should have its own supporting index for optimal performance.",
                query.or_filters.len()
            ),
        ));
    }
}

/// Info when grouping by a high-cardinality field like _id.
fn check_group_high_cardinality(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if let Some(ref group) = query.group_by {
        if group.field == "_id" {
            advice.push(QueryAdvice::info(
                "GROUP_HIGH_CARDINALITY",
                "Grouping by '_id' produces one group per document — this is usually not useful. \
                 Did you mean to group by a different field?",
            ));
        }
    }
}

/// Warning when destructive pipeline stages ($out, $merge) are detected — user must take backup.
fn check_destructive_pipeline_stages(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    let stages = match query.raw_pipeline {
        Some(ref s) => s,
        None => return,
    };

    for stage in stages {
        let obj = match stage.as_object() {
            Some(o) => o,
            None => continue,
        };
        for key in obj.keys() {
            let lower = key.to_lowercase();
            if lower == "$out" {
                advice.push(QueryAdvice::error(
                    "DESTRUCTIVE_OUT",
                    "WARNING: $out will REPLACE the entire target collection with pipeline results. \
                     Take a full backup of the target collection BEFORE executing this query. \
                     This operation is irreversible.",
                ));
            } else if lower == "$merge" {
                advice.push(QueryAdvice::warning(
                    "DESTRUCTIVE_MERGE",
                    "WARNING: $merge will INSERT or UPDATE documents in the target collection. \
                     Take a backup of the target collection BEFORE executing this query. \
                     Existing documents may be overwritten depending on the whenMatched strategy.",
                ));
            }
        }
    }
}

/// Info when raw pipeline or raw filter is used — structured validation is limited.
fn check_raw_passthrough(query: &ParsedQuery, advice: &mut Vec<QueryAdvice>) {
    if query.raw_pipeline.is_some() {
        advice.push(QueryAdvice::info(
            "RAW_PIPELINE_USED",
            "Query uses a raw aggregate pipeline — structured field validation is limited. \
             Review the pipeline stages manually for correctness.",
        ));
    }
    if query.raw_filter.is_some() {
        advice.push(QueryAdvice::info(
            "RAW_FILTER_USED",
            "Query uses a raw filter document — structured field validation is limited. \
             Review the filter for correctness.",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::*;

    fn make_find(filters: Vec<Filter>, limit: usize) -> ParsedQuery {
        ParsedQuery {
            collection: "test".to_string(),
            operation: QueryOperation::Find,
            filters,
            limit,
            ..Default::default()
        }
    }

    fn string_filter(field: &str, op: FilterOperator, value: &str) -> Filter {
        Filter {
            field: field.to_string(),
            operator: op,
            value: serde_json::Value::String(value.to_string()),
        }
    }

    fn num_filter(field: &str, op: FilterOperator, value: i64) -> Filter {
        Filter {
            field: field.to_string(),
            operator: op,
            value: serde_json::json!(value),
        }
    }

    fn make_schema(field_count: usize, indexes: Vec<IndexInfo>) -> CollectionSchema {
        let fields: Vec<FieldDef> = (0..field_count)
            .map(|i| FieldDef {
                name: format!("field_{}", i),
                field_type: "string".to_string(),
                description: None,
            })
            .collect();
        CollectionSchema {
            name: "test".to_string(),
            description: String::new(),
            fields,
            indexes,
            source: "yaml".to_string(),
        }
    }

    fn index_on(field: &str) -> IndexInfo {
        IndexInfo {
            name: format!("{}_1", field),
            keys: vec![IndexKey {
                field: field.to_string(),
                direction: 1,
            }],
            unique: false,
        }
    }

    fn compound_index(fields: &[&str]) -> IndexInfo {
        let name = fields.iter().map(|f| format!("{f}_1")).collect::<Vec<_>>().join("_");
        IndexInfo {
            name,
            keys: fields.iter().map(|f| IndexKey { field: f.to_string(), direction: 1 }).collect(),
            unique: false,
        }
    }

    // ---- check_unbounded_query ----

    #[test]
    fn unbounded_query_warns_no_filters_high_limit() {
        let q = make_find(vec![], 5000);
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "UNBOUNDED_QUERY"));
    }

    #[test]
    fn bounded_query_no_warning_with_low_limit() {
        let q = make_find(vec![], 100);
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "UNBOUNDED_QUERY"));
    }

    #[test]
    fn bounded_query_no_warning_with_filters() {
        let q = make_find(
            vec![num_filter("age", FilterOperator::Gt, 25)],
            5000,
        );
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "UNBOUNDED_QUERY"));
    }

    // ---- check_large_limit ----

    #[test]
    fn large_limit_warns() {
        let q = make_find(vec![], 10000);
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "LARGE_LIMIT"));
    }

    #[test]
    fn normal_limit_no_warning() {
        let q = make_find(vec![], 100);
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "LARGE_LIMIT"));
    }

    // ---- check_missing_projection ----

    #[test]
    fn missing_projection_info_when_filters_present() {
        let q = make_find(
            vec![num_filter("age", FilterOperator::Gt, 25)],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "MISSING_PROJECTION"));
    }

    #[test]
    fn missing_projection_no_info_when_no_filters() {
        // No filters means it's a browse query; projection advice is less useful
        let q = make_find(vec![], 100);
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "MISSING_PROJECTION"));
    }

    #[test]
    fn has_projection_no_info() {
        let mut q = make_find(
            vec![num_filter("age", FilterOperator::Gt, 25)],
            100,
        );
        q.projection = vec!["name".to_string(), "age".to_string()];
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "MISSING_PROJECTION"));
    }

    // ---- check_regex_without_anchor ----

    #[test]
    fn unanchored_regex_warns() {
        let q = make_find(
            vec![string_filter("name", FilterOperator::Regex, "smith")],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "REGEX_NO_ANCHOR"));
    }

    #[test]
    fn anchored_regex_no_warning() {
        let q = make_find(
            vec![string_filter("name", FilterOperator::Regex, "^smith")],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "REGEX_NO_ANCHOR"));
    }

    // ---- check_not_equals_performance ----

    #[test]
    fn ne_operator_warns() {
        let q = make_find(
            vec![string_filter("status", FilterOperator::Ne, "deleted")],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "NE_OPERATOR"));
    }

    #[test]
    fn nin_operator_warns() {
        let q = make_find(
            vec![Filter {
                field: "status".to_string(),
                operator: FilterOperator::Nin,
                value: serde_json::json!(["deleted", "archived"]),
            }],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "NIN_OPERATOR"));
    }

    // ---- check_case_sensitivity ----

    #[test]
    fn case_sensitive_eq_on_string_field_warns() {
        let q = make_find(
            vec![string_filter("name", FilterOperator::Eq, "Alice")],
            100,
        );
        let schema = make_schema(2, vec![]);
        // Without schema — assumes string, should still fire
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "CASE_SENSITIVE_EQ"));

        // With schema confirming string type
        let mut schema_with_name = schema;
        schema_with_name.fields.push(FieldDef {
            name: "name".to_string(),
            field_type: "string".to_string(),
            description: None,
        });
        let advice = analyze_query(&q, Some(&schema_with_name));
        assert!(advice.iter().any(|a| a.code == "CASE_SENSITIVE_EQ"));
    }

    #[test]
    fn eq_on_number_no_case_warning() {
        let q = make_find(
            vec![num_filter("age", FilterOperator::Eq, 25)],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "CASE_SENSITIVE_EQ"));
    }

    #[test]
    fn eq_on_non_string_field_type_no_warning() {
        let q = make_find(
            vec![string_filter("year", FilterOperator::Eq, "2020")],
            100,
        );
        let mut schema = make_schema(0, vec![]);
        schema.fields.push(FieldDef {
            name: "year".to_string(),
            field_type: "number".to_string(),
            description: None,
        });
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "CASE_SENSITIVE_EQ"));
    }

    // ---- check_empty_filters_on_large_collection ----

    #[test]
    fn no_filters_wide_collection_warns() {
        let q = make_find(vec![], 100);
        let schema = make_schema(15, vec![]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(advice.iter().any(|a| a.code == "NO_FILTER_WIDE_COLLECTION"));
    }

    #[test]
    fn no_filters_narrow_collection_no_warning() {
        let q = make_find(vec![], 100);
        let schema = make_schema(3, vec![]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "NO_FILTER_WIDE_COLLECTION"));
    }

    // ---- check_sort_without_index ----

    #[test]
    fn sort_without_index_warns() {
        let mut q = make_find(vec![], 100);
        q.sort = Some(SortSpec {
            field: "created_at".to_string(),
            order: -1,
        });
        let schema = make_schema(5, vec![index_on("name")]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(advice.iter().any(|a| a.code == "SORT_NO_INDEX"));
    }

    #[test]
    fn sort_with_index_no_warning() {
        let mut q = make_find(vec![], 100);
        q.sort = Some(SortSpec {
            field: "name".to_string(),
            order: 1,
        });
        let schema = make_schema(5, vec![index_on("name")]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "SORT_NO_INDEX"));
    }

    // ---- check_contains_on_large_field ----

    #[test]
    fn contains_gives_text_search_hint() {
        let q = make_find(
            vec![string_filter("description", FilterOperator::Contains, "hello")],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "TEXT_SEARCH_HINT"));
    }

    // ---- check_multiple_array_filters ----

    #[test]
    fn multiple_dotted_prefix_info() {
        let q = make_find(
            vec![
                string_filter("transactions.symbol", FilterOperator::Eq, "AAPL"),
                num_filter("transactions.amount", FilterOperator::Gt, 100),
            ],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "DOTTED_PREFIX_GROUP"));
    }

    #[test]
    fn single_dotted_filter_no_group_info() {
        let q = make_find(
            vec![string_filter("transactions.symbol", FilterOperator::Eq, "AAPL")],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "DOTTED_PREFIX_GROUP"));
    }

    // ---- check_write_safety ----

    #[test]
    fn update_no_filter_errors() {
        let q = ParsedQuery {
            collection: "users".to_string(),
            operation: QueryOperation::Update,
            update_fields: Some(serde_json::json!({"active": false})),
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "UPDATE_NO_FILTER" && a.severity == "error"));
    }

    #[test]
    fn delete_no_filter_errors() {
        let q = ParsedQuery {
            collection: "users".to_string(),
            operation: QueryOperation::Delete,
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "DELETE_NO_FILTER" && a.severity == "error"));
    }

    #[test]
    fn update_many_warns() {
        let q = ParsedQuery {
            collection: "users".to_string(),
            operation: QueryOperation::Update,
            filters: vec![string_filter("status", FilterOperator::Eq, "inactive")],
            update_fields: Some(serde_json::json!({"archived": true})),
            update_many: true,
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "UPDATE_MANY" && a.severity == "warning"));
        assert!(!advice.iter().any(|a| a.code == "UPDATE_NO_FILTER"));
    }

    #[test]
    fn delete_many_warns() {
        let q = ParsedQuery {
            collection: "logs".to_string(),
            operation: QueryOperation::Delete,
            filters: vec![num_filter("age_days", FilterOperator::Gt, 90)],
            delete_many: true,
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "DELETE_MANY" && a.severity == "warning"));
    }

    // ---- check_distinct_without_index ----

    #[test]
    fn distinct_without_index_warns() {
        let q = ParsedQuery {
            collection: "movies".to_string(),
            operation: QueryOperation::Distinct,
            projection: vec!["genre".to_string()],
            ..Default::default()
        };
        let schema = make_schema(5, vec![index_on("title")]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(advice.iter().any(|a| a.code == "DISTINCT_NO_INDEX"));
    }

    #[test]
    fn distinct_with_index_no_warning() {
        let q = ParsedQuery {
            collection: "movies".to_string(),
            operation: QueryOperation::Distinct,
            projection: vec!["genre".to_string()],
            ..Default::default()
        };
        let schema = make_schema(5, vec![index_on("genre")]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "DISTINCT_NO_INDEX"));
    }

    // ---- integration: multiple rules fire together ----

    #[test]
    fn multiple_rules_fire_together() {
        let q = ParsedQuery {
            collection: "big_table".to_string(),
            operation: QueryOperation::Find,
            filters: vec![
                string_filter("name", FilterOperator::Ne, "admin"),
                string_filter("bio", FilterOperator::Regex, "developer"),
            ],
            sort: Some(SortSpec {
                field: "created_at".to_string(),
                order: -1,
            }),
            limit: 10000,
            ..Default::default()
        };
        let schema = make_schema(12, vec![index_on("name")]);
        let advice = analyze_query(&q, Some(&schema));

        let codes: Vec<&str> = advice.iter().map(|a| a.code.as_str()).collect();
        assert!(codes.contains(&"LARGE_LIMIT"));
        assert!(codes.contains(&"NE_OPERATOR"));
        assert!(codes.contains(&"REGEX_NO_ANCHOR"));
        assert!(codes.contains(&"SORT_NO_INDEX"));
        assert!(codes.contains(&"TEXT_SEARCH_HINT"));
        assert!(codes.contains(&"MISSING_PROJECTION"));
    }

    #[test]
    fn clean_query_produces_no_warnings() {
        let mut q = make_find(
            vec![num_filter("age", FilterOperator::Gt, 25)],
            50,
        );
        q.projection = vec!["name".to_string(), "age".to_string()];
        q.sort = Some(SortSpec {
            field: "age".to_string(),
            order: -1,
        });
        let schema = make_schema(5, vec![index_on("age")]);
        let advice = analyze_query(&q, Some(&schema));
        // Should only have info-level items at most (case sensitivity for non-string)
        let warnings_or_errors: Vec<_> = advice
            .iter()
            .filter(|a| a.severity != "info")
            .collect();
        assert!(
            warnings_or_errors.is_empty(),
            "Expected no warnings/errors, got: {:?}",
            warnings_or_errors
        );
    }

    // ── check_filter_no_index ─────────────────────────────────

    #[test]
    fn filter_no_index_warns_when_field_unindexed() {
        let q = make_find(
            vec![string_filter("email", FilterOperator::Eq, "test@example.com")],
            100,
        );
        // Schema has one real index (not on email)
        let schema = make_schema(5, vec![index_on("username")]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(advice.iter().any(|a| a.code == "FILTER_NO_INDEX"));
    }

    #[test]
    fn filter_no_index_silent_when_field_is_indexed() {
        let q = make_find(
            vec![string_filter("username", FilterOperator::Eq, "alice")],
            100,
        );
        let schema = make_schema(5, vec![index_on("username")]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "FILTER_NO_INDEX"));
    }

    #[test]
    fn filter_no_index_silent_when_only_id_index() {
        // If the schema only has _id_, we have no real index info — stay quiet
        let q = make_find(
            vec![string_filter("email", FilterOperator::Eq, "x@y.com")],
            100,
        );
        let schema = make_schema(3, vec![]);  // make_schema adds no indexes
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "FILTER_NO_INDEX"));
    }

    #[test]
    fn esr_rewrite_suggested_when_compound_index_covers_but_wrong_order() {
        // Existing compound index is {year, status} but query filters equality on status then
        // range on year — ESR order would be {status, year}. Index exists, just reorder.
        let q = ParsedQuery {
            collection: "movies".to_string(),
            operation: QueryOperation::Find,
            filters: vec![
                string_filter("status", FilterOperator::Eq, "active"),
                num_filter("year", FilterOperator::Gt, 2000),
            ],
            limit: 50,
            ..Default::default()
        };
        let schema = make_schema(5, vec![compound_index(&["year", "status"])]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(advice.iter().any(|a| a.code == "ESR_REWRITE"),
            "Expected ESR_REWRITE, got: {:?}", advice);
        assert!(!advice.iter().any(|a| a.code == "FILTER_NO_INDEX"),
            "Should not suggest new index when existing one covers query");
    }

    #[test]
    fn esr_no_advice_when_compound_index_already_in_esr_order() {
        // Index is {status, year} — query has equality on status, range on year. Already ESR.
        let q = ParsedQuery {
            collection: "movies".to_string(),
            operation: QueryOperation::Find,
            filters: vec![
                string_filter("status", FilterOperator::Eq, "active"),
                num_filter("year", FilterOperator::Gt, 2000),
            ],
            limit: 50,
            ..Default::default()
        };
        let schema = make_schema(5, vec![compound_index(&["status", "year"])]);
        let advice = analyze_query(&q, Some(&schema));
        assert!(!advice.iter().any(|a| a.code == "FILTER_NO_INDEX" || a.code == "ESR_REWRITE"),
            "No index advice expected when query already matches ESR order: {:?}", advice);
    }

    #[test]
    fn esr_compound_index_suggested_for_multi_field_query() {
        // No useful index at all — should suggest one compound ESR index, not individual ones
        let q = ParsedQuery {
            collection: "orders".to_string(),
            operation: QueryOperation::Find,
            filters: vec![
                string_filter("status", FilterOperator::Eq, "pending"),
                num_filter("amount", FilterOperator::Gt, 100),
            ],
            sort: Some(SortSpec { field: "created_at".to_string(), order: -1 }),
            limit: 50,
            ..Default::default()
        };
        let schema = make_schema(5, vec![index_on("other_field")]);
        let advice = analyze_query(&q, Some(&schema));
        let no_index = advice.iter().find(|a| a.code == "FILTER_NO_INDEX");
        assert!(no_index.is_some(), "Expected FILTER_NO_INDEX: {:?}", advice);
        // Message should suggest compound ESR index: status (E), created_at (S), amount (R)
        let msg = &no_index.unwrap().message;
        assert!(msg.contains("status"), "ESR index should include equality field 'status'");
        assert!(msg.contains("created_at"), "ESR index should include sort field 'created_at'");
        assert!(msg.contains("amount"), "ESR index should include range field 'amount'");
    }

    #[test]
    fn esr_sort_field_included_in_compound_suggestion() {
        // Query: equality on genre, sort on year, range on rating — ESR: {genre, year, rating}
        let q = ParsedQuery {
            collection: "movies".to_string(),
            operation: QueryOperation::Find,
            filters: vec![
                string_filter("genre", FilterOperator::Eq, "Action"),
                num_filter("rating", FilterOperator::Gte, 7),
            ],
            sort: Some(SortSpec { field: "year".to_string(), order: -1 }),
            limit: 50,
            ..Default::default()
        };
        let schema = make_schema(5, vec![index_on("unrelated")]);
        let advice = analyze_query(&q, Some(&schema));
        let no_index = advice.iter().find(|a| a.code == "FILTER_NO_INDEX");
        assert!(no_index.is_some());
        let msg = &no_index.unwrap().message;
        // ESR order: genre (E) → year (S) → rating (R)
        let genre_pos = msg.find("genre").unwrap();
        let year_pos  = msg.find("year").unwrap();
        let rating_pos = msg.find("rating").unwrap();
        assert!(genre_pos < year_pos,  "genre (E) must come before year (S) in suggestion");
        assert!(year_pos  < rating_pos, "year (S) must come before rating (R) in suggestion");
    }

    // ── New advisory rules ──────────────────────────────────────

    #[test]
    fn aggregate_no_match_warns() {
        let q = ParsedQuery {
            collection: "orders".to_string(),
            operation: QueryOperation::Aggregate,
            group_by: Some(crate::domain::models::GroupSpec {
                field: "status".to_string(),
                accumulators: vec![crate::domain::models::Accumulator {
                    output_field: "count".to_string(),
                    operator: "$sum".to_string(),
                    input_field: "1".to_string(),
                }],
            }),
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "AGGREGATE_NO_MATCH"));
    }

    #[test]
    fn lookup_triggers_info() {
        let q = ParsedQuery {
            collection: "orders".to_string(),
            operation: QueryOperation::Aggregate,
            lookup: Some(crate::domain::models::LookupSpec {
                from: "customers".to_string(),
                local_field: "cid".to_string(),
                foreign_field: "_id".to_string(),
                as_field: "customer".to_string(),
            }),
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "LOOKUP_PERF"));
    }

    #[test]
    fn or_query_triggers_info() {
        let q = ParsedQuery {
            collection: "orders".to_string(),
            or_filters: vec![
                vec![string_filter("status", FilterOperator::Eq, "pending")],
                vec![string_filter("status", FilterOperator::Eq, "shipped")],
            ],
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "OR_QUERY"));
    }

    #[test]
    fn group_by_id_warns() {
        let q = ParsedQuery {
            collection: "users".to_string(),
            operation: QueryOperation::Aggregate,
            group_by: Some(crate::domain::models::GroupSpec {
                field: "_id".to_string(),
                accumulators: vec![],
            }),
            ..Default::default()
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "GROUP_HIGH_CARDINALITY"));
    }

    fn raw_join_then_match(match_cond: serde_json::Value) -> ParsedQuery {
        ParsedQuery {
            collection: "movies".to_string(),
            operation: QueryOperation::Aggregate,
            raw_pipeline: Some(vec![
                serde_json::json!({"$lookup": {"from": "reviews", "localField": "title",
                    "foreignField": "movie", "as": "review_info"}}),
                serde_json::json!({"$match": {"review_info": match_cond}}),
            ]),
            ..Default::default()
        }
    }

    fn has_always_matches(q: &ParsedQuery) -> bool {
        analyze_query(q, None).iter().any(|a| a.code == "LOOKUP_ALWAYS_MATCHES")
    }

    #[test]
    fn lookup_match_ne_null_warns_because_lookup_output_is_never_null() {
        assert!(has_always_matches(&raw_join_then_match(serde_json::json!({"$ne": null}))));
    }

    #[test]
    fn lookup_match_exists_true_warns() {
        assert!(has_always_matches(&raw_join_then_match(serde_json::json!({"$exists": true}))));
    }

    #[test]
    fn lookup_match_ne_empty_array_is_correct_and_does_not_warn() {
        assert!(!has_always_matches(&raw_join_then_match(serde_json::json!({"$ne": []}))));
        assert!(!has_always_matches(&raw_join_then_match(
            serde_json::json!({"$exists": true, "$ne": []})
        )));
    }

    #[test]
    fn lookup_match_on_first_element_does_not_warn() {
        let q = ParsedQuery {
            raw_pipeline: Some(vec![
                serde_json::json!({"$lookup": {"from": "reviews", "localField": "title",
                    "foreignField": "movie", "as": "review_info"}}),
                serde_json::json!({"$match": {"review_info.0": {"$exists": true}}}),
            ]),
            ..raw_join_then_match(serde_json::json!(null))
        };
        assert!(!has_always_matches(&q));
    }

    #[test]
    fn match_before_lookup_does_not_warn() {
        let q = ParsedQuery {
            raw_pipeline: Some(vec![
                serde_json::json!({"$match": {"review_info": {"$ne": null}}}),
                serde_json::json!({"$lookup": {"from": "reviews", "localField": "title",
                    "foreignField": "movie", "as": "review_info"}}),
            ]),
            ..raw_join_then_match(serde_json::json!(null))
        };
        assert!(!has_always_matches(&q));
    }
}
