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
pub async fn execute(pool: &MongoPool, query: &ParsedQuery, schema: Option<&CollectionSchema>) -> Result<QueryResult, AppError> {
    let filter = build_filter(query, schema)?;

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
            // For distinct, try multiple sources for the target field:
            // 1. First projection field (standard)
            // 2. First filter field
            // 3. "field" key in document (some LLMs return {"field": "language"})
            let doc_field = query.document.as_ref()
                .and_then(|d| d.as_object())
                .and_then(|o| o.get("field"))
                .and_then(|v| v.as_str())
                .map(String::from);

            let field = query
                .projection
                .first()
                .or_else(|| query.filters.first().map(|f| &f.field))
                .or(doc_field.as_ref())
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
            // Raw pipeline takes priority — passthrough native stages
            let pipeline = if let Some(ref raw_stages) = query.raw_pipeline {
                raw_stages.iter()
                    .map(|s| match json_to_bson(s)? {
                        Bson::Document(d) => Ok(d),
                        _ => Err(AppError::Validation("Pipeline stage must be an object".into())),
                    })
                    .collect::<Result<Vec<Document>, AppError>>()?
            } else {
                build_aggregate_pipeline(query, &filter)?
            };

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

/// Build a single filter condition as a BSON value.
fn build_single_condition(filter: &Filter) -> Result<Bson, AppError> {
    let bson_value = coerce_value_for_field(&filter.field, &filter.value)?;
    match filter.operator {
        FilterOperator::Eq => Ok(bson_value),
        FilterOperator::Ne => Ok(Bson::Document(doc! { "$ne": bson_value })),
        FilterOperator::Gt => Ok(Bson::Document(doc! { "$gt": bson_value })),
        FilterOperator::Gte => Ok(Bson::Document(doc! { "$gte": bson_value })),
        FilterOperator::Lt => Ok(Bson::Document(doc! { "$lt": bson_value })),
        FilterOperator::Lte => Ok(Bson::Document(doc! { "$lte": bson_value })),
        FilterOperator::In => {
            let arr = to_bson_array(&filter.value)?;
            Ok(Bson::Document(doc! { "$in": arr }))
        }
        FilterOperator::Nin => {
            let arr = to_bson_array(&filter.value)?;
            Ok(Bson::Document(doc! { "$nin": arr }))
        }
        FilterOperator::Contains => {
            let pattern = filter.value.as_str().ok_or_else(|| {
                AppError::Validation("Contains operator requires a string value".into())
            })?;
            Ok(Bson::Document(doc! { "$regex": regex_escape(pattern), "$options": "i" }))
        }
        FilterOperator::Regex => {
            let pattern = filter.value.as_str().ok_or_else(|| {
                AppError::Validation("Regex operator requires a string value".into())
            })?;
            Ok(Bson::Document(doc! { "$regex": pattern, "$options": "i" }))
        }
        FilterOperator::Exists => {
            let exists = filter.value.as_bool().unwrap_or(true);
            Ok(Bson::Document(doc! { "$exists": exists }))
        }
        FilterOperator::All => {
            let arr = to_bson_array(&filter.value)?;
            Ok(Bson::Document(doc! { "$all": arr }))
        }
        FilterOperator::Size => {
            let size = json_to_bson(&filter.value)?;
            Ok(Bson::Document(doc! { "$size": size }))
        }
        FilterOperator::Type => {
            let type_val = json_to_bson(&filter.value)?;
            Ok(Bson::Document(doc! { "$type": type_val }))
        }
    }
}

/// Build a complete BSON filter document from a ParsedQuery, including $or, $text, and $expr.
pub(crate) fn build_filter(query: &ParsedQuery, schema: Option<&CollectionSchema>) -> Result<Document, AppError> {
    // Raw filter takes priority — passthrough native MongoDB filter syntax
    if let Some(ref raw) = query.raw_filter {
        return match json_to_bson(raw)? {
            Bson::Document(d) => Ok(d),
            _ => Err(AppError::Validation("raw_filter must be a JSON object".into())),
        };
    }

    let mut doc = build_and_filter(&query.filters, schema)?;

    // $or conditions
    if !query.or_filters.is_empty() {
        let mut or_branches: Vec<Bson> = Vec::new();
        for branch in &query.or_filters {
            let branch_doc = build_and_filter(branch, schema)?;
            or_branches.push(Bson::Document(branch_doc));
        }
        doc.insert("$or", Bson::Array(or_branches));
    }

    // $text search
    if let Some(ref search) = query.text_search {
        doc.insert("$text", doc! { "$search": search.as_str() });
    }

    // $expr (cross-field comparison)
    if let Some(ref expr_val) = query.expr {
        let bson_expr = json_to_bson(expr_val)?;
        doc.insert("$expr", bson_expr);
    }

    Ok(doc)
}

/// Build AND filter conditions from a list of Filter structs.
/// When schema is available and the prefix field is an array type, multiple filters
/// on the same prefix are grouped under $elemMatch for same-element matching.
/// For non-array nested objects, plain dot notation is used.
fn build_and_filter(filters: &[Filter], schema: Option<&CollectionSchema>) -> Result<Document, AppError> {
    // Group filters by their array prefix.
    // e.g. "transactions.symbol" and "transactions.amount" share prefix "transactions"
    // while "name" and "year" have no prefix (top-level).
    let mut array_groups: std::collections::BTreeMap<String, Vec<&Filter>> = std::collections::BTreeMap::new();
    let mut top_level: Vec<&Filter> = Vec::new();

    for filter in filters {
        if let Some(dot_pos) = filter.field.find('.') {
            let prefix = &filter.field[..dot_pos];
            array_groups.entry(prefix.to_string()).or_default().push(filter);
        } else {
            top_level.push(filter);
        }
    }

    let mut doc = Document::new();

    // Group top-level filters by field name to handle same-field ranges
    // e.g. price > 10 AND price < 100 → { price: { $gt: 10, $lt: 100 } }
    let mut field_groups: std::collections::BTreeMap<String, Vec<&Filter>> = std::collections::BTreeMap::new();
    for filter in &top_level {
        field_groups.entry(filter.field.clone()).or_default().push(filter);
    }

    for (field, group) in &field_groups {
        if group.len() == 1 {
            let condition = build_single_condition(group[0])?;
            doc.insert(field.clone(), condition);
        } else {
            // Multiple conditions on same field — merge into single sub-document
            // e.g. { $gt: 10, $lt: 100 } for range queries
            let mut merged = Document::new();
            let mut has_eq = false;
            for filter in group {
                let condition = build_single_condition(filter)?;
                match condition {
                    Bson::Document(sub) => {
                        for (k, v) in sub {
                            merged.insert(k, v);
                        }
                    }
                    _ => {
                        // Eq condition — can't merge with operators, use $and
                        has_eq = true;
                        break;
                    }
                }
            }
            if has_eq || merged.is_empty() {
                // Fall back to $and for eq + operator combos
                let mut and_clauses: Vec<Bson> = Vec::new();
                for filter in group {
                    let condition = build_single_condition(filter)?;
                    and_clauses.push(Bson::Document(doc! { field.clone(): condition }));
                }
                // Add to existing $and or create new one
                if let Some(existing_and) = doc.get_mut("$and") {
                    if let Bson::Array(arr) = existing_and {
                        arr.extend(and_clauses);
                    }
                } else {
                    doc.insert("$and".to_string(), Bson::Array(and_clauses));
                }
            } else {
                doc.insert(field.clone(), Bson::Document(merged));
            }
        }
    }

    // For dotted fields: check schema to decide $elemMatch (array) vs dot notation (object).
    // If schema says the prefix is an array AND there are 2+ filters, use $elemMatch.
    // Otherwise, use plain dot notation with range merging.
    for (prefix, group_filters) in &array_groups {
        let is_array = schema
            .and_then(|s| s.fields.iter().find(|f| f.name == *prefix))
            .map_or(false, |f| f.field_type == "array");

        if is_array && group_filters.len() >= 2 {
            // Array field with multiple conditions — use $elemMatch
            let mut elem_match = Document::new();
            for filter in group_filters {
                let inner_field = &filter.field[prefix.len() + 1..];
                let condition = build_single_condition(filter)?;
                elem_match.insert(inner_field.to_string(), condition);
            }
            doc.insert(prefix.clone(), doc! { "$elemMatch": elem_match });
        } else {
            // Non-array (nested object) or single filter — use dot notation
            let mut nested_groups: std::collections::BTreeMap<String, Vec<&Filter>> = std::collections::BTreeMap::new();
            for filter in group_filters {
                nested_groups.entry(filter.field.clone()).or_default().push(filter);
            }
            for (field, group) in &nested_groups {
                if group.len() == 1 {
                    let condition = build_single_condition(group[0])?;
                    doc.insert(field.clone(), condition);
                } else {
                    // Multiple conditions on same dotted field — merge ranges
                    let mut merged = Document::new();
                    let mut has_eq = false;
                    for filter in group {
                        let condition = build_single_condition(filter)?;
                        match condition {
                            Bson::Document(sub) => {
                                for (k, v) in sub {
                                    merged.insert(k, v);
                                }
                            }
                            _ => { has_eq = true; break; }
                        }
                    }
                    if has_eq || merged.is_empty() {
                        let mut and_clauses: Vec<Bson> = Vec::new();
                        for filter in group {
                            let condition = build_single_condition(filter)?;
                            and_clauses.push(Bson::Document(doc! { field.clone(): condition }));
                        }
                        if let Some(existing_and) = doc.get_mut("$and") {
                            if let Bson::Array(arr) = existing_and {
                                arr.extend(and_clauses);
                            }
                        } else {
                            doc.insert("$and".to_string(), Bson::Array(and_clauses));
                        }
                    } else {
                        doc.insert(field.clone(), Bson::Document(merged));
                    }
                }
            }
        }
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
        serde_json::Value::String(s) => {
            // Detect ISO 8601 date strings and convert to BSON DateTime
            // Patterns: "2026-01-01T00:00:00Z", "2026-01-01T00:00:00.000Z",
            //           "2026-01-01", "2026-01-01T00:00:00+00:00"
            if let Some(dt) = try_parse_datetime(s) {
                Ok(dt)
            } else {
                Ok(Bson::String(s.clone()))
            }
        }
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

/// Build a full aggregate pipeline from ParsedQuery structured fields.
/// Order: $match → $lookup → $unwind → $group → $bucket → $facet → $sort → $limit → $project
fn build_aggregate_pipeline(query: &ParsedQuery, match_filter: &Document) -> Result<Vec<Document>, AppError> {
    let mut pipeline = Vec::new();

    // $match — from filters + or_filters + text_search + expr
    if !match_filter.is_empty() {
        pipeline.push(doc! { "$match": match_filter });
    }

    // $lookup — cross-collection join
    if let Some(ref lookup) = query.lookup {
        pipeline.push(doc! {
            "$lookup": {
                "from": &lookup.from,
                "localField": &lookup.local_field,
                "foreignField": &lookup.foreign_field,
                "as": &lookup.as_field,
            }
        });
    }

    // $unwind — flatten array field
    if let Some(ref unwind_field) = query.unwind {
        let path = if unwind_field.starts_with('$') {
            unwind_field.clone()
        } else {
            format!("${unwind_field}")
        };
        pipeline.push(doc! { "$unwind": path });
    }

    // $group — group by + accumulators
    if let Some(ref group) = query.group_by {
        let id_val: Bson = if group.field == "null" || group.field.is_empty() {
            Bson::Null
        } else {
            Bson::String(format!("${}", group.field))
        };

        let mut group_doc = doc! { "_id": id_val };
        for acc in &group.accumulators {
            let acc_val = build_accumulator_expr(&acc.operator, &acc.input_field)?;
            group_doc.insert(acc.output_field.clone(), acc_val);
        }
        pipeline.push(doc! { "$group": group_doc });
    }

    // $bucket — range grouping
    if let Some(ref bucket) = query.bucket {
        let group_by_path = if bucket.group_by.starts_with('$') {
            bucket.group_by.clone()
        } else {
            format!("${}", bucket.group_by)
        };
        let boundaries: Result<Vec<Bson>, _> = bucket.boundaries.iter().map(json_to_bson).collect();
        let mut bucket_doc = doc! {
            "groupBy": group_by_path,
            "boundaries": Bson::Array(boundaries?),
        };
        if let Some(ref default) = bucket.default_bucket {
            bucket_doc.insert("default", default.as_str());
        }
        // Always add count output
        bucket_doc.insert("output", doc! { "count": { "$sum": 1 } });
        pipeline.push(doc! { "$bucket": bucket_doc });
    }

    // $facet — parallel sub-pipelines (raw JSON passthrough)
    if let Some(ref facet_val) = query.facet {
        let bson_facet = json_to_bson(facet_val)?;
        pipeline.push(doc! { "$facet": bson_facet });
    }

    // $sort
    if let Some(ref sort_spec) = query.sort {
        pipeline.push(doc! { "$sort": { &sort_spec.field: sort_spec.order } });
    }

    // $limit
    pipeline.push(doc! { "$limit": query.limit as i64 });

    // $project
    if !query.projection.is_empty() {
        let mut proj = Document::new();
        for field in &query.projection {
            proj.insert(field.as_str(), 1);
        }
        pipeline.push(doc! { "$project": proj });
    }

    Ok(pipeline)
}

/// Build a BSON accumulator expression like { "$sum": "$amount" } or { "$sum": 1 }.
fn build_accumulator_expr(operator: &str, input_field: &str) -> Result<Bson, AppError> {
    // Normalize operator — accept with or without $ prefix
    let op = if operator.starts_with('$') {
        operator.to_string()
    } else {
        format!("${operator}")
    };

    static ALLOWED_ACCUMULATORS: &[&str] = &[
        "$sum", "$avg", "$min", "$max", "$first", "$last", "$push", "$addToSet", "$count",
    ];
    if !ALLOWED_ACCUMULATORS.contains(&op.as_str()) {
        return Err(AppError::Validation(format!(
            "Unsupported accumulator operator: {op}"
        )));
    }

    // $count takes no input field
    if op == "$count" {
        return Ok(Bson::Document(doc! { &op: {} }));
    }

    // "1" means count (e.g. { $sum: 1 })
    let input: Bson = if input_field == "1" || input_field.is_empty() {
        Bson::Int64(1)
    } else if input_field.starts_with('$') {
        Bson::String(input_field.to_string())
    } else {
        Bson::String(format!("${input_field}"))
    };

    Ok(Bson::Document(doc! { &op: input }))
}

/// Build a MongoDB shell command string from a ParsedQuery (for display in the UI).
pub fn build_mongo_shell_command(query: &ParsedQuery) -> String {
    let filter = build_filter(query, None).unwrap_or_default();
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
            // Raw pipeline: render stages directly
            if let Some(ref raw_stages) = query.raw_pipeline {
                let stages: Vec<String> = raw_stages.iter()
                    .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "{}".to_string()))
                    .collect();
                return format!("db.{}.aggregate([\n  {}\n])", query.collection, stages.join(",\n  "));
            }

            let mut stages = Vec::new();

            if !filter.is_empty() {
                stages.push(format!("{{ $match: {} }}", filter_str));
            }

            if let Some(ref lookup) = query.lookup {
                stages.push(format!(
                    "{{ $lookup: {{ from: \"{}\", localField: \"{}\", foreignField: \"{}\", as: \"{}\" }} }}",
                    lookup.from, lookup.local_field, lookup.foreign_field, lookup.as_field
                ));
            }

            if let Some(ref unwind_field) = query.unwind {
                let path = if unwind_field.starts_with('$') {
                    unwind_field.clone()
                } else {
                    format!("${unwind_field}")
                };
                stages.push(format!("{{ $unwind: \"{}\" }}", path));
            }

            if let Some(ref group) = query.group_by {
                let id_str = if group.field == "null" || group.field.is_empty() {
                    "null".to_string()
                } else {
                    format!("\"${}\"", group.field)
                };
                let accs: Vec<String> = group.accumulators.iter().map(|a| {
                    let op = if a.operator.starts_with('$') { a.operator.clone() } else { format!("${}", a.operator) };
                    let input = if a.input_field == "1" || a.input_field.is_empty() {
                        "1".to_string()
                    } else if a.input_field.starts_with('$') {
                        format!("\"{}\"", a.input_field)
                    } else {
                        format!("\"${}\"", a.input_field)
                    };
                    format!("{}: {{ {}: {} }}", a.output_field, op, input)
                }).collect();
                stages.push(format!("{{ $group: {{ _id: {}, {} }} }}", id_str, accs.join(", ")));
            }

            if let Some(ref bucket) = query.bucket {
                let bounds: Vec<String> = bucket.boundaries.iter()
                    .map(|v| v.to_string())
                    .collect();
                let default_str = bucket.default_bucket.as_deref()
                    .map(|d| format!(", default: \"{}\"", d))
                    .unwrap_or_default();
                stages.push(format!(
                    "{{ $bucket: {{ groupBy: \"${}\", boundaries: [{}]{} }} }}",
                    bucket.group_by, bounds.join(", "), default_str
                ));
            }

            if query.facet.is_some() {
                stages.push("{ $facet: { ... } }".to_string());
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
/// Coerce a JSON value based on the field name.
/// If the field looks like a date field and the value is a number/short-string year,
/// convert it to a BSON DateTime.
fn coerce_value_for_field(field: &str, value: &serde_json::Value) -> Result<Bson, AppError> {
    let field_lower = field.to_lowercase();
    let is_date_field = field_lower.contains("date")
        || field_lower.contains("_at")
        || field_lower == "released"
        || field_lower == "timestamp"
        || field_lower.ends_with("time");

    if is_date_field {
        // If value is a number that looks like a year (1900-2100), convert to date
        if let Some(n) = value.as_i64() {
            if (1900..=2100).contains(&n) {
                let iso = format!("{n}-01-01T00:00:00Z");
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&iso) {
                    return Ok(Bson::DateTime(mongodb::bson::DateTime::from_millis(dt.timestamp_millis())));
                }
            }
        }
        // If value is a string, try date parsing (json_to_bson will handle it)
    }

    json_to_bson(value)
}

/// Try to parse a string as an ISO 8601 date and return a BSON DateTime.
/// Returns None if the string doesn't look like a date.
fn try_parse_datetime(s: &str) -> Option<Bson> {
    // Quick check: must start with 4 digits
    if s.len() < 4 || !s[..4].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    // Handle bare year string: "2010" → "2010-01-01T00:00:00Z"
    if s.len() == 4 {
        let with_full = format!("{s}-01-01T00:00:00Z");
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&with_full) {
            let ms = dt.timestamp_millis();
            return Some(Bson::DateTime(mongodb::bson::DateTime::from_millis(ms)));
        }
        return None;
    }

    // Must have dash at position 4 for date formats
    if s.as_bytes().get(4) != Some(&b'-') {
        return None;
    }

    // Try full ISO 8601 with time: "2026-01-01T00:00:00Z" or "2026-01-01T00:00:00.000Z"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        let ms = dt.timestamp_millis();
        return Some(Bson::DateTime(mongodb::bson::DateTime::from_millis(ms)));
    }

    // Try "2026-01-01T00:00:00Z" without timezone offset (append Z)
    if s.contains('T') && !s.ends_with('Z') && !s.contains('+') {
        let with_z = format!("{s}Z");
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&with_z) {
            let ms = dt.timestamp_millis();
            return Some(Bson::DateTime(mongodb::bson::DateTime::from_millis(ms)));
        }
    }

    // Try date-only: "2026-01-01" → midnight UTC
    if s.len() == 10 {
        let with_time = format!("{s}T00:00:00Z");
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&with_time) {
            let ms = dt.timestamp_millis();
            return Some(Bson::DateTime(mongodb::bson::DateTime::from_millis(ms)));
        }
    }

    None
}

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
        let doc = build_and_filter(&[], None).unwrap();
        assert!(doc.is_empty());
    }

    #[test]
    fn test_build_eq_filter() {
        let filters = vec![Filter {
            field: "name".into(),
            operator: FilterOperator::Eq,
            value: serde_json::Value::String("Alice".into()),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
        assert_eq!(doc.get_str("name").unwrap(), "Alice");
    }

    #[test]
    fn test_build_gt_filter() {
        let filters = vec![Filter {
            field: "age".into(),
            operator: FilterOperator::Gt,
            value: serde_json::json!(25),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
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
        let doc = build_and_filter(&filters, None).unwrap();
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
        assert!(build_and_filter(&filters, None).is_err());
    }

    // ── New filter operators ───────────────────────────────────

    #[test]
    fn test_build_exists_filter() {
        let filters = vec![Filter {
            field: "phone".into(),
            operator: FilterOperator::Exists,
            value: serde_json::json!(true),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
        let cond = doc.get_document("phone").unwrap();
        assert_eq!(cond.get_bool("$exists").unwrap(), true);
    }

    #[test]
    fn test_build_all_filter() {
        let filters = vec![Filter {
            field: "tags".into(),
            operator: FilterOperator::All,
            value: serde_json::json!(["rust", "mongodb"]),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
        let cond = doc.get_document("tags").unwrap();
        assert!(cond.contains_key("$all"));
    }

    #[test]
    fn test_build_size_filter() {
        let filters = vec![Filter {
            field: "items".into(),
            operator: FilterOperator::Size,
            value: serde_json::json!(3),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
        let cond = doc.get_document("items").unwrap();
        assert_eq!(cond.get_i64("$size").unwrap(), 3);
    }

    #[test]
    fn test_build_type_filter() {
        let filters = vec![Filter {
            field: "age".into(),
            operator: FilterOperator::Type,
            value: serde_json::json!("number"),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
        let cond = doc.get_document("age").unwrap();
        assert_eq!(cond.get_str("$type").unwrap(), "number");
    }

    // ── $or, $text, $expr via build_filter ─────────────────────

    #[test]
    fn test_build_or_filter() {
        let query = ParsedQuery {
            collection: "orders".into(),
            or_filters: vec![
                vec![Filter {
                    field: "status".into(),
                    operator: FilterOperator::Eq,
                    value: serde_json::json!("pending"),
                }],
                vec![Filter {
                    field: "status".into(),
                    operator: FilterOperator::Eq,
                    value: serde_json::json!("shipped"),
                }],
            ],
            ..Default::default()
        };
        let doc = build_filter(&query, None).unwrap();
        let or_arr = doc.get_array("$or").unwrap();
        assert_eq!(or_arr.len(), 2);
    }

    #[test]
    fn test_build_text_filter() {
        let query = ParsedQuery {
            collection: "articles".into(),
            text_search: Some("mongodb tutorial".into()),
            ..Default::default()
        };
        let doc = build_filter(&query, None).unwrap();
        let text_doc = doc.get_document("$text").unwrap();
        assert_eq!(text_doc.get_str("$search").unwrap(), "mongodb tutorial");
    }

    #[test]
    fn test_build_expr_filter() {
        let query = ParsedQuery {
            collection: "orders".into(),
            expr: Some(serde_json::json!({"$gt": ["$total", "$budget"]})),
            ..Default::default()
        };
        let doc = build_filter(&query, None).unwrap();
        assert!(doc.contains_key("$expr"));
    }

    // ── Aggregate pipeline building ────────────────────────────

    #[test]
    fn test_build_group_pipeline() {
        use crate::domain::models::{GroupSpec, Accumulator};
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            group_by: Some(GroupSpec {
                field: "customer_id".into(),
                accumulators: vec![Accumulator {
                    output_field: "total".into(),
                    operator: "$sum".into(),
                    input_field: "amount".into(),
                }],
            }),
            ..Default::default()
        };
        let filter = build_filter(&query, None).unwrap();
        let pipeline = build_aggregate_pipeline(&query, &filter).unwrap();
        // Should have $group + $limit stages (no $match since no filters)
        assert!(pipeline.iter().any(|s| s.contains_key("$group")));
        assert!(pipeline.iter().any(|s| s.contains_key("$limit")));

        let group_stage = pipeline.iter().find(|s| s.contains_key("$group")).unwrap();
        let group_doc = group_stage.get_document("$group").unwrap();
        assert_eq!(group_doc.get_str("_id").unwrap(), "$customer_id");
        let total = group_doc.get_document("total").unwrap();
        assert_eq!(total.get_str("$sum").unwrap(), "$amount");
    }

    #[test]
    fn test_build_lookup_pipeline() {
        use crate::domain::models::LookupSpec;
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            lookup: Some(LookupSpec {
                from: "customers".into(),
                local_field: "customer_id".into(),
                foreign_field: "_id".into(),
                as_field: "customer".into(),
            }),
            ..Default::default()
        };
        let filter = build_filter(&query, None).unwrap();
        let pipeline = build_aggregate_pipeline(&query, &filter).unwrap();
        assert!(pipeline.iter().any(|s| s.contains_key("$lookup")));
    }

    #[test]
    fn test_build_unwind_pipeline() {
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            unwind: Some("items".into()),
            ..Default::default()
        };
        let filter = build_filter(&query, None).unwrap();
        let pipeline = build_aggregate_pipeline(&query, &filter).unwrap();
        let unwind_stage = pipeline.iter().find(|s| s.contains_key("$unwind")).unwrap();
        assert_eq!(unwind_stage.get_str("$unwind").unwrap(), "$items");
    }

    #[test]
    fn test_build_bucket_pipeline() {
        use crate::domain::models::BucketSpec;
        let query = ParsedQuery {
            collection: "products".into(),
            operation: QueryOperation::Aggregate,
            bucket: Some(BucketSpec {
                group_by: "price".into(),
                boundaries: vec![
                    serde_json::json!(0),
                    serde_json::json!(50),
                    serde_json::json!(100),
                    serde_json::json!(500),
                ],
                default_bucket: Some("Other".into()),
            }),
            ..Default::default()
        };
        let filter = build_filter(&query, None).unwrap();
        let pipeline = build_aggregate_pipeline(&query, &filter).unwrap();
        assert!(pipeline.iter().any(|s| s.contains_key("$bucket")));
    }

    #[test]
    fn test_build_full_pipeline() {
        use crate::domain::models::{GroupSpec, Accumulator, LookupSpec, SortSpec};
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            filters: vec![Filter {
                field: "status".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("completed"),
            }],
            lookup: Some(LookupSpec {
                from: "customers".into(),
                local_field: "customer_id".into(),
                foreign_field: "_id".into(),
                as_field: "customer".into(),
            }),
            unwind: Some("items".into()),
            group_by: Some(GroupSpec {
                field: "customer_id".into(),
                accumulators: vec![Accumulator {
                    output_field: "count".into(),
                    operator: "$sum".into(),
                    input_field: "1".into(),
                }],
            }),
            sort: Some(SortSpec { field: "count".into(), order: -1 }),
            limit: 10,
            ..Default::default()
        };
        let filter = build_filter(&query, None).unwrap();
        let pipeline = build_aggregate_pipeline(&query, &filter).unwrap();
        // Pipeline order: $match, $lookup, $unwind, $group, $sort, $limit
        let stage_keys: Vec<String> = pipeline.iter()
            .flat_map(|d| d.keys().map(|k| k.to_string()))
            .collect();
        assert!(stage_keys.contains(&"$match".to_string()));
        assert!(stage_keys.contains(&"$lookup".to_string()));
        assert!(stage_keys.contains(&"$unwind".to_string()));
        assert!(stage_keys.contains(&"$group".to_string()));
        assert!(stage_keys.contains(&"$sort".to_string()));
        assert!(stage_keys.contains(&"$limit".to_string()));
    }

    #[test]
    fn test_invalid_accumulator_rejected() {
        let result = build_accumulator_expr("$evil", "amount");
        assert!(result.is_err());
    }

    // ── Raw passthrough ────────────────────────────────────────

    #[test]
    fn test_raw_filter_takes_priority() {
        let query = ParsedQuery {
            collection: "orders".into(),
            filters: vec![Filter {
                field: "status".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("active"),
            }],
            raw_filter: Some(serde_json::json!({
                "$or": [{"status": "pending"}, {"status": "shipped"}]
            })),
            ..Default::default()
        };
        let doc = build_filter(&query, None).unwrap();
        // raw_filter should win — should have $or, not status: "active"
        assert!(doc.contains_key("$or"));
        assert!(!doc.contains_key("status"));
    }

    #[test]
    fn test_structured_fallback_when_no_raw() {
        let query = ParsedQuery {
            collection: "orders".into(),
            filters: vec![Filter {
                field: "status".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("active"),
            }],
            ..Default::default()
        };
        let doc = build_filter(&query, None).unwrap();
        assert_eq!(doc.get_str("status").unwrap(), "active");
    }

    // ── Nested object dot notation (not $elemMatch) ──────────

    fn make_schema(fields: &[(&str, &str)]) -> CollectionSchema {
        CollectionSchema {
            name: "test".into(),
            description: String::new(),
            fields: fields.iter().map(|(name, ft)| crate::domain::models::FieldDef {
                name: name.to_string(),
                field_type: ft.to_string(),
                description: None,
            }).collect(),
            indexes: vec![],
            source: "test".into(),
        }
    }

    #[test]
    fn test_nested_object_uses_dot_notation_with_schema() {
        // salary is a nested object (number fields), NOT an array
        let schema = make_schema(&[
            ("salary.bonus", "number"),
            ("salary.currency", "string"),
        ]);
        let filters = vec![
            Filter {
                field: "salary.bonus".into(),
                operator: FilterOperator::Gt,
                value: serde_json::json!(0),
            },
            Filter {
                field: "salary.currency".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("USD"),
            },
        ];
        let doc = build_and_filter(&filters, Some(&schema)).unwrap();
        // No $elemMatch — salary is not an array
        assert!(!doc.contains_key("salary"), "Should not have bare 'salary' key");
        assert!(doc.contains_key("salary.bonus"));
        assert!(doc.contains_key("salary.currency"));
    }

    #[test]
    fn test_array_field_uses_elemmatch_with_schema() {
        // transactions IS an array — $elemMatch should be used
        let schema = make_schema(&[
            ("transactions", "array"),
            ("transactions.symbol", "string"),
            ("transactions.amount", "number"),
        ]);
        let filters = vec![
            Filter {
                field: "transactions.symbol".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("AAPL"),
            },
            Filter {
                field: "transactions.amount".into(),
                operator: FilterOperator::Gt,
                value: serde_json::json!(5000),
            },
        ];
        let doc = build_and_filter(&filters, Some(&schema)).unwrap();
        // Should use $elemMatch since transactions is an array
        assert!(doc.contains_key("transactions"), "Should have 'transactions' with $elemMatch");
        let elem = doc.get_document("transactions").unwrap();
        assert!(elem.contains_key("$elemMatch"), "Should use $elemMatch for array field");
        let inner = elem.get_document("$elemMatch").unwrap();
        assert_eq!(inner.get_str("symbol").unwrap(), "AAPL");
        assert!(inner.get_document("amount").unwrap().contains_key("$gt"));
    }

    #[test]
    fn test_no_schema_uses_dot_notation_fallback() {
        // Without schema, should fall back to dot notation (safe default)
        let filters = vec![
            Filter {
                field: "salary.bonus".into(),
                operator: FilterOperator::Gt,
                value: serde_json::json!(0),
            },
            Filter {
                field: "salary.currency".into(),
                operator: FilterOperator::Eq,
                value: serde_json::json!("USD"),
            },
        ];
        let doc = build_and_filter(&filters, None).unwrap();
        assert!(!doc.contains_key("salary"));
        assert!(doc.contains_key("salary.bonus"));
        assert!(doc.contains_key("salary.currency"));
    }

    #[test]
    fn test_single_dotted_field_still_works() {
        let filters = vec![Filter {
            field: "address.city".into(),
            operator: FilterOperator::Eq,
            value: serde_json::json!("NYC"),
        }];
        let doc = build_and_filter(&filters, None).unwrap();
        assert_eq!(doc.get_str("address.city").unwrap(), "NYC");
    }

    #[test]
    fn test_dotted_same_field_range_merging() {
        // salary.base > 50000 AND salary.base < 100000
        let filters = vec![
            Filter {
                field: "salary.base".into(),
                operator: FilterOperator::Gt,
                value: serde_json::json!(50000),
            },
            Filter {
                field: "salary.base".into(),
                operator: FilterOperator::Lt,
                value: serde_json::json!(100000),
            },
        ];
        let doc = build_and_filter(&filters, None).unwrap();
        let cond = doc.get_document("salary.base").unwrap();
        assert_eq!(cond.get_i64("$gt").unwrap(), 50000);
        assert_eq!(cond.get_i64("$lt").unwrap(), 100000);
    }

    #[test]
    fn test_raw_pipeline_shell_command() {
        let query = ParsedQuery {
            collection: "orders".into(),
            operation: QueryOperation::Aggregate,
            raw_pipeline: Some(vec![
                serde_json::json!({"$match": {"status": "active"}}),
                serde_json::json!({"$group": {"_id": "$category", "count": {"$sum": 1}}}),
            ]),
            ..Default::default()
        };
        let cmd = build_mongo_shell_command(&query);
        assert!(cmd.contains("db.orders.aggregate"));
        assert!(cmd.contains("$match"));
        assert!(cmd.contains("$group"));
    }
}
