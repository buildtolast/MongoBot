use crate::domain::models::{CollectionSchema, ParsedQuery};

/// Collections that are internal to the application and must never be exposed
/// to user-facing NL queries, regardless of the unsafe_allow_all flag.
const INTERNAL_COLLECTIONS: &[&str] = &["query_feedback"];

/// Errors from safety validation.
#[derive(Debug, thiserror::Error)]
pub enum SafetyError {
    #[error("Operation '{0}' is not allowed")]
    DisallowedOperation(String),

    #[error("Unknown field '{field}' in collection '{collection}'")]
    UnknownField { field: String, collection: String },

    #[error("Limit {requested} exceeds maximum allowed ({max})")]
    LimitExceeded { requested: usize, max: usize },

    #[error("Potential injection pattern detected in field '{0}'")]
    InjectionDetected(String),

    #[error("Collection '{0}' not found in schema")]
    UnknownCollection(String),

    #[error("Write operation '{op}' requires {requirement}")]
    WriteValidation { op: String, requirement: String },
}

/// Validates parsed queries against safety rules.
pub struct SafetyChecker {
    allowed_operations: Vec<String>,
    max_rows: usize,
    /// When true, destructive pipeline stages ($out, $merge, $function, etc.) are allowed.
    unsafe_allow_all: bool,
}

impl SafetyChecker {
    /// Create a new safety checker from configuration.
    pub fn new(allowed_operations: Vec<String>, max_rows: usize) -> Self {
        Self {
            allowed_operations,
            max_rows,
            unsafe_allow_all: false,
        }
    }

    /// Create a safety checker with the unsafe_allow_all flag.
    pub fn with_unsafe_allow_all(mut self, allow: bool) -> Self {
        self.unsafe_allow_all = allow;
        self
    }

    /// Validate a parsed query against safety rules and schema.
    /// A query with a `lookup` is rejected unless the joined collection is
    /// known; use `validate_with_join` to supply its schema.
    pub fn validate(
        &self,
        query: &ParsedQuery,
        schema: Option<&CollectionSchema>,
    ) -> Result<(), SafetyError> {
        self.validate_with_join(query, schema, None)
    }

