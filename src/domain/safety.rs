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

        // 4. If schema is available, validate fields exist
        if let Some(schema) = schema {
            let known_fields: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();

            for filter in &query.filters {
                if !field_matches(&filter.field, &known_fields) {
                    return Err(SafetyError::UnknownField {
                        field: filter.field.clone(),
                        collection: query.collection.clone(),
                    });
                }
            }

            for proj in &query.projection {
                if !field_matches(proj, &known_fields) {
                    return Err(SafetyError::UnknownField {
                        field: proj.clone(),
                        collection: query.collection.clone(),
                    });
                }
            }

            if let Some(ref sort) = query.sort {
                if !field_matches(&sort.field, &known_fields) {
                    return Err(SafetyError::UnknownField {
                        field: sort.field.clone(),
                        collection: query.collection.clone(),
                    });
                }
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
    let suspicious_patterns = ["$where", "$expr", "$function", "$accumulator", "mapReduce"];

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
            sort: None,
            limit: 100,
        };
        assert!(checker.validate(&query, Some(&test_schema())).is_ok());
    }

    #[test]
    fn test_disallowed_operation() {
        let checker = SafetyChecker::new(vec!["find".into()], 10000);
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Aggregate,
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 100,
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
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 200,
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
            projection: vec![],
            sort: None,
            limit: 100,
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
            projection: vec![],
            sort: None,
            limit: 100,
        };
        let result = checker.validate(&query, Some(&test_schema()));
        assert!(matches!(result, Err(SafetyError::UnknownField { .. })));
    }

    #[test]
    fn test_id_always_allowed() {
        assert!(field_matches("_id", &["name", "age"]));
    }
}
