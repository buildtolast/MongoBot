use mongodb::bson::{Bson, Document, doc};
use crate::domain::models::{ParsedQuery, QueryOperation, Filter, FilterOperator, CollectionSchema, IndexAnalysis};
use crate::error::AppError;
use crate::infra::mongo::MongoPool;

/// Data from a write operation.
#[derive(Debug)]
pub struct WriteResultData {
    pub operation: String,
    pub collection: String,
    pub matched_count: u64,
    pub modified_count: u64,
    pub inserted_id: Option<String>,
}

/// Result of executing a MongoDB query.
#[derive(Debug)]
pub enum QueryResult {
    Documents(Vec<Document>),
    Count(u64),
    Values(Vec<Bson>),
    Write(WriteResultData),
}

/// Execute a parsed query against MongoDB.
pub async fn execute(pool: &MongoPool, query: &ParsedQuery) -> Result<QueryResult, AppError> {
    let filter = build_filter(&query.filters)?;

    match query.operation {
        QueryOperation::Find => {
            let sort = query.sort.as_ref().map(|s| {
                doc! { &s.field: s.order }
            });

            let projection = if query.projection.is_empty() {
                None
            } else {
                let mut proj_doc = Document::new();
                for field in &query.projection {
                    proj_doc.insert(field.as_str(), 1);
                }
                Some(proj_doc)
            };

            let docs = pool
                .find_documents(
                    &query.collection,
                    Some(filter),
                    sort,
                    Some(query.limit as i64),
                    projection,
                )
                .await?;

            Ok(QueryResult::Documents(docs))
        }

        QueryOperation::Count => {
            let count = pool
                .count_documents(&query.collection, Some(filter))
                .await?;
            Ok(QueryResult::Count(count))
        }

        QueryOperation::Distinct => {
            // For distinct, use the first projection field or first filter field
            let field = query
                .projection
                .first()
                .or_else(|| query.filters.first().map(|f| &f.field))
                .ok_or_else(|| {
                    AppError::Validation(
                        "Distinct operation requires a field (via projection or filter)".into(),
                    )
                })?;

            let values = pool
                .distinct_values(&query.collection, field, Some(filter))
                .await?;
            Ok(QueryResult::Values(values))
        }

        QueryOperation::Aggregate => {
            // Build a simple pipeline from filters + sort + limit
            let mut pipeline = Vec::new();

            if !query.filters.is_empty() {
                pipeline.push(doc! { "$match": &filter });
            }

            if let Some(ref sort_spec) = query.sort {
                pipeline.push(doc! { "$sort": { &sort_spec.field: sort_spec.order } });
            }

            pipeline.push(doc! { "$limit": query.limit as i64 });

            if !query.projection.is_empty() {
                let mut proj = Document::new();
                for field in &query.projection {
                    proj.insert(field.as_str(), 1);
                }
                pipeline.push(doc! { "$project": proj });
            }

            let coll = pool.collection(&query.collection);
            let mut cursor = coll.aggregate(pipeline).await?;

            let mut results = Vec::new();
            use futures::TryStreamExt;
            while let Some(d) = cursor.try_next().await? {
                results.push(d);
            }

            Ok(QueryResult::Documents(results))
        }

        QueryOperation::Insert => {
            let doc_json = query.document.as_ref().ok_or_else(|| {
                AppError::Validation("Insert operation requires a 'document' field".into())
            })?;
            let bson_val = json_to_bson(doc_json)?;
            let document = match bson_val {
                Bson::Document(d) => d,
                _ => return Err(AppError::Validation("Insert document must be a JSON object".into())),
            };
            let inserted_id = pool.insert_document(&query.collection, document).await?;
            Ok(QueryResult::Write(WriteResultData {
                operation: "insert".to_string(),
                collection: query.collection.clone(),
                matched_count: 0,
                modified_count: 0,
                inserted_id: Some(inserted_id),
            }))
        }

        QueryOperation::Update => {
            let update_json = query.update_fields.as_ref().ok_or_else(|| {
                AppError::Validation("Update operation requires 'update_fields'".into())
            })?;
            let bson_val = json_to_bson(update_json)?;
            let update_doc = match bson_val {
                Bson::Document(d) => d,
                _ => return Err(AppError::Validation("update_fields must be a JSON object".into())),
            };
            let (matched, modified) = pool
                .update_documents(&query.collection, filter, update_doc, query.update_many)
                .await?;
            Ok(QueryResult::Write(WriteResultData {
                operation: "update".to_string(),
                collection: query.collection.clone(),
                matched_count: matched,
                modified_count: modified,
                inserted_id: None,
            }))
        }

        QueryOperation::Delete => {
            let deleted = pool
                .delete_documents(&query.collection, filter, query.delete_many)
                .await?;
            Ok(QueryResult::Write(WriteResultData {
                operation: "delete".to_string(),
                collection: query.collection.clone(),
                matched_count: deleted,
                modified_count: deleted,
                inserted_id: None,
            }))
        }
    }
}