    /// Validate a parsed query, checking `$lookup` fields against the joined
    /// (`lookup.from`) collection's schema.
    pub fn validate_with_join(
        &self,
        query: &ParsedQuery,
        schema: Option<&CollectionSchema>,
        from_schema: Option<&CollectionSchema>,
    ) -> Result<(), SafetyError> {
        // 0. Absolute denylist — internal collections are never queryable,
        //    even with unsafe_allow_all enabled.
        if INTERNAL_COLLECTIONS.contains(&query.collection.as_str()) {
            return Err(SafetyError::UnknownCollection(query.collection.clone()));
        }
        if let Some(ref lookup) = query.lookup {
            if INTERNAL_COLLECTIONS.contains(&lookup.from.as_str()) {
                return Err(SafetyError::UnknownCollection(lookup.from.clone()));
            }
        }

        // 1. Check operation is allowed
        let op = query.operation.as_str();
        if !self.allowed_operations.iter().any(|a| a == op) {
            return Err(SafetyError::DisallowedOperation(op.to_string()));
        }

        // 2. Check limit is within bounds
        if query.limit > self.max_rows {
            return Err(SafetyError::LimitExceeded {
                requested: query.limit,
                max: self.max_rows,
            });
        }

        // 3. Check for injection patterns in field names
        for filter in &query.filters {
            check_injection(&filter.field)?;
        }
        for proj in &query.projection {
            check_injection(proj)?;
        }
        if let Some(ref sort) = query.sort {
            check_injection(&sort.field)?;
        }

        // 3b. Check injection in or_filters
        for branch in &query.or_filters {
            for filter in branch {
                check_injection(&filter.field)?;
            }
        }

        // 3c. Validate aggregate pipeline fields
        if let Some(ref group) = query.group_by {
            check_injection(&group.field)?;
            for acc in &group.accumulators {
                check_injection(&acc.input_field)?;
                check_injection(&acc.output_field)?;
                // Whitelist accumulator operators
                let op = acc.operator.trim_start_matches('$');
                let allowed = ["sum", "avg", "min", "max", "first", "last", "push", "addToSet", "count"];
                if !allowed.contains(&op) {
                    return Err(SafetyError::InjectionDetected(format!(
                        "Unsupported accumulator: {}", acc.operator
                    )));
                }
            }
        }
        if let Some(ref lookup) = query.lookup {
            check_injection(&lookup.local_field)?;
            check_injection(&lookup.foreign_field)?;
            check_injection(&lookup.as_field)?;
        }
        if let Some(ref unwind) = query.unwind {
            check_injection(unwind.trim_start_matches('$'))?;
        }
        if let Some(ref bucket) = query.bucket {
            check_injection(&bucket.group_by)?;
        }

        // 3d. Validate $expr is a safe cross-field comparison
        if let Some(ref expr) = query.expr {
            check_expr_safety(expr)?;
        }

        // 3e. Validate raw pipeline stages
        if let Some(ref stages) = query.raw_pipeline {
            validate_raw_pipeline(stages, self.unsafe_allow_all)?;
        }

        // 3f. Validate raw filter document
        if let Some(ref filter) = query.raw_filter {
            validate_raw_filter(filter, self.unsafe_allow_all)?;
        }

        // 4. Validate write-specific safety rules
        match query.operation {
            crate::domain::models::QueryOperation::Insert => {
                let doc = query.document.as_ref();
                let is_empty = doc.is_none_or(|v| {
                    v.as_object().is_none_or(|m| m.is_empty())
                });
                if is_empty {
                    return Err(SafetyError::WriteValidation {
                        op: "insert".into(),
                        requirement: "a non-empty document".into(),
                    });
                }
            }
            crate::domain::models::QueryOperation::Update => {
                if query.filters.is_empty() {
                    return Err(SafetyError::WriteValidation {
                        op: "update".into(),
                        requirement: "at least one filter".into(),
                    });
                }
                let fields = query.update_fields.as_ref();
                let is_empty = fields.is_none_or(|v| {
                    v.as_object().is_none_or(|m| m.is_empty())
                });
                if is_empty {
                    return Err(SafetyError::WriteValidation {
                        op: "update".into(),
                        requirement: "at least one update field".into(),
                    });
                }
            }
            crate::domain::models::QueryOperation::Delete => {
                if query.filters.is_empty() {
                    return Err(SafetyError::WriteValidation {
                        op: "delete".into(),
                        requirement: "at least one filter (cannot delete without filters)".into(),
                    });
                }
            }
            _ => {}
        }

        // 5. Validate collection and fields exist in schema.
        //    If the collection is unknown and unsafe_allow_all is set, skip field validation.
        //    If unsafe_allow_all is not set, reject unknown collections outright.
        let schema = match schema {
            Some(s) => s,
            None if self.unsafe_allow_all => return Ok(()),
            None => return Err(SafetyError::UnknownCollection(query.collection.clone())),
        };
        let known_fields: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();

        let check_field = |field: &str| -> Result<(), SafetyError> {
            if !field_matches(field, &known_fields) {
                return Err(SafetyError::UnknownField {
                    field: field.to_string(),
                    collection: query.collection.clone(),
                });
            }
            Ok(())
        };

        // Fields of the joined collection, or None when it is unknown and
        // unsafe_allow_all permits skipping validation of joined references.
        let from_fields: Option<Vec<&str>> = match (&query.lookup, from_schema) {
            (None, _) => None,
            (Some(_), Some(s)) => Some(s.fields.iter().map(|f| f.name.as_str()).collect()),
            (Some(_), None) if self.unsafe_allow_all => None,
            (Some(lookup), None) => return Err(SafetyError::UnknownCollection(lookup.from.clone())),
        };

        // Stages after $lookup may also reference the join output: the `as`
        // name itself or `<as>.<field of the joined collection>`.
        let check_post_join_field = |field: &str| -> Result<(), SafetyError> {
            if let Some(ref lookup) = query.lookup {
                let joined = field
                    .strip_prefix(lookup.as_field.as_str())
                    .filter(|rest| rest.is_empty() || rest.starts_with('.'));
                if let Some(rest) = joined {
                    let rest = rest.trim_start_matches('.');
                    return match &from_fields {
                        Some(known) if !rest.is_empty() && !field_matches(rest, known) => {
                            Err(SafetyError::UnknownField {
                                field: rest.to_string(),
                                collection: lookup.from.clone(),
                            })
                        }
                        _ => Ok(()),
                    };
                }
            }
            check_field(field)
        };

        // Filters become the leading $match, which runs before $lookup, so they
        // may only reference the base collection.
        for filter in &query.filters {
            check_field(&filter.field)?;
        }
        for proj in &query.projection {
            check_post_join_field(proj)?;
        }
        if let Some(ref sort) = query.sort {
            check_post_join_field(&sort.field)?;
        }
        // Validate or_filters fields
        for branch in &query.or_filters {
            for filter in branch {
                check_field(&filter.field)?;
            }
        }
        // Validate aggregate field references
        if let Some(ref group) = query.group_by {
            if !group.field.is_empty() && group.field != "null" {
                check_post_join_field(&group.field)?;
            }
            for acc in &group.accumulators {
                if !acc.input_field.is_empty() && acc.input_field != "1" {
                    check_post_join_field(acc.input_field.trim_start_matches('$'))?;
                }
            }
        }
        if let Some(ref lookup) = query.lookup {
            check_field(&lookup.local_field)?;
            if let Some(ref known) = from_fields {
                if !field_matches(&lookup.foreign_field, known) {
                    return Err(SafetyError::UnknownField {
                        field: lookup.foreign_field.clone(),
                        collection: lookup.from.clone(),
                    });
                }
            }
        }
        if let Some(ref unwind) = query.unwind {
            check_post_join_field(unwind.trim_start_matches('$'))?;
        }
        if let Some(ref bucket) = query.bucket {
            check_post_join_field(&bucket.group_by)?;
        }

        Ok(())
    }
}

