use mongodb::bson::Document;
use crate::error::AppError;

/// Convert a slice of BSON documents to CSV format.
///
/// Headers are auto-detected from the first document's keys.
/// Nested documents are flattened with dot notation.
pub fn documents_to_csv(docs: &[Document]) -> Result<String, AppError> {
    if docs.is_empty() {
        return Ok(String::new());
    }

    // Collect all unique field names from the first doc
    let headers = collect_headers(&docs[0]);

    let mut wtr = csv::Writer::from_writer(Vec::new());

    // Write header row
    wtr.write_record(&headers)
        .map_err(|e| AppError::Export(format!("CSV header write failed: {e}")))?;

    // Write data rows
    for doc in docs {
        let flat = flatten_document(doc, "");
        let row: Vec<String> = headers
            .iter()
            .map(|h| flat.get(h.as_str()).cloned().unwrap_or_default())
            .collect();
        wtr.write_record(&row)
            .map_err(|e| AppError::Export(format!("CSV row write failed: {e}")))?;
    }

    let bytes = wtr
        .into_inner()
        .map_err(|e| AppError::Export(format!("CSV flush failed: {e}")))?;

    String::from_utf8(bytes).map_err(|e| AppError::Export(format!("CSV encoding error: {e}")))
}

/// Convert a slice of BSON documents to pretty-printed JSON.
pub fn documents_to_json_pretty(docs: &[Document]) -> Result<String, AppError> {
    let values: Vec<serde_json::Value> = docs
        .iter()
        .map(|doc| {
            let bson = mongodb::bson::Bson::Document(doc.clone());
            bson.into_relaxed_extjson()
        })
        .collect();

    serde_json::to_string_pretty(&values)
        .map_err(|e| AppError::Export(format!("JSON serialization failed: {e}")))
}

/// Collect all headers from a document, flattening nested structures.
fn collect_headers(doc: &Document) -> Vec<String> {
    let mut headers = Vec::new();
    collect_headers_recursive(doc, "", &mut headers);
    headers
}

fn collect_headers_recursive(doc: &Document, prefix: &str, headers: &mut Vec<String>) {
    for (key, value) in doc {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        match value {
            mongodb::bson::Bson::Document(nested) => {
                collect_headers_recursive(nested, &full_key, headers);
            }
            _ => {
                headers.push(full_key);
            }
        }
    }
}

/// Flatten a BSON document to a map of dotted-path keys to string values.
fn flatten_document(doc: &Document, prefix: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();

    for (key, value) in doc {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        match value {
            mongodb::bson::Bson::Document(nested) => {
                map.extend(flatten_document(nested, &full_key));
            }
            mongodb::bson::Bson::Array(arr) => {
                let formatted: Vec<String> = arr.iter().map(bson_to_string).collect();
                map.insert(full_key, format!("[{}]", formatted.join(",")));
            }
            other => {
                map.insert(full_key, bson_to_string(other));
            }
        }
    }

    map
}

/// Convert a single BSON value to a display string.
fn bson_to_string(val: &mongodb::bson::Bson) -> String {
    match val {
        mongodb::bson::Bson::Null => String::new(),
        mongodb::bson::Bson::String(s) => s.clone(),
        mongodb::bson::Bson::Int32(n) => n.to_string(),
        mongodb::bson::Bson::Int64(n) => n.to_string(),
        mongodb::bson::Bson::Double(n) => n.to_string(),
        mongodb::bson::Bson::Boolean(b) => b.to_string(),
        mongodb::bson::Bson::ObjectId(oid) => oid.to_hex(),
        mongodb::bson::Bson::DateTime(dt) => {
            dt.try_to_rfc3339_string().unwrap_or_else(|_| dt.to_string())
        }
        other => format!("{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn test_empty_docs_csv() {
        let result = documents_to_csv(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_simple_csv() {
        let docs = vec![
            doc! { "name": "Alice", "age": 30 },
            doc! { "name": "Bob", "age": 25 },
        ];
        let csv = documents_to_csv(&docs).unwrap();
        assert!(csv.contains("name,age"));
        assert!(csv.contains("Alice,30"));
        assert!(csv.contains("Bob,25"));
    }

    #[test]
    fn test_nested_csv() {
        let docs = vec![doc! { "name": "Alice", "address": { "city": "NYC" } }];
        let csv = documents_to_csv(&docs).unwrap();
        assert!(csv.contains("address.city"));
        assert!(csv.contains("NYC"));
    }

    #[test]
    fn test_json_pretty() {
        let docs = vec![doc! { "name": "Alice", "age": 30 }];
        let json = documents_to_json_pretty(&docs).unwrap();
        assert!(json.contains("Alice"));
        assert!(json.contains("30"));
    }

    #[test]
    fn test_bson_to_string() {
        assert_eq!(bson_to_string(&mongodb::bson::Bson::Null), "");
        assert_eq!(
            bson_to_string(&mongodb::bson::Bson::String("hello".into())),
            "hello"
        );
        assert_eq!(bson_to_string(&mongodb::bson::Bson::Int32(42)), "42");
        assert_eq!(bson_to_string(&mongodb::bson::Bson::Boolean(true)), "true");
    }

    #[test]
    fn bson_to_string_int64() {
        assert_eq!(bson_to_string(&mongodb::bson::Bson::Int64(9999)), "9999");
    }

    #[test]
    fn bson_to_string_double() {
        assert_eq!(bson_to_string(&mongodb::bson::Bson::Double(3.14)), "3.14");
    }

    #[test]
    fn bson_to_string_objectid() {
        use mongodb::bson::oid::ObjectId;
        let oid = ObjectId::new();
        let s = bson_to_string(&mongodb::bson::Bson::ObjectId(oid));
        assert_eq!(s.len(), 24);
    }

    #[test]
    fn bson_to_string_datetime() {
        let dt = mongodb::bson::DateTime::from_millis(0);
        let s = bson_to_string(&mongodb::bson::Bson::DateTime(dt));
        assert!(s.contains("1970"));
    }

    #[test]
    fn csv_with_array_field_formats_inline() {
        let docs = vec![doc! { "tags": ["a", "b", "c"] }];
        let csv = documents_to_csv(&docs).unwrap();
        assert!(csv.contains("tags"));
        assert!(csv.contains("[a,b,c]"));
    }

    #[test]
    fn csv_doc_with_multiple_docs_different_fields() {
        // Multiple docs where second doc has an extra field not in first
        let docs = vec![
            doc! { "name": "Alice", "age": 30 },
            doc! { "name": "Bob", "city": "NY" },
        ];
        let csv = documents_to_csv(&docs).unwrap();
        assert!(csv.contains("name"));
    }

    #[test]
    fn flatten_document_handles_array() {
        let doc = doc! { "tags": ["x", "y"] };
        let flat = flatten_document(&doc, "");
        assert!(flat.contains_key("tags"));
        assert!(flat["tags"].contains("x"));
    }
}
