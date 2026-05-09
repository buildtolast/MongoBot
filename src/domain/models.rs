use serde::{Deserialize, Serialize};
use mongodb::bson::oid::ObjectId;
use chrono::{DateTime, Utc};
/// A parsed MongoDB query from natural language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Native MongoDB aggregate pipeline stages. When present, takes priority over
    /// structured fields (group_by, lookup, unwind, etc.) in the executor.
    #[serde(default)]
    pub raw_pipeline: Option<Vec<serde_json::Value>>,

    /// Native MongoDB filter document. When present, takes priority over structured
    /// filters/or_filters/text_search/expr in the executor.
    #[serde(default)]
    pub raw_filter: Option<serde_json::Value>,
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
            raw_pipeline: None,
            raw_filter: None,
        }
    }
}

/// MongoDB operation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperation {
    #[default]
    Find,
    Count,
    Aggregate,
    Distinct,
    Insert,
    Update,
    Delete,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupSpec {
    /// Field to group by (becomes _id). Use "null" for whole-collection aggregation.
    pub field: String,
    /// Accumulator expressions.
    #[serde(default)]
    pub accumulators: Vec<Accumulator>,
}

/// A single accumulator in a $group stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Optional provider override (e.g. "ollama", "anthropic"). Uses default if omitted.
    #[serde(default)]
    pub provider: Option<String>,

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

    /// Raw JSON response from the LLM before parsing/validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_llm_response: Option<String>,

    /// Per-step execution timing breakdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<ExecutionTiming>,
    /// A snapshot of the schema used for the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_snapshot: Option<String>,
}

/// Per-step execution timing breakdown for the query pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTiming {
    /// Time spent parsing NL query via LLM (ms).
    pub llm_parse_ms: u64,
    /// Model that was actually used for LLM parsing.
    pub model_used: String,
    /// Time spent validating query safety (ms).
    pub safety_check_ms: u64,
    /// Time spent executing against MongoDB (ms).
    pub mongo_exec_ms: u64,
    /// Time spent building response (ms).
    pub response_build_ms: u64,
    /// Total wall-clock time (ms).
    pub total_ms: u64,
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
    /// The user-edited version of the parsed query (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_parsed: Option<ParsedQuery>,
    /// Confirmation token (prevents replay).
    pub confirm: bool,
    /// Context for auto-capturing feedback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<FeedbackContext>,
}

/// Context provided by the client to allow the server to auto-capture feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackContext {
    pub nl_query: String,
    pub provider: String,
    pub model: String,
    pub raw_llm_response: Option<String>,
    pub mongo_query: String,
    pub result_count: usize,
}

/// Feedback type for a query.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackType {
    /// User explicitly liked the result.
    Liked,
    /// User explicitly disliked the result.
    Disliked,
    /// User edited the parsed query and then approved it.
    Corrected,
    /// User rejected the query in preview mode without editing.
    Abandoned,
}

/// A request to submit feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackRequest {
    /// The type of feedback.
    pub feedback: FeedbackType,
    /// The natural language query that was used.
    pub nl_query: String,
    /// The provider used.
    pub provider: String,
    /// The model used.
    pub model: String,
    /// The raw LLM response.
    pub raw_llm_response: Option<String>,
    /// The parsed query that resulted from the NL query.
    pub parsed_query: ParsedQuery,
    /// The user-edited version of the parsed query (if any).
    pub user_edited_query: Option<ParsedQuery>,
    /// The actual MongoDB query that was executed.
    pub mongo_query: String,
    /// Whether the query was executed.
    pub executed: bool,
    /// Number of results returned.
    pub result_count: usize,
    /// Optional freeform note from the user.
    pub note: Option<String>,
    /// A snapshot of the schema at the time of the query.
    pub schema_snapshot: String,
}

/// A record of feedback stored in MongoDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryFeedback {
    /// MongoDB ObjectId.
    #[serde(rename = "_id")]
    pub id: ObjectId,
    /// Timestamp of the feedback.
    pub timestamp: DateTime<Utc>,
    /// The natural language query.
    pub nl_query: String,
    /// The provider used.
    pub provider: String,
    /// The model used.
    pub model: String,
    /// The raw LLM response.
    pub raw_llm_response: Option<String>,
    /// The parsed query.
    pub parsed_query: ParsedQuery,
    /// The user-edited version of the parsed query (if any).
    pub user_edited_query: Option<ParsedQuery>,
    /// The actual MongoDB query.
    pub mongo_query: String,
    /// The type of feedback.
    pub feedback: FeedbackType,
    /// Optional freeform note.
    pub note: Option<String>,
    /// Whether the query was executed.
    pub executed: bool,
    /// Number of results returned.
    pub result_count: usize,
    /// A snapshot of the schema.
    pub schema_snapshot: String,
    /// Embedding for Phase 2 retrieval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_feedback_type_serialization() {
        let val = json!("corrected");
        let f: FeedbackType = serde_json::from_value(val).unwrap();
        assert_eq!(f, FeedbackType::Corrected);

        let val = json!("liked");
        let f: FeedbackType = serde_json::from_value(val).unwrap();
        assert_eq!(f, FeedbackType::Liked);
    }

    #[test]
    fn test_query_feedback_serialization() {
        let feedback = QueryFeedback {
            id: mongodb::bson::oid::ObjectId::new(),
            timestamp: Utc::now(),
            nl_query: "find all movies".to_string(),
            provider: "ollama".to_string(),
            model: "qwen3-coder:30b".to_string(),
            raw_llm_response: Some("{}".to_string()),
            parsed_query: ParsedQuery::default(),
            user_edited_query: None,
            mongo_query: "db.movies.find({})".to_string(),
            feedback: FeedbackType::Liked,
            note: Some("Good".to_string()),
            executed: true,
            result_count: 10,
            schema_snapshot: "movies".to_string(),
            embedding: None,
        };

        let serialized = serde_json::to_string(&feedback).unwrap();
        let deserialized: QueryFeedback = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.nl_query, feedback.nl_query);
        assert_eq!(deserialized.feedback, feedback.feedback);
        assert_eq!(deserialized.result_count, feedback.result_count);
    }
}
