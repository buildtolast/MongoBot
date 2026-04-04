use crate::domain::models::{
    CollectionSchema, FilterOperator, ParsedQuery, QueryOperation,
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
    check_sort_without_index(query, schema, &mut advice);
    check_contains_on_large_field(query, &mut advice);
    check_multiple_array_filters(query, &mut advice);
    check_write_safety(query, &mut advice);
    check_distinct_without_index(query, schema, &mut advice);

    advice
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

/// Info when multiple filters target fields under the same array prefix,
/// indicating $elemMatch will be used (which users should be aware of).
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
                "ELEMMATCH_USED",
                format!(
                    "Multiple filters on array '{}' ({} conditions) will use \
                     $elemMatch to match on the same array element. Verify this \
                     is the intended behavior.",
                    prefix, count
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::*;

    fn make_find(filters: Vec<Filter>, limit: usize) -> ParsedQuery {
        ParsedQuery {
            collection: "test".to_string(),
            operation: QueryOperation::Find,
            filters,
            projection: vec![],
            sort: None,
            limit,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
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
    fn multiple_array_filters_info() {
        let q = make_find(
            vec![
                string_filter("transactions.symbol", FilterOperator::Eq, "AAPL"),
                num_filter("transactions.amount", FilterOperator::Gt, 100),
            ],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "ELEMMATCH_USED"));
    }

    #[test]
    fn single_array_filter_no_elemmatch_info() {
        let q = make_find(
            vec![string_filter("transactions.symbol", FilterOperator::Eq, "AAPL")],
            100,
        );
        let advice = analyze_query(&q, None);
        assert!(!advice.iter().any(|a| a.code == "ELEMMATCH_USED"));
    }

    // ---- check_write_safety ----

    #[test]
    fn update_no_filter_errors() {
        let q = ParsedQuery {
            collection: "users".to_string(),
            operation: QueryOperation::Update,
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: Some(serde_json::json!({"active": false})),
            update_many: false,
            delete_many: false,
        };
        let advice = analyze_query(&q, None);
        assert!(advice.iter().any(|a| a.code == "UPDATE_NO_FILTER" && a.severity == "error"));
    }

    #[test]
    fn delete_no_filter_errors() {
        let q = ParsedQuery {
            collection: "users".to_string(),
            operation: QueryOperation::Delete,
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
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
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: Some(serde_json::json!({"archived": true})),
            update_many: true,
            delete_many: false,
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
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: true,
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
            filters: vec![],
            projection: vec!["genre".to_string()],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
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
            filters: vec![],
            projection: vec!["genre".to_string()],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
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
            projection: vec![],
            sort: Some(SortSpec {
                field: "created_at".to_string(),
                order: -1,
            }),
            limit: 10000,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
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
}