/// Check if a field name exists in the known fields list.
/// Supports dot-notation: "address.city" matches if "address" or "address.city" is known.
fn field_matches(field: &str, known: &[&str]) -> bool {
    if known.contains(&field) {
        return true;
    }

    // Check if a parent of the dotted path is known
    let mut prefix = String::new();
    for part in field.split('.') {
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(part);
        if known.contains(&prefix.as_str()) {
            return true;
        }
    }

    // _id is always valid
    field == "_id"
}

/// Validate raw aggregate pipeline stages for dangerous operators.
/// When `allow_all` is true, only JS injection patterns are blocked ($function, $accumulator, $where).
fn validate_raw_pipeline(stages: &[serde_json::Value], allow_all: bool) -> Result<(), SafetyError> {
    // JS injection is always blocked regardless of allow_all
    const ALWAYS_BLOCKED: &[&str] = &["$function", "$accumulator", "$where"];

    // These are only blocked when allow_all is false
    const RESTRICTED_STAGES: &[&str] = &[
        "$out", "$merge", "$indexStats", "$collStats", "$planCacheStats", "$currentOp",
    ];

    for stage in stages {
        let obj = stage.as_object().ok_or_else(|| {
            SafetyError::InjectionDetected("Pipeline stage must be an object".into())
        })?;

        for key in obj.keys() {
            let lower = key.to_lowercase();

            // JS injection — always blocked
            for blocked in ALWAYS_BLOCKED {
                if lower == blocked.to_lowercase() {
                    return Err(SafetyError::InjectionDetected(format!(
                        "Pipeline stage '{}' is not allowed (JavaScript execution)", key
                    )));
                }
            }

            // Restricted stages — blocked unless allow_all
            if !allow_all {
                for blocked in RESTRICTED_STAGES {
                    if lower == blocked.to_lowercase() {
                        return Err(SafetyError::InjectionDetected(format!(
                            "Pipeline stage '{}' is not allowed (set UNSAFE_ALLOW_ALL_OPERATIONS=true to enable)", key
                        )));
                    }
                }
            }
        }

        // Recursively scan values for JS injection patterns
        scan_value_for_injection(stage)?;
    }

    Ok(())
}

/// Validate a raw MongoDB filter document for dangerous operators.
/// When `allow_all` is true, the operator allowlist is not enforced (but JS injection is still blocked).
fn validate_raw_filter(filter: &serde_json::Value, allow_all: bool) -> Result<(), SafetyError> {
    const BLOCKED_OPERATORS: &[&str] = &[
        "$where", "$function", "$accumulator",
    ];

    const ALLOWED_OPERATORS: &[&str] = &[
        "$and", "$or", "$nor", "$not",
        "$gt", "$gte", "$lt", "$lte", "$eq", "$ne",
        "$in", "$nin", "$exists", "$type", "$size", "$all",
        "$elemMatch", "$regex", "$options", "$text", "$search",
        "$expr",
    ];

    scan_filter_keys(filter, BLOCKED_OPERATORS, if allow_all { None } else { Some(ALLOWED_OPERATORS) })?;
    Ok(())
}

