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

    /// Document to insert (for insert operations).
    #[serde(default)]
    pub document: Option<serde_json::Value>,

    /// Fields to update (for update operations). Maps field names to new values.
    #[serde(default)]
    pub update_fields: Option<serde_json::Value>,

    /// Whether to update all matching documents (default: false = updateOne).
    #[serde(default)]
    pub update_many: bool,

    /// Whether to delete all matching documents (default: false = deleteOne).
    #[serde(default)]
    pub delete_many: bool,

    /// Group-by specification for aggregate $group stage.
    #[serde(default)]
    pub group_by: Option<GroupSpec>,

    /// Cross-collection join ($lookup stage).
    #[serde(default)]
    pub lookup: Option<LookupSpec>,

    /// Array field to flatten ($unwind stage).
    #[serde(default)]
    pub unwind: Option<String>,

    /// Parallel sub-pipelines ($facet stage). Raw JSON since structure is free-form.
    #[serde(default)]
    pub facet: Option<serde_json::Value>,

    /// Range grouping ($bucket stage).
    #[serde(default)]
    pub bucket: Option<BucketSpec>,

    /// OR conditions — each inner Vec is one $or branch.
    #[serde(default)]
    pub or_filters: Vec<Vec<Filter>>,

    /// Full-text search string ($text: { $search: "..." }).
    #[serde(default)]
    pub text_search: Option<String>,

    /// Cross-field comparison ($expr). Raw JSON value.
    #[serde(default)]
    pub expr: Option<serde_json::Value>,
}

fn default_limit() -> usize {
    100
}

impl Default for ParsedQuery {
    fn default() -> Self {
        Self {
            collection: String::new(),
            operation: QueryOperation::default(),
            filters: vec![],
            projection: vec![],
            sort: None,
            limit: 100,
            document: None,
            update_fields: None,
            update_many: false,
            delete_many: false,
            group_by: None,
            lookup: None,
            unwind: None,
            facet: None,
            bucket: None,
            or_filters: vec![],
            text_search: None,
            expr: None,
        }
    }
}

/// MongoDB operation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperation {
    Find,
    Count,
    Aggregate,
    Distinct,
    Insert,
    Update,
    Delete,
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
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    /// Returns true if this is a write operation (insert/update/delete).
    pub fn is_write(&self) -> bool {
        matches!(self, Self::Insert | Self::Update | Self::Delete)
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
    Exists,
    All,
    Size,
    Type,
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

/// Group-by specification for aggregate $group stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSpec {
    /// Field to group by (becomes _id). Use "null" for whole-collection aggregation.
    pub field: String,
    /// Accumulator expressions.
    #[serde(default)]
    pub accumulators: Vec<Accumulator>,
}

/// A single accumulator in a $group stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accumulator {
    /// Output field name in the grouped result.
    pub output_field: String,
    /// Accumulator operator: $sum, $avg, $min, $max, $first, $last, $push, $addToSet, $count.
    pub operator: String,
    /// Input field path (e.g. "amount"). Use "1" for $sum counting.
    #[serde(default)]
    pub input_field: String,
}

/// Lookup specification for aggregate $lookup stage (cross-collection join).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupSpec {
    /// Foreign collection to join.
    pub from: String,
    /// Field in the local collection.
    pub local_field: String,
    /// Field in the foreign collection.
    pub foreign_field: String,
    /// Output array field name.
    pub as_field: String,
}

/// Bucket specification for aggregate $bucket stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSpec {
    /// Field to bucket by.
    pub group_by: String,
    /// Boundary values for buckets.
    pub boundaries: Vec<serde_json::Value>,
    /// Label for values outside boundaries.
    #[serde(default)]
    pub default_bucket: Option<String>,
}

/// Schema definition for a MongoDB collection.
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

    /// Indexes on this collection (discovered at runtime).
    #[serde(default)]
    pub indexes: Vec<IndexInfo>,

    /// Source of this schema: "yaml", "introspected", or "merged".
    #[serde(default = "default_schema_source")]
    pub source: String,
}

fn default_schema_source() -> String {
    "yaml".to_string()
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

/// MongoDB index information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    /// Index name (e.g. "director_1_year_-1").
    pub name: String,
    /// Index key fields and their sort direction (1 or -1).
    pub keys: Vec<IndexKey>,
    /// Whether this is a unique index.
    #[serde(default)]
    pub unique: bool,
}

/// A single key within an index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexKey {
    pub field: String,
    pub direction: i32,
}

/// Index usage analysis for a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexAnalysis {
    /// Whether the query can use an index.
    pub index_used: bool,
    /// Name of the matching index, if any.
    pub matching_index: Option<String>,
    /// Fields in the query that are NOT indexed.
    pub unindexed_fields: Vec<String>,
    /// Suggestions for improving query performance.
    pub suggestions: Vec<String>,
}

/// Incoming query request from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    /// Natural language query string.
    pub query: String,

    /// Optional export format.
    #[serde(default)]
    pub format: Option<ExportFormat>,

    /// Optional model override (e.g. "qwen2.5-coder:7b"). Uses default if omitted.
    #[serde(default)]
    pub model: Option<String>,

    /// If true, only parse and validate — don't execute the query.
    #[serde(default)]
    pub preview: bool,
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

    /// Index usage analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_analysis: Option<IndexAnalysis>,

    /// Query advice: warnings, suggestions, best practice tips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<crate::domain::query_advisor::QueryAdvice>,

    /// Whether this is a write operation requiring confirmation.
    #[serde(default)]
    pub requires_confirmation: bool,

    /// Number of documents affected by write operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_count: Option<u64>,
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

/// Result of a write operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    pub operation: String,
    pub collection: String,
    pub matched_count: u64,
    pub modified_count: u64,
    pub inserted_id: Option<String>,
}

/// Confirmation request — client sends this back to execute a write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmRequest {
    /// The original parsed query to execute.
    pub parsed: ParsedQuery,
    /// Confirmation token (prevents replay).
    pub confirm: bool,
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
        assert!(query.document.is_none());
        assert!(query.update_fields.is_none());
        assert!(!query.update_many);
        assert!(!query.delete_many);
    }

    #[test]
    fn test_filter_operator_variants() {
        let json = r#"{"field":"x","operator":"contains","value":"test"}"#;
        let f: Filter = serde_json::from_str(json).unwrap();
        assert_eq!(f.operator, FilterOperator::Contains);
    }
}