/// Build a BSON filter document from a list of Filter conditions.
pub(crate) fn build_filter(filters: &[Filter]) -> Result<Document, AppError> {
    let mut doc = Document::new();

    for filter in filters {
        let bson_value = json_to_bson(&filter.value)?;

        let condition = match filter.operator {
            FilterOperator::Eq => bson_value,
            FilterOperator::Ne => Bson::Document(doc! { "$ne": bson_value }),
            FilterOperator::Gt => Bson::Document(doc! { "$gt": bson_value }),
            FilterOperator::Gte => Bson::Document(doc! { "$gte": bson_value }),
            FilterOperator::Lt => Bson::Document(doc! { "$lt": bson_value }),
            FilterOperator::Lte => Bson::Document(doc! { "$lte": bson_value }),
            FilterOperator::In => {
                let arr = to_bson_array(&filter.value)?;
                Bson::Document(doc! { "$in": arr })
            }
            FilterOperator::Nin => {
                let arr = to_bson_array(&filter.value)?;
                Bson::Document(doc! { "$nin": arr })
            }
            FilterOperator::Contains => {
                let pattern = filter
                    .value
                    .as_str()
                    .ok_or_else(|| {
                        AppError::Validation("Contains operator requires a string value".into())
                    })?;
                let escaped = regex_escape(pattern);
                Bson::Document(doc! { "$regex": escaped, "$options": "i" })
            }
            FilterOperator::Regex => {
                let pattern = filter
                    .value
                    .as_str()
                    .ok_or_else(|| {
                        AppError::Validation("Regex operator requires a string value".into())
                    })?;
                Bson::Document(doc! { "$regex": pattern, "$options": "i" })
            }
        };

        doc.insert(filter.field.clone(), condition);
    }

    Ok(doc)
}

/// Convert a serde_json::Value to a BSON value.
pub(crate) fn json_to_bson(value: &serde_json::Value) -> Result<Bson, AppError> {
    match value {
        serde_json::Value::Null => Ok(Bson::Null),
        serde_json::Value::Bool(b) => Ok(Bson::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Bson::Int64(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Bson::Double(f))
            } else {
                Err(AppError::Validation(format!("Unsupported number: {n}")))
            }
        }
        serde_json::Value::String(s) => Ok(Bson::String(s.clone())),
        serde_json::Value::Array(arr) => {
            let bson_arr: Result<Vec<Bson>, _> = arr.iter().map(json_to_bson).collect();
            Ok(Bson::Array(bson_arr?))
        }
        serde_json::Value::Object(map) => {
            let mut bson_doc = Document::new();
            for (k, v) in map {
                bson_doc.insert(k.clone(), json_to_bson(v)?);
            }
            Ok(Bson::Document(bson_doc))
        }
    }
}

/// Convert a JSON value (expected array) to a BSON array.
fn to_bson_array(value: &serde_json::Value) -> Result<Bson, AppError> {
    match value {
        serde_json::Value::Array(arr) => {
            let bson_arr: Result<Vec<Bson>, _> = arr.iter().map(json_to_bson).collect();
            Ok(Bson::Array(bson_arr?))
        }
        _ => Err(AppError::Validation(
            "In/Nin operators require an array value".into(),
        )),
    }
}