/// Recursively scan a JSON value for dangerous string patterns in keys.
fn scan_value_for_injection(val: &serde_json::Value) -> Result<(), SafetyError> {
    const DANGEROUS: &[&str] = &["$where", "$function", "$accumulator"];

    match val {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let lower = key.to_lowercase();
                for pattern in DANGEROUS {
                    if lower == pattern.to_lowercase() {
                        return Err(SafetyError::InjectionDetected(format!(
                            "Dangerous operator '{}' in pipeline", key
                        )));
                    }
                }
                scan_value_for_injection(child)?;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                scan_value_for_injection(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Recursively scan filter keys, blocking dangerous operators and validating $ operators.
/// When `allowed` is None, the allowlist is not enforced (all $ operators pass except blocked ones).
fn scan_filter_keys(
    val: &serde_json::Value,
    blocked: &[&str],
    allowed: Option<&[&str]>,
) -> Result<(), SafetyError> {
    match val {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if key.starts_with('$') {
                    // Check if blocked (JS injection — always enforced)
                    let lower = key.to_lowercase();
                    for b in blocked {
                        if lower == b.to_lowercase() {
                            return Err(SafetyError::InjectionDetected(format!(
                                "Operator '{}' is not allowed in filters (JavaScript execution)", key
                            )));
                        }
                    }
                    // Check if known — only when allowlist is enforced
                    if let Some(allow_list) = allowed {
                        if !allow_list.contains(&key.as_str()) {
                            return Err(SafetyError::InjectionDetected(format!(
                                "Unknown operator '{}' in filter", key
                            )));
                        }
                    }
                    // Special: $expr must pass safety check
                    if key == "$expr" {
                        check_expr_safety(child)?;
                    }
                }
                scan_filter_keys(child, blocked, allowed)?;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                scan_filter_keys(item, blocked, allowed)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Check a field name for potential injection patterns.
fn check_injection(field: &str) -> Result<(), SafetyError> {
    // $expr is handled separately via check_expr_safety — not blocked here
    let suspicious_patterns = ["$where", "$function", "$accumulator", "mapReduce"];

    let lower = field.to_lowercase();
    for pattern in &suspicious_patterns {
        if lower.contains(&pattern.to_lowercase()) {
            return Err(SafetyError::InjectionDetected(field.to_string()));
        }
    }

    if field.starts_with('$') {
        return Err(SafetyError::InjectionDetected(field.to_string()));
    }

    Ok(())
}

/// Validate that an $expr value is a safe cross-field comparison.
/// Only allows simple comparisons like { "$gt": ["$field_a", "$field_b"] }.
fn check_expr_safety(expr: &serde_json::Value) -> Result<(), SafetyError> {
    let obj = expr.as_object().ok_or_else(|| {
        SafetyError::InjectionDetected("$expr must be an object".into())
    })?;

    if obj.len() != 1 {
        return Err(SafetyError::InjectionDetected(
            "$expr must have exactly one operator".into(),
        ));
    }

    let (op, val) = obj.iter().next().unwrap();
    let allowed_ops = ["$gt", "$gte", "$lt", "$lte", "$eq", "$ne"];
    if !allowed_ops.contains(&op.as_str()) {
        return Err(SafetyError::InjectionDetected(format!(
            "$expr operator '{}' not allowed — only comparisons permitted", op
        )));
    }

    // Value must be a 2-element array of field references (strings starting with $)
    let arr = val.as_array().ok_or_else(|| {
        SafetyError::InjectionDetected("$expr comparison must have an array of two field references".into())
    })?;

    if arr.len() != 2 {
        return Err(SafetyError::InjectionDetected(
            "$expr comparison must have exactly two operands".into(),
        ));
    }

    // Each operand must be a string starting with $ (field ref) or a literal number/string
    for operand in arr {
        match operand {
            serde_json::Value::String(s) => {
                // Field references start with $, plain strings are allowed too
                if s.starts_with('$') {
                    // Check the field name part for injection
                    let field_name = s.trim_start_matches('$');
                    check_injection(field_name)?;
                }
            }
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {}
            _ => {
                return Err(SafetyError::InjectionDetected(
                    "$expr operands must be field references or literals".into(),
                ));
            }
        }
    }

    Ok(())
}

impl From<SafetyError> for crate::error::AppError {
    fn from(err: SafetyError) -> Self {
        crate::error::AppError::Safety(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::*;

    fn test_checker() -> SafetyChecker {
        SafetyChecker::new(
            vec![
                "find".into(),
                "count".into(),
                "distinct".into(),
                "aggregate".into(),
                "insert".into(),
                "update".into(),
                "delete".into(),
            ],
            10000,
        )
    }

    fn test_schema() -> CollectionSchema {
        CollectionSchema {
            name: "users".into(),
            description: "User collection".into(),
            fields: vec![
                FieldDef {
                    name: "name".into(),
                    field_type: "string".into(),
                    description: None,
                },
                FieldDef {
                    name: "age".into(),
                    field_type: "number".into(),
                    description: None,
                },
                FieldDef {
                    name: "email".into(),
                    field_type: "string".into(),
                    description: None,
                },
            ],
            indexes: vec![],
            source: "test".into(),
        }
    }

    #[test]
    fn test_valid_query_passes() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            filters: vec![Filter {
                field: "age".into(),
                operator: FilterOperator::Gt,
                value: serde_json::json!(25),
            }],
            projection: vec!["name".into()],
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&test_schema())).is_ok());
    }

    #[test]
    fn test_disallowed_operation() {
        let checker = SafetyChecker::new(vec!["find".into()], 10000);
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Aggregate,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::DisallowedOperation(_))));
    }

    #[test]
    fn test_limit_exceeded() {
        let checker = SafetyChecker::new(vec!["find".into()], 100);
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            limit: 200,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::LimitExceeded { .. })));
    }

    #[test]
    fn test_injection_detected() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            filters: vec![Filter {
                field: "$where".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("1==1"),
            }],
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::InjectionDetected(_))));
    }

    #[test]
    fn test_unknown_field() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            filters: vec![Filter {
                field: "nonexistent".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("x"),
            }],
            ..Default::default()
        };
        let result = checker.validate(&query, Some(&test_schema()));
        assert!(matches!(result, Err(SafetyError::UnknownField { .. })));
    }

    #[test]
    fn test_delete_requires_filter() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Delete,
            delete_many: true,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::WriteValidation { .. })));
    }

    #[test]
    fn test_insert_requires_document() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Insert,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::WriteValidation { .. })));
    }

    #[test]
    fn test_update_requires_filter_and_fields() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Update,
            update_fields: Some(serde_json::json!({"name": "Bob"})),
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::WriteValidation { .. })));
    }

    #[test]
    fn test_id_always_allowed() {
        assert!(field_matches("_id", &["name", "age"]));
    }

    // ── $expr safety ────────────────────────────────────────────

    #[test]
    fn test_expr_simple_comparison_allowed() {
        let expr = serde_json::json!({"$gt": ["$age", "$min_age"]});
        assert!(check_expr_safety(&expr).is_ok());
    }

    #[test]
    fn test_expr_with_literal_allowed() {
        let expr = serde_json::json!({"$gte": ["$score", 100]});
        assert!(check_expr_safety(&expr).is_ok());
    }

    #[test]
    fn test_expr_function_blocked() {
        let expr = serde_json::json!({"$function": {"body": "return true", "args": [], "lang": "js"}});
        assert!(check_expr_safety(&expr).is_err());
    }

    #[test]
    fn test_expr_nested_object_blocked() {
        let expr = serde_json::json!({"$gt": [{"$add": ["$a", "$b"]}, "$c"]});
        assert!(check_expr_safety(&expr).is_err());
    }

    // ── Accumulator validation ──────────────────────────────────

    #[test]
    fn test_valid_accumulator_passes() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            group_by: Some(GroupSpec {
                field: "name".into(),
                accumulators: vec![Accumulator {
                    output_field: "total".into(),
                    operator: "$sum".into(),
                    input_field: "age".into(),
                }],
            }),
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&test_schema())).is_ok());
    }

    #[test]
    fn test_invalid_accumulator_blocked() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            group_by: Some(GroupSpec {
                field: "name".into(),
                accumulators: vec![Accumulator {
                    output_field: "result".into(),
                    operator: "$evil".into(),
                    input_field: "age".into(),
                }],
            }),
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&test_schema())).is_err());
    }

    // ── or_filters injection check ──────────────────────────────

    #[test]
    fn test_or_filters_injection_caught() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            or_filters: vec![vec![Filter {
                field: "$where".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("1==1"),
            }]],
            ..Default::default()
        };
        assert!(checker.validate(&query, None).is_err());
    }

    // ── Raw pipeline validation ─────────────────────────────────

    #[test]
    fn test_raw_pipeline_blocks_out() {
        let stages = vec![serde_json::json!({"$out": "evil_collection"})];
        assert!(validate_raw_pipeline(&stages, false).is_err());
    }

    #[test]
    fn test_raw_pipeline_blocks_merge() {
        let stages = vec![serde_json::json!({"$merge": {"into": "target"}})];
        assert!(validate_raw_pipeline(&stages, false).is_err());
    }

    #[test]
    fn test_raw_pipeline_blocks_function() {
        let stages = vec![serde_json::json!({
            "$addFields": {"result": {"$function": {"body": "return 1", "args": [], "lang": "js"}}}
        })];
        assert!(validate_raw_pipeline(&stages, false).is_err());
    }

    #[test]
    fn test_raw_pipeline_allows_safe_stages() {
        let stages = vec![
            serde_json::json!({"$match": {"status": "active"}}),
            serde_json::json!({"$group": {"_id": "$category", "count": {"$sum": 1}}}),
            serde_json::json!({"$sort": {"count": -1}}),
        ];
        assert!(validate_raw_pipeline(&stages, false).is_ok());
    }

    // ── Raw filter validation ───────────────────────────────────

    #[test]
    fn test_raw_filter_blocks_where() {
        let filter = serde_json::json!({"$where": "this.a > this.b"});
        assert!(validate_raw_filter(&filter, false).is_err());
    }

    #[test]
    fn test_raw_filter_allows_complex_or() {
        let filter = serde_json::json!({
            "$or": [
                {"status": "active", "age": {"$gt": 25}},
                {"$and": [{"role": "admin"}, {"verified": true}]}
            ]
        });
        assert!(validate_raw_filter(&filter, false).is_ok());
    }

    // ── unsafe_allow_all flag ───────────────────────────────────

    #[test]
    fn test_unsafe_allow_all_permits_out() {
        let stages = vec![serde_json::json!({"$out": "backup_collection"})];
        // Blocked by default
        assert!(validate_raw_pipeline(&stages, false).is_err());
        // Allowed with flag
        assert!(validate_raw_pipeline(&stages, true).is_ok());
    }

    #[test]
    fn test_unsafe_allow_all_permits_merge() {
        let stages = vec![serde_json::json!({"$merge": {"into": "target"}})];
        assert!(validate_raw_pipeline(&stages, false).is_err());
        assert!(validate_raw_pipeline(&stages, true).is_ok());
    }

    #[test]
    fn test_unsafe_allow_all_still_blocks_function() {
        // $function is JS injection — blocked even with allow_all
        let stages = vec![serde_json::json!({
            "$addFields": {"x": {"$function": {"body": "return 1", "args": [], "lang": "js"}}}
        })];
        assert!(validate_raw_pipeline(&stages, true).is_err());
    }

    #[test]
    fn test_unknown_collection_errors() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "nonexistent".into(),
            operation: QueryOperation::Find,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(matches!(result, Err(SafetyError::UnknownCollection(_))));
    }

    // ── INTERNAL_COLLECTIONS denylist ───────────────────────────

    #[test]
    fn test_rejects_query_feedback_collection() {
        // query_feedback must be blocked even with unsafe_allow_all enabled
        let checker = test_checker().with_unsafe_allow_all(true);
        let query = ParsedQuery {
            collection: "query_feedback".into(),
            operation: QueryOperation::Find,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(
            matches!(result, Err(SafetyError::UnknownCollection(_))),
            "query_feedback must be rejected regardless of unsafe_allow_all"
        );
    }

    #[test]
    fn test_unsafe_allows_unknown_collection() {
        let checker = test_checker().with_unsafe_allow_all(true);
        let query = ParsedQuery {
            collection: "ad_hoc_collection".into(),
            operation: QueryOperation::Find,
            ..Default::default()
        };
        let result = checker.validate(&query, None);
        assert!(result.is_ok(), "unsafe_allow_all should permit unknown non-internal collections");
    }

    // ── field_matches dotted-path ────────────────────────────────

    #[test]
    fn field_matches_exact_match() {
        assert!(field_matches("title", &["title", "year"]));
    }

    #[test]
    fn field_matches_dotted_parent_known() {
        // "address.city" should match when "address" is a known field
        assert!(field_matches("address.city", &["address", "name"]));
    }

    #[test]
    fn field_matches_id_always_true() {
        assert!(field_matches("_id", &[]));
    }

    #[test]
    fn field_matches_unknown_returns_false() {
        assert!(!field_matches("unknown_field", &["title", "year"]));
    }

    // ── validate_raw_pipeline edge cases ────────────────────────

    #[test]
    fn raw_pipeline_non_object_stage_returns_error() {
        let stages = vec![serde_json::json!("not_an_object")];
        let result = validate_raw_pipeline(&stages, false);
        assert!(result.is_err());
    }

    #[test]
    fn raw_pipeline_nested_function_still_blocked_with_allow_all() {
        let stages = vec![serde_json::json!({
            "$match": {"$function": {"body": "function() {}", "args": [], "lang": "js"}}
        })];
        let result = validate_raw_pipeline(&stages, true);
        assert!(result.is_err());
    }

    // ── validate_raw_filter unknown operator ────────────────────

    #[test]
    fn raw_filter_unknown_operator_blocked_without_allow_all() {
        let filter = serde_json::json!({"field": {"$customOp": 1}});
        let result = validate_raw_filter(&filter, false);
        assert!(result.is_err(), "unknown $ operator should be blocked");
    }

    #[test]
    fn raw_filter_unknown_operator_allowed_with_allow_all() {
        let filter = serde_json::json!({"field": {"$customOp": 1}});
        let result = validate_raw_filter(&filter, true);
        assert!(result.is_ok(), "unknown $ operator allowed when unsafe_allow_all");
    }

    #[test]
    fn raw_filter_with_expr_runs_safety_check() {
        // $expr in raw_filter triggers check_expr_safety
        let filter = serde_json::json!({"$expr": {"$gt": ["$price", "$cost"]}});
        assert!(validate_raw_filter(&filter, false).is_ok());
    }

    #[test]
    fn raw_filter_expr_with_dangerous_op_blocked() {
        let filter = serde_json::json!({"$expr": {"$function": {}}});
        assert!(validate_raw_filter(&filter, false).is_err());
    }

    // ── check_expr_safety edge cases ────────────────────────────

    #[test]
    fn expr_safety_wrong_operand_count() {
        let expr = serde_json::json!({"$gt": ["$a", "$b", "$c"]});
        assert!(check_expr_safety(&expr).is_err());
    }

    #[test]
    fn expr_safety_non_array_operand() {
        let expr = serde_json::json!({"$gt": "not_an_array"});
        assert!(check_expr_safety(&expr).is_err());
    }

    #[test]
    fn expr_safety_object_operand_blocked() {
        let expr = serde_json::json!({"$gt": [{"$function": {}}, "$b"]});
        assert!(check_expr_safety(&expr).is_err());
    }

    #[test]
    fn expr_safety_multiple_operators_blocked() {
        let expr = serde_json::json!({"$gt": ["$a", "$b"], "$lt": ["$c", "$d"]});
        assert!(check_expr_safety(&expr).is_err());
    }

    // ── Cross-collection $lookup validation ───

    fn schema_with(name: &str, fields: &[&str]) -> CollectionSchema {
        CollectionSchema {
            name: name.into(),
            description: String::new(),
            fields: fields
                .iter()
                .map(|f| FieldDef {
                    name: (*f).into(),
                    field_type: "string".into(),
                    description: None,
                })
                .collect(),
            indexes: vec![],
            source: "test".into(),
        }
    }

    fn reviews_schema() -> CollectionSchema {
        schema_with("reviews", &["movie", "score", "reviewer"])
    }

    fn movies_schema() -> CollectionSchema {
        schema_with("movies", &["title", "year", "genres", "imdb.rating"])
    }

    fn reviews_join_movies() -> ParsedQuery {
        ParsedQuery {
            collection: "reviews".into(),
            operation: QueryOperation::Aggregate,
            lookup: Some(crate::domain::models::LookupSpec {
                from: "movies".into(),
                local_field: "movie".into(),
                foreign_field: "title".into(),
                as_field: "movie_info".into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn join_foreign_field_is_validated_against_from_collection() {
        let result = test_checker().validate_with_join(
            &reviews_join_movies(),
            Some(&reviews_schema()),
            Some(&movies_schema()),
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn join_rejects_foreign_field_missing_from_from_collection() {
        let mut query = reviews_join_movies();
        query.lookup.as_mut().unwrap().foreign_field = "no_such_field".into();
        let result =
            test_checker().validate_with_join(&query, Some(&reviews_schema()), Some(&movies_schema()));
        assert!(result.is_err());
    }

    #[test]
    fn join_allows_projection_and_sort_on_joined_fields() {
        let query = ParsedQuery {
            unwind: Some("movie_info".into()),
            projection: vec!["movie".into(), "movie_info.year".into(), "movie_info.imdb.rating".into()],
            sort: Some(SortSpec { field: "movie_info.year".into(), order: -1 }),
            ..reviews_join_movies()
        };
        let result =
            test_checker().validate_with_join(&query, Some(&reviews_schema()), Some(&movies_schema()));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn join_rejects_unknown_joined_field() {
        let query = ParsedQuery {
            projection: vec!["movie_info.no_such_field".into()],
            ..reviews_join_movies()
        };
        let result =
            test_checker().validate_with_join(&query, Some(&reviews_schema()), Some(&movies_schema()));
        assert!(result.is_err());
    }

    #[test]
    fn join_allows_group_by_joined_field() {
        let query = ParsedQuery {
            unwind: Some("movie_info.genres".into()),
            group_by: Some(crate::domain::models::GroupSpec {
                field: "movie_info.genres".into(),
                accumulators: vec![crate::domain::models::Accumulator {
                    output_field: "avg_score".into(),
                    operator: "$avg".into(),
                    input_field: "score".into(),
                }],
            }),
            ..reviews_join_movies()
        };
        let result =
            test_checker().validate_with_join(&query, Some(&reviews_schema()), Some(&movies_schema()));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn join_rejects_filter_on_joined_field_because_match_runs_before_lookup() {
        let query = ParsedQuery {
            filters: vec![Filter {
                field: "movie_info.year".into(),
                operator: FilterOperator::Gt,
                value: serde_json::json!(2000),
            }],
            ..reviews_join_movies()
        };
        let result =
            test_checker().validate_with_join(&query, Some(&reviews_schema()), Some(&movies_schema()));
        assert!(result.is_err());
    }

    #[test]
    fn join_rejects_unknown_from_collection() {
        let result =
            test_checker().validate_with_join(&reviews_join_movies(), Some(&reviews_schema()), None);
        assert!(result.is_err());
    }

    #[test]
    fn join_rejects_internal_from_collection_even_with_unsafe_allow_all() {
        let mut query = reviews_join_movies();
        query.lookup.as_mut().unwrap().from = "query_feedback".into();
        let checker = test_checker().with_unsafe_allow_all(true);
        let result = checker.validate_with_join(&query, Some(&reviews_schema()), None);
        assert!(result.is_err());
    }

    // ── Schema field validation for aggregate pipeline fields ───

    #[test]
    fn lookup_local_field_must_exist_in_schema() {
        let checker = test_checker();
        let schema = test_schema();
        let query = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Aggregate,
            lookup: Some(crate::domain::models::LookupSpec {
                from: "reviews".into(),
                local_field: "nonexistent_field".into(),
                foreign_field: "movie_id".into(),
                as_field: "reviews".into(),
            }),
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&schema)).is_err());
    }

    #[test]
    fn unwind_field_must_exist_in_schema() {
        let checker = test_checker();
        let schema = test_schema();
        let query = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Aggregate,
            unwind: Some("nonexistent_array".into()),
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&schema)).is_err());
    }

    #[test]
    fn bucket_field_must_exist_in_schema() {
        let checker = test_checker();
        let schema = test_schema();
        let query = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Aggregate,
            bucket: Some(crate::domain::models::BucketSpec {
                group_by: "nonexistent_field".into(),
                boundaries: vec![serde_json::json!(0), serde_json::json!(50), serde_json::json!(100)],
                default_bucket: None,
            }),
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&schema)).is_err());
    }

    #[test]
    fn or_filters_field_must_exist_in_schema() {
        let checker = test_checker();
        let schema = test_schema();
        let query = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Find,
            or_filters: vec![vec![Filter {
                field: "nonexistent_field".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("value"),
            }]],
            ..Default::default()
        };
        assert!(checker.validate(&query, Some(&schema)).is_err());
    }

    #[test]
    fn sort_field_injection_is_caught() {
        let checker = test_checker();
        use crate::domain::models::SortSpec;
        let query = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Find,
            sort: Some(SortSpec { field: "$where".into(), order: 1 }),
            ..Default::default()
        };
        assert!(checker.validate(&query, None).is_err());
    }

    #[test]
    fn projection_injection_is_caught() {
        let checker = test_checker();
        let query = ParsedQuery {
            collection: "movies".into(),
            operation: QueryOperation::Find,
            projection: vec!["$function".into()],
            ..Default::default()
        };
        assert!(checker.validate(&query, None).is_err());
    }
}
