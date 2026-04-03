// Query parsing utilities — the main NL-to-query logic lives in infra/llm.rs.
// This module provides additional parsing helpers.

use crate::domain::models::ParsedQuery;
use crate::error::AppError;

/// Validate that a ParsedQuery has all required fields filled sensibly.
#[allow(dead_code)] // Available for handlers to call after LLM parsing
pub(crate) fn validate_parsed_query(query: &ParsedQuery) -> Result<(), AppError> {
    if query.collection.is_empty() {
        return Err(AppError::Validation("Collection name cannot be empty".into()));
    }

    if query.limit == 0 {
        return Err(AppError::Validation("Limit must be greater than 0".into()));
    }

    // Validate filter fields are non-empty
    for filter in &query.filters {
        if filter.field.is_empty() {
            return Err(AppError::Validation("Filter field name cannot be empty".into()));
        }
    }

    // Validate projection fields are non-empty
    for proj in &query.projection {
        if proj.is_empty() {
            return Err(AppError::Validation("Projection field name cannot be empty".into()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{QueryOperation, Filter, FilterOperator};

    #[test]
    fn test_validate_valid_query() {
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
        };
        assert!(validate_parsed_query(&query).is_ok());
    }

    #[test]
    fn test_validate_empty_collection() {
        let query = ParsedQuery {
            collection: "".into(),
            operation: QueryOperation::Find,
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
        };
        assert!(validate_parsed_query(&query).is_err());
    }

    #[test]
    fn test_validate_zero_limit() {
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 0,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
        };
        assert!(validate_parsed_query(&query).is_err());
    }

    #[test]
    fn test_validate_empty_filter_field() {
        let query = ParsedQuery {
            collection: "users".into(),
            operation: QueryOperation::Find,
            filters: vec![Filter {
                field: "".into(),
                operator: FilterOperator::Eq,
                value: serde_json::Value::Null,
            }],
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
        };
        assert!(validate_parsed_query(&query).is_err());
    }
}