/// Build a MongoDB shell command string from a ParsedQuery (for display in the UI).
pub fn build_mongo_shell_command(query: &ParsedQuery) -> String {
    let filter = build_filter(&query.filters).unwrap_or_default();
    let filter_str = format_bson_doc(&filter);

    match query.operation {
        QueryOperation::Find => {
            let mut cmd = format!("db.{}.find({})", query.collection, filter_str);

            if !query.projection.is_empty() {
                let proj: Vec<String> = query.projection.iter()
                    .map(|f| format!("{}: 1", f))
                    .collect();
                cmd = format!("db.{}.find({}, {{ {} }})", query.collection, filter_str, proj.join(", "));
            }

            if let Some(ref sort) = query.sort {
                cmd.push_str(&format!(".sort({{ {}: {} }})", sort.field, sort.order));
            }

            cmd.push_str(&format!(".limit({})", query.limit));
            cmd
        }
        QueryOperation::Count => {
            format!("db.{}.countDocuments({})", query.collection, filter_str)
        }
        QueryOperation::Distinct => {
            let field = query.projection.first()
                .or_else(|| query.filters.first().map(|f| &f.field))
                .map(|s| s.as_str())
                .unwrap_or("_id");
            format!("db.{}.distinct(\"{}\", {})", query.collection, field, filter_str)
        }
        QueryOperation::Aggregate => {
            let mut stages = Vec::new();
            if !query.filters.is_empty() {
                stages.push(format!("{{ $match: {} }}", filter_str));
            }
            if let Some(ref sort) = query.sort {
                stages.push(format!("{{ $sort: {{ {}: {} }} }}", sort.field, sort.order));
            }
            stages.push(format!("{{ $limit: {} }}", query.limit));
            if !query.projection.is_empty() {
                let proj: Vec<String> = query.projection.iter()
                    .map(|f| format!("{}: 1", f))
                    .collect();
                stages.push(format!("{{ $project: {{ {} }} }}", proj.join(", ")));
            }
            format!("db.{}.aggregate([\n  {}\n])", query.collection, stages.join(",\n  "))
        }

        QueryOperation::Insert => {
            let doc_str = query
                .document
                .as_ref()
                .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".to_string()))
                .unwrap_or_else(|| "{}".to_string());
            format!("db.{}.insertOne({})", query.collection, doc_str)
        }

        QueryOperation::Update => {
            let update_str = query
                .update_fields
                .as_ref()
                .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".to_string()))
                .unwrap_or_else(|| "{}".to_string());
            let method = if query.update_many { "updateMany" } else { "updateOne" };
            format!(
                "db.{}.{}({}, {{ $set: {} }})",
                query.collection, method, filter_str, update_str
            )
        }

        QueryOperation::Delete => {
            let method = if query.delete_many { "deleteMany" } else { "deleteOne" };
            format!("db.{}.{}({})", query.collection, method, filter_str)
        }
    }
}

/// Format a BSON Document as a readable shell-style string.
fn format_bson_doc(doc: &Document) -> String {
    if doc.is_empty() {
        return "{}".to_string();
    }
    // Use the BSON relaxed extended JSON format for readability
    let json = serde_json::to_value(doc)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|_| "{}".to_string());
    // Compact it to single line for shell display
    json.replace('\n', " ").replace("  ", " ")
}

