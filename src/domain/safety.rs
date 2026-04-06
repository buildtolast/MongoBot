use crate::domain::models::{CollectionSchema, ParsedQuery};

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
}

impl SafetyChecker {
    /// Create a new safety checker from configuration.
    pub fn new(allowed_operations: Vec<String>, max_rows: usize) -> Self {
        Self {
            allowed_operations,
            max_rows,
        }
    }

    /// Validate a parsed query against safety rules and schema.
    pub fn validate(
        &self,
        query: &ParsedQuery,
        schema: Option<&CollectionSchema>,
    ) -> Result<(), SafetyError> {
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

        // 4. Validate write-specific safety rules
        match query.operation {
            crate::domain::models::QueryOperation::Insert => {
                let doc = query.document.as_ref();
                let is_empty = doc.map_or(true, |v| {
                    v.as_object().map_or(true, |m| m.is_empty())
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
                let is_empty = fields.map_or(true, |v| {
                    v.as_object().map_or(true, |m| m.is_empty())
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

        // 5. If schema is available, validate fields exist
        if let Some(schema) = schema {
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

            for filter in &query.filters {
                check_field(&filter.field)?;
            }
            for proj in &query.projection {
                check_field(proj)?;
            }
            if let Some(ref sort) = query.sort {
                check_field(&sort.field)?;
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
                    check_field(&group.field)?;
                }
                for acc in &group.accumulators {
                    if !acc.input_field.is_empty() && acc.input_field != "1" {
                        check_field(acc.input_field.trim_start_matches('$'))?;
                    }
                }
            }
            if let Some(ref lookup) = query.lookup {
                check_field(&lookup.local_field)?;
            }
            if let Some(ref unwind) = query.unwind {
                check_field(unwind.trim_start_matches('$'))?;
            }
            if let Some(ref bucket) = query.bucket {
                check_field(&bucket.group_by)?;
            }
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
}
