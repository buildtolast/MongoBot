use serde::{Deserialize, Serialize};

/// A parsed MongoDB query from natural language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedQuery {
    /// Target collection name.
    pub collection: String,

    /// The type of MongoDB operation.
    #[serde(default)]
    pub operation: QueryOperation,

    /// Filters to apply.
    #[serde(default)]
    pub filters: Vec<Filter>,

    /// Fields to include in the result.
    #[serde(default)]
    pub projection: Vec<String>,

    /// Sort specification.
    #[serde(default)]
    pub sort: Option<SortSpec>,

    /// Maximum number of documents to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// MongoDB operation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperation {
    Find,
    Count,
    Aggregate,
    Distinct,
}

impl Default for QueryOperation {
    fn default() -> Self {
        Self::Find
    }
}

impl QueryOperation {
    /// Return the operation name as a lowercase string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Find => "find",
            Self::Count => "count",
            Self::Aggregate => "aggregate",
            Self::Distinct => "distinct",
        }
    }
}

/// A single filter condition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    /// Field name to filter on.
    pub field: String,

    /// Comparison operator.
    pub operator: FilterOperator,

    /// Value to compare against.
    pub value: serde_json::Value,
}

/// Filter comparison operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Nin,
    Contains,
    Regex,
}

/// Sort specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortSpec {
    /// Field to sort by.
    pub field: String,

    /// Sort order: 1 for ascending, -1 for descending.
    #[serde(default = "default_sort_order")]
    pub order: i32,
}

fn default_sort_order() -> i32 {
    1
}

/// Schema definition for a MongoDB collection, loaded from YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionSchema {
    /// Collection name.
    pub name: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: String,

    /// Field definitions.
    #[serde(default)]
    pub fields: Vec<FieldDef>,
}

/// A single field definition within a collection schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    /// Field name (supports dot notation for nested fields).
    pub name: String,

    /// Data type (e.g. "string", "number", "date", "objectId", "boolean", "array").
    pub field_type: String,

    /// Optional description of what this field contains.
    #[serde(default)]
    pub description: Option<String>,
}

/// Incoming query request from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    /// Natural language query string.
    pub query: String,

    /// Optional export format.
    #[serde(default)]
    pub format: Option<ExportFormat>,
}

/// Query response returned to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    /// The parsed structured query.
    pub parsed: ParsedQuery,

    /// Query results.
    pub results: serde_json::Value,

    /// Number of result items.
    pub count: usize,

    /// Execution time in milliseconds.
    pub execution_ms: u64,

    /// The equivalent MongoDB shell command.
    pub mongo_query: String,
}

/// Export format options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Json,
}

/// Export request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportParams {
    /// Natural language query string.
    pub query: String,

    /// Export format.
    #[serde(default)]
    pub format: Option<ExportFormat>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsed_query_deserialize() {
        let json = r#"{
            "collection": "users",
            "operation": "find",
            "filters": [{"field": "age", "operator": "gt", "value": 25}],
            "projection": ["name", "age"],
            "sort": {"field": "age", "order": -1},
            "limit": 50
        }"#;
        let query: ParsedQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.collection, "users");
        assert_eq!(query.operation, QueryOperation::Find);
        assert_eq!(query.filters.len(), 1);
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn test_parsed_query_defaults() {
        let json = r#"{"collection": "users"}"#;
        let query: ParsedQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.operation, QueryOperation::Find);
        assert_eq!(query.limit, 100);
        assert!(query.filters.is_empty());
        assert!(query.projection.is_empty());
        assert!(query.sort.is_none());
    }

    #[test]
    fn test_filter_operator_variants() {
        let json = r#"{"field":"x","operator":"contains","value":"test"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        assert_eq!(f.operator, FilterOperator::Contains);
    }
}