/// Analyze whether a query can use available indexes.
pub fn analyze_index_usage(query: &ParsedQuery, schema: Option<&CollectionSchema>) -> IndexAnalysis {
    let schema = match schema {
        Some(s) => s,
        None => return IndexAnalysis {
            index_used: false,
            matching_index: None,
            unindexed_fields: query.filters.iter().map(|f| f.field.clone()).collect(),
            suggestions: vec!["No schema available — cannot analyze index usage".to_string()],
        },
    };

    if schema.indexes.is_empty() {
        return IndexAnalysis {
            index_used: false,
            matching_index: None,
            unindexed_fields: query.filters.iter().map(|f| f.field.clone()).collect(),
            suggestions: vec!["No indexes found on this collection".to_string()],
        };
    }

    // Collect all fields used in filters and sort
    let filter_fields: Vec<&str> = query.filters.iter().map(|f| f.field.as_str()).collect();
    let sort_field: Option<&str> = query.sort.as_ref().map(|s| s.field.as_str());

    // Check each index to see if it covers query fields
    let mut best_match: Option<(String, usize)> = None; // (name, matched_fields_count)

    for idx in &schema.indexes {
        if idx.name == "_id_" {
            continue; // Skip default _id index
        }

        let idx_fields: Vec<&str> = idx.keys.iter().map(|k| k.field.as_str()).collect();

        // An index is useful if its prefix matches query filter/sort fields
        // MongoDB uses leftmost prefix matching
        let first_idx_field = idx_fields.first().copied().unwrap_or("");

        let mut matched = 0usize;

        // Check if filter fields match index prefix
        for ff in &filter_fields {
            if idx_fields.contains(ff) {
                matched += 1;
            }
        }

        // Check if sort field is in the index
        if let Some(sf) = sort_field {
            if idx_fields.contains(&sf) {
                matched += 1;
            }
        }

        // Index is usable if at least the first key matches a filter field
        if filter_fields.contains(&first_idx_field)
            || sort_field == Some(first_idx_field)
        {
            if best_match.as_ref().map_or(true, |(_, best)| matched > *best) {
                best_match = Some((idx.name.clone(), matched));
            }
        }
    }

    // Find fields NOT covered by any index
    let all_indexed_fields: std::collections::HashSet<&str> = schema
        .indexes
        .iter()
        .flat_map(|idx| idx.keys.iter().map(|k| k.field.as_str()))
        .collect();

    let unindexed_fields: Vec<String> = filter_fields
        .iter()
        .filter(|f| !all_indexed_fields.contains(**f))
        .map(|f| f.to_string())
        .collect();

    let mut suggestions = Vec::new();

    if best_match.is_none() && !filter_fields.is_empty() {
        suggestions.push(format!(
            "Consider creating an index on: {}",
            filter_fields.join(", ")
        ));
    }

    if !unindexed_fields.is_empty() {
        suggestions.push(format!(
            "Fields not indexed: {} — query may require a collection scan",
            unindexed_fields.join(", ")
        ));
    }

    if let Some(sf) = sort_field {
        if !all_indexed_fields.contains(sf) && best_match.is_none() {
            suggestions.push(format!(
                "Sort field '{}' is not indexed — consider adding an index for faster sorting",
                sf
            ));
        }
    }

    IndexAnalysis {
        index_used: best_match.is_some(),
        matching_index: best_match.map(|(name, _)| name),
        unindexed_fields,
        suggestions,
    }
}

/// Escape regex special characters for a literal substring match.
fn regex_escape(s: &str) -> String {
    let special = [
        '.', '^', '$', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '\\',
    ];
    let mut escaped = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if special.contains(&c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_empty_filter() {
        let doc = build_filter(&[]).unwrap();
        assert!(doc.is_empty());
    }

    #[test]
    fn test_build_eq_filter() {
        let filters = vec![Filter {
            field: "name".into(),
            operator: FilterOperator::Eq,
            value: serde_json::Value::String("Alice".into()),
        }];
        let doc = build_filter(&filters).unwrap();
        assert_eq!(doc.get_str("name").unwrap(), "Alice");
    }

    #[test]
    fn test_build_gt_filter() {
        let filters = vec![Filter {
            field: "age".into(),
            operator: FilterOperator::Gt,
            value: serde_json::json!(25),
        }];
        let doc = build_filter(&filters).unwrap();
        let condition = doc.get_document("age").unwrap();
        assert_eq!(condition.get_i64("$gt").unwrap(), 25);
    }

    #[test]
    fn test_json_to_bson_string() {
        let val = serde_json::Value::String("hello".into());
        let bson = json_to_bson(&val).unwrap();
        assert_eq!(bson, Bson::String("hello".into()));
    }

    #[test]
    fn test_json_to_bson_number() {
        let val = serde_json::json!(42);
        let bson = json_to_bson(&val).unwrap();
        assert_eq!(bson, Bson::Int64(42));
    }

    #[test]
    fn test_json_to_bson_array() {
        let val = serde_json::json!([1, 2, 3]);
        let bson = json_to_bson(&val).unwrap();
        match bson {
            Bson::Array(arr) => assert_eq!(arr.len(), 3),
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_regex_escape() {
        assert_eq!(regex_escape("hello.world"), "hello\\.world");
        assert_eq!(regex_escape("a+b"), "a\\+b");
        assert_eq!(regex_escape("plain"), "plain");
    }

    #[test]
    fn test_contains_filter() {
        let filters = vec![Filter {
            field: "name".into(),
            operator: FilterOperator::Contains,
            value: serde_json::Value::String("test".into()),
        }];
        let doc = build_filter(&filters).unwrap();
        let condition = doc.get_document("name").unwrap();
        assert!(condition.contains_key("$regex"));
        assert!(condition.contains_key("$options"));
    }

    #[test]
    fn test_in_filter_requires_array() {
        let filters = vec![Filter {
            field: "status".into(),
            operator: FilterOperator::In,
            value: serde_json::Value::String("not_array".into()),
        }];
        assert!(build_filter(&filters).is_err());
    }
}
