use mongodb::bson::{Bson, Document, doc};
use crate::domain::models::{ParsedQuery, QueryOperation, Filter, FilterOperator};
use crate::error::AppError;
use crate::infra::mongo::MongoPool;

/// Result of executing a MongoDB query.
#[derive(Debug)]
pub enum QueryResult {
    Documents(Vec<Document>),
    Count(u64),
    Values(Vec<Bson>),
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
