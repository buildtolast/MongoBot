use mongodb::{
    Client, Database, Collection,
    IndexModel,
    bson::{Bson, Document, doc},
    options::{ClientOptions, CountOptions, FindOptions, IndexOptions},
};
use std::time::Duration;
use crate::domain::models::{CollectionSchema, FieldDef, IndexInfo, IndexKey};
use crate::error::AppError;

/// Maximum number of feedback records to scan for cosine similarity.
/// Chosen as a balance between recall and per-request memory/latency.
/// Above this threshold, switch to a vector database (Atlas Vector Search, FAISS).
const MAX_FEEDBACK_SCAN: i64 = 10_000;

/// MongoDB connection pool wrapper.
#[derive(Clone)]
pub struct MongoPool {
    client: Client,
    db_name: String,
}

impl MongoPool {
    /// Create a MongoPool for unit tests — parses the URI but does NOT ping.
    /// Only use in tests that do not call any MongoDB methods.
    #[cfg(test)]
    pub async fn new_for_tests() -> Self {
        let opts = ClientOptions::parse("mongodb://localhost:27017").await
            .expect("test: failed to parse mongo URI");
        let client = Client::with_options(opts)
            .expect("test: failed to create test mongo client");
        Self { client, db_name: "test".to_string() }
    }

    /// Connect to MongoDB, verify with a ping command.
    pub async fn connect(uri: &str, db_name: &str) -> anyhow::Result<Self> {
        let mut opts = ClientOptions::parse(uri).await?;

        // Pool sizing
        opts.min_pool_size = Some(2);
        opts.max_pool_size = Some(20);
        opts.connect_timeout = Some(Duration::from_secs(10));
        opts.server_selection_timeout = Some(Duration::from_secs(5));

        let client = Client::with_options(opts)?;

        // Verify connectivity
        client
            .database("admin")
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|e| anyhow::anyhow!("MongoDB ping failed: {e}"))?;

        tracing::info!(db = db_name, "Connected to MongoDB");

        Ok(Self {
            client,
            db_name: db_name.to_string(),
        })
    }

    /// Get the configured database handle.
    pub fn db(&self) -> Database {
        self.client.database(&self.db_name)
    }

    /// Get a typed collection handle.
    pub fn collection(&self, name: &str) -> Collection<Document> {
        self.db().collection(name)
    }

    /// Execute a find query with optional filter, sort, limit, and projection.
    pub async fn find(
        &self,
        collection_name: &str,
        filter: Option<Document>,
        sort: Option<Document>,
        limit: Option<i64>,
        projection: Option<Document>,
    ) -> Result<mongodb::Cursor<Document>, AppError> {
        let coll = self.collection(collection_name);
        let mut find_opts = FindOptions::builder()
            .max_time(Duration::from_secs(30))
            .build();

        if let Some(s) = sort {
            find_opts.sort = Some(s);
        }
        if let Some(l) = limit {
            find_opts.limit = Some(l);
        }
        if let Some(p) = projection {
            find_opts.projection = Some(p);
        }

        coll.find(filter.unwrap_or_default())
            .with_options(find_opts)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Collect documents into a Vec.
    pub async fn find_documents(
        &self,
        collection_name: &str,
        filter: Option<Document>,
        sort: Option<Document>,
        limit: Option<i64>,
        projection: Option<Document>,
    ) -> Result<Vec<Document>, AppError> {
        let mut cursor = self.find(collection_name, filter, sort, limit, projection).await?;
        let mut results = Vec::new();
        use futures::TryStreamExt;
        while let Some(doc) = cursor.try_next().await? {
            results.push(doc);
        }

        Ok(results)
    }

    /// Count documents matching a filter.
    pub async fn count_documents(
        &self,
        collection_name: &str,
        filter: Option<Document>,
    ) -> Result<u64, AppError> {
        let coll = self.collection(collection_name);
        let opts = CountOptions::builder()
            .max_time(Duration::from_secs(30))
            .build();
        let count = coll
            .count_documents(filter.unwrap_or_default())
            .with_options(opts)
            .await?;
        Ok(count)
    }

    /// List all collection names in the database.
    pub async fn list_collections(&self) -> Result<Vec<String>, AppError> {
        let names = self.db().list_collection_names().await?;
        Ok(names)
    }

    /// Introspect a collection: sample documents to discover fields and types,
    /// and list all indexes.
    pub async fn introspect_collection(&self, name: &str) -> Result<CollectionSchema, AppError> {
        let coll = self.collection(name);

        // Sample up to 50 docs to discover fields and types
        let sample_opts = FindOptions::builder()
            .limit(Some(50))
            .max_time(Duration::from_secs(10))
            .build();
        let mut cursor = coll.find(doc! {}).with_options(sample_opts).await?;

        let mut field_types: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        use futures::TryStreamExt;
        while let Some(doc) = cursor.try_next().await? {
            collect_field_types(&doc, "", &mut field_types);
        }

        // Post-process: collapse paths where many id-like keys appear at the
        // same prefix. This catches the "map of <hash> -> sub-doc" pattern
        // that accumulates across sampled documents — each individual doc
        // may only have 1-2 hash keys, but across 50 sampled docs the
        // distinct count grows large.
        let field_types = collapse_dynamic_key_paths(field_types);

        let fields: Vec<FieldDef> = field_types
            .into_iter()
            .map(|(name, field_type)| FieldDef {
                name,
                field_type,
                description: None,
            })
            .collect();

        // Get indexes
        let indexes = self.get_indexes(name).await?;

        let doc_count = coll.estimated_document_count().await.unwrap_or(0);

        Ok(CollectionSchema {
            name: name.to_string(),
            description: format!("{} documents", doc_count),
            fields,
            indexes,
            source: "introspected".to_string(),
        })
    }

    /// Get all indexes for a collection.
    pub async fn get_indexes(&self, collection_name: &str) -> Result<Vec<IndexInfo>, AppError> {
        let coll = self.collection(collection_name);
        let mut cursor = coll.list_indexes().await?;

        let mut indexes = Vec::new();
        use futures::TryStreamExt;
        while let Some(index_model) = cursor.try_next().await? {
            let name = index_model
                .options
                .as_ref()
                .and_then(|o| o.name.clone())
                .unwrap_or_default();
            let unique = index_model
                .options
                .as_ref()
                .and_then(|o| o.unique)
                .unwrap_or(false);

            let mut keys = Vec::new();
            for (field, bson) in &index_model.keys {
                let direction = match bson {
                    Bson::Int32(n) => *n,
                    Bson::Int64(n) => *n as i32,
                    Bson::Double(n) => *n as i32,
                    _ => 1,
                };
                keys.push(IndexKey {
                    field: field.clone(),
                    direction,
                });
            }

            indexes.push(IndexInfo {
                name,
                keys,
                unique,
            });
        }

        Ok(indexes)
    }

    /// Create a single-field index on a collection if it doesn't already exist.
    /// MongoDB's createIndex is idempotent — calling it on an existing index is a no-op.
    /// Returns `true` if a new index was created, `false` if it already existed.
    pub async fn ensure_index(
        &self,
        collection_name: &str,
        field: &str,
        direction: i32,
    ) -> Result<bool, AppError> {
        let coll = self.collection(collection_name);

        // Check if an index already covers this field as its leading key.
        let mut cursor = coll.list_indexes().await
            .map_err(|e| AppError::Internal(format!("list_indexes failed: {e}")))?;
        use futures::TryStreamExt;
        while let Some(idx) = cursor.try_next().await
            .map_err(|e| AppError::Internal(format!("index cursor error: {e}")))?
        {
            if idx.keys.keys().next().map(|k| k.as_str()) == Some(field) {
                return Ok(false); // already covered
            }
        }

        let key = doc! { field: direction };
        let opts = IndexOptions::builder()
            .background(true)
            .build();
        let model = IndexModel::builder()
            .keys(key)
            .options(opts)
            .build();
        coll.create_index(model).await
            .map_err(|e| AppError::Internal(format!("create_index failed on {collection_name}.{field}: {e}")))?;
        Ok(true)
    }

    /// Insert a single document.
    pub async fn insert_document(
        &self,
        collection_name: &str,
        document: Document,
    ) -> Result<String, AppError> {
        let coll = self.collection(collection_name);
        let result = coll.insert_one(document).await?;
        Ok(result.inserted_id.to_string())
    }

    /// Alias for insert_document to support standard MongoDB naming.
    pub async fn insert_one(&self, collection_name: &str, document: Document) -> Result<(), AppError> {
        self.insert_document(collection_name, document).await.map(|_| ())
    }

    /// Find documents matching a filter.
    /// Execute an aggregation pipeline.
    pub async fn aggregate(
        &self,
        collection_name: &str,
        pipeline: Vec<Document>,
    ) -> Result<mongodb::Cursor<Document>, AppError> {
        let coll = self.collection(collection_name);
        coll.aggregate(pipeline).await.map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Update documents matching a filter.
    pub async fn update_documents(
        &self,
        collection_name: &str,
        filter: Document,
        update: Document,
        many: bool,
    ) -> Result<(u64, u64), AppError> {
        let coll = self.collection(collection_name);
        let update_doc = doc! { "$set": update };
        if many {
            let result = coll.update_many(filter, update_doc).await?;
            Ok((result.matched_count, result.modified_count))
        } else {
            let result = coll.update_one(filter, update_doc).await?;
            Ok((result.matched_count, result.modified_count))
        }
    }

    /// Delete documents matching a filter.
    pub async fn delete_documents(
        &self,
        collection_name: &str,
        filter: Document,
        many: bool,
    ) -> Result<u64, AppError> {
        let coll = self.collection(collection_name);
        if many {
            let result = coll.delete_many(filter).await?;
            Ok(result.deleted_count)
        } else {
            let result = coll.delete_one(filter).await?;
            Ok(result.deleted_count)
        }
    }

    /// Get distinct values for a field.
    pub async fn distinct_values(
        &self,
        collection_name: &str,
        field: &str,
        filter: Option<Document>,
    ) -> Result<Vec<Bson>, AppError> {
        use mongodb::options::DistinctOptions;
        let coll = self.collection(collection_name);
        let opts = DistinctOptions::builder()
            .max_time(Duration::from_secs(30))
            .build();
        let values = coll
            .distinct(field, filter.unwrap_or_default())
            .with_options(opts)
            .await?;
        Ok(values)
    }

    /// Find top-K similar feedback records using cosine similarity.
    /// Returns a list of feedback documents.
    pub async fn find_similar_feedback(
        &self,
        query_embedding: &[f32],
        k: usize,
    ) -> Result<Vec<Document>, AppError> {
        tracing::debug!(k, "find_similar_feedback start");
        let collection = "query_feedback";
        // Only retrieve liked or corrected examples
        let filter = doc! { "feedback": { "$in": ["liked", "corrected"] } };

        let sort = doc! { "timestamp": -1 };
        let mut cursor = self
            .find(collection, Some(filter), Some(sort), Some(MAX_FEEDBACK_SCAN), None)
            .await?;
        let mut results = Vec::new();
        let mut scanned: usize = 0;
        use futures::TryStreamExt;
        // Intentional silent-degrade: a cursor error during retrieval stops the scan
        // but still returns whatever partial results were collected.  Propagating here
        // would turn a transient network hiccup into a hard failure for the caller,
        // which is worse than serving zero few-shot examples.
        loop {
            match cursor.try_next().await {
                Err(e) => {
                    tracing::warn!("find_similar_feedback cursor error (returning partial results): {e}");
                    break;
                }
                Ok(None) => break,
                Ok(Some(doc)) => {
                    scanned += 1;
                    if let Some(emb_bson) = doc.get("embedding") {
                        if let Some(emb_array) = emb_bson.as_array() {
                            let emb: Vec<f32> = emb_array.iter()
                                .filter_map(|b| b.as_f64().map(|f| f as f32))
                                .collect();
                            if emb.len() == query_embedding.len() {
                                let sim = cosine_similarity(query_embedding, &emb);
                                results.push((sim, doc));
                            }
                        }
                    }
                }
            }
        }
        if scanned >= MAX_FEEDBACK_SCAN as usize {
            tracing::warn!(
                "find_similar_feedback hit MAX_FEEDBACK_SCAN={}; consider migrating to a vector database",
                MAX_FEEDBACK_SCAN
            );
        }

        // Sort by similarity descending
        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let returned = results.len().min(k);
        tracing::debug!(scanned, returned, "find_similar_feedback complete");
        Ok(results.into_iter().take(k).map(|(_, doc)| doc).collect())
    }
}

pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb + 1e-10)
}

/// Recursively collect field names and BSON type names from a document.
fn collect_field_types(
    doc: &Document,
    prefix: &str,
    types: &mut std::collections::BTreeMap<String, String>,
) {
    for (key, value) in doc {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        let type_name = bson_type_name(value);

        match value {
            Bson::Document(inner) => {
                if inner.contains_key("$oid")
                    || inner.contains_key("$date")
                    || inner.contains_key("$numberLong")
                {
                    types.insert(full_key, type_name);
                } else if looks_like_dynamic_key_map(inner) {
                    // The document is a map keyed by dynamic identifiers
                    // (hash strings, ObjectIds, UUIDs). Recording every key
                    // would explode the schema with hundreds of meaningless
                    // entries. Record the parent as an object and descend
                    // into the first value with a `<key>` placeholder so
                    // the value's structure is still discoverable.
                    types.insert(full_key.clone(), "object".to_string());
                    if let Some((_, first_value)) = inner.iter().next() {
                        let placeholder = format!("{full_key}.<key>");
                        match first_value {
                            Bson::Document(sub) => {
                                collect_field_types(sub, &placeholder, types);
                            }
                            other => {
                                types.insert(placeholder, bson_type_name(other));
                            }
                        }
                    }
                } else {
                    collect_field_types(inner, &full_key, types);
                }
            }
            Bson::Array(arr) => {
                // Record the array field itself
                types.insert(full_key.clone(), "array".to_string());
                // Dive into the first element to discover inner fields
                // This enables dot-notation queries like "transactions.symbol"
                if let Some(Bson::Document(inner_doc)) = arr.first() {
                        collect_field_types(inner_doc, &full_key, types);
                    }
            }
            _ => {
                types.entry(full_key).or_insert(type_name);
            }
        }
    }
}

/// Post-process the accumulated field-type map: detect prefixes where
/// many distinct id-like segments appear and collapse all matching paths
/// to use a `<key>` placeholder.
///
/// Example input keys (from sampling 50 customer docs):
///   tier_and_details.069f...062.active   -> boolean
///   tier_and_details.069f...062.tier     -> string
///   tier_and_details.07d5...8a2.active   -> boolean
///   tier_and_details.07d5...8a2.tier     -> string
///   ... (many more hash-keyed entries)
///
/// Output:
///   tier_and_details.<key>.active        -> boolean
///   tier_and_details.<key>.tier          -> string
///
/// A prefix is treated as dynamic if 3+ distinct id-like segments appear
/// directly under it across the sampled paths.
fn collapse_dynamic_key_paths(
    types: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    use std::collections::{BTreeMap, HashMap, HashSet};

    // First pass: for each prefix, count distinct id-like next-segments.
    let mut prefix_id_counts: HashMap<String, HashSet<String>> = HashMap::new();
    for path in types.keys() {
        let segments: Vec<&str> = path.split('.').collect();
        for (i, seg) in segments.iter().enumerate() {
            if is_id_like_key(seg) {
                let prefix = segments[..i].join(".");
                prefix_id_counts
                    .entry(prefix)
                    .or_default()
                    .insert((*seg).to_string());
            }
        }
    }

    // Prefixes with 3+ distinct id-like next-segments are treated as dynamic maps.
    let dynamic_prefixes: HashSet<String> = prefix_id_counts
        .into_iter()
        .filter(|(_, ids)| ids.len() >= 3)
        .map(|(p, _)| p)
        .collect();

    // Second pass: rewrite paths, replacing id-like segments at dynamic
    // prefixes with `<key>`. Use a BTreeMap so duplicate rewritten keys
    // collapse naturally.
    let mut result: BTreeMap<String, String> = BTreeMap::new();
    for (path, ty) in types {
        let segments: Vec<&str> = path.split('.').collect();
        let mut rewritten: Vec<String> = Vec::with_capacity(segments.len());
        for (i, seg) in segments.iter().enumerate() {
            if is_id_like_key(seg) {
                let prefix = segments[..i].join(".");
                if dynamic_prefixes.contains(&prefix) {
                    rewritten.push("<key>".to_string());
                    continue;
                }
            }
            rewritten.push((*seg).to_string());
        }
        // Use entry().or_insert() so the first-seen type wins on collisions
        // (avoids flapping when sampled docs disagree on inner type).
        result.entry(rewritten.join(".")).or_insert(ty);
    }
    result
}

/// Heuristic: detect whether a document's keys look like dynamic identifiers
/// (hashes, UUIDs, ObjectIds) used as map keys, rather than static field names.
/// Such "map of <id> -> X" shapes should NOT have every key promoted to a
/// schema field — that would generate hundreds of bogus entries.
///
/// Returns true when the doc has 5+ keys AND at least 80% of keys match a
/// long-hex pattern (16+ hex chars) or look UUID-shaped.
///
/// Used by collect_field_types as an early-exit optimisation. The post-pass
/// in collapse_dynamic_key_paths catches the cross-document accumulation
/// case that this per-doc heuristic misses.
fn looks_like_dynamic_key_map(doc: &Document) -> bool {
    let total = doc.len();
    if total < 5 {
        return false;
    }
    let id_like = doc.keys().filter(|k| is_id_like_key(k)).count();
    id_like * 5 >= total * 4
}

fn is_id_like_key(key: &str) -> bool {
    let len = key.len();
    if len < 16 {
        return false;
    }
    // Long pure-hex key (ObjectId is 24, MD5 is 32, SHA1 is 40, etc.)
    if key.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    // UUID: 8-4-4-4-12 hex with dashes, total length 36
    if len == 36 {
        let bytes = key.as_bytes();
        let dash_positions = [8usize, 13, 18, 23];
        let dashes_ok = dash_positions.iter().all(|&i| bytes[i] == b'-');
        let hex_ok = key
            .chars()
            .enumerate()
            .all(|(i, c)| dash_positions.contains(&i) || c.is_ascii_hexdigit());
        if dashes_ok && hex_ok {
            return true;
        }
    }
    false
}

/// Map a BSON value to a human-readable type name.
fn bson_type_name(value: &Bson) -> String {
    match value {
        Bson::ObjectId(_) => "objectId".to_string(),
        Bson::String(_) => "string".to_string(),
        Bson::Int32(_) | Bson::Int64(_) => "number".to_string(),
        Bson::Double(_) => "number".to_string(),
        Bson::Boolean(_) => "boolean".to_string(),
        Bson::DateTime(_) => "date".to_string(),
        Bson::Array(_) => "array".to_string(),
        Bson::Null => "null".to_string(),
        Bson::Document(d) => {
            if d.contains_key("$oid") { "objectId".to_string() }
            else if d.contains_key("$date") { "date".to_string() }
            else { "object".to_string() }
        }
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collapse_dynamic_key_paths, cosine_similarity, is_id_like_key,
        looks_like_dynamic_key_map, MAX_FEEDBACK_SCAN,
    };
    use mongodb::bson::doc;

    #[test]
    fn test_collapse_dynamic_key_paths_collapses_hash_map() {
        // Simulate accumulated paths from sampling 5 customer docs where
        // each customer's tier_and_details has 1 distinct hash key.
        let mut types = std::collections::BTreeMap::new();
        for hex in [
            "069f84c793a14fa891f716cf09314062",
            "07d516cfd7fc4ec6acf175bb78cb98a2",
            "0db0e5c9a332493ea75f70d028ed36e8",
            "0df078f33aa74a2e9696e0520c1a828a",
            "0f415868bc0b4acfa9160adf1b840fa9",
        ] {
            types.insert(format!("tier_and_details.{hex}.active"), "boolean".into());
            types.insert(format!("tier_and_details.{hex}.tier"), "string".into());
        }
        types.insert("name".into(), "string".into());
        types.insert("email".into(), "string".into());

        let result = collapse_dynamic_key_paths(types);

        // 5 hashes × 2 sub-fields = 10 entries collapse to 2 placeholder entries
        assert!(result.contains_key("tier_and_details.<key>.active"));
        assert!(result.contains_key("tier_and_details.<key>.tier"));
        assert!(result.contains_key("name"));
        assert!(result.contains_key("email"));
        // Shouldn't contain any of the literal hash keys
        assert!(!result.contains_key("tier_and_details.069f84c793a14fa891f716cf09314062.active"));
        assert_eq!(result.len(), 4); // 2 placeholder + name + email
    }

    #[test]
    fn test_collapse_does_not_collapse_when_too_few_hashes() {
        // 2 distinct hashes is below the threshold of 3
        let mut types = std::collections::BTreeMap::new();
        types.insert("map.069f84c793a14fa891f716cf09314062.x".into(), "string".into());
        types.insert("map.07d516cfd7fc4ec6acf175bb78cb98a2.x".into(), "string".into());
        let result = collapse_dynamic_key_paths(types);
        // Both literal paths preserved
        assert_eq!(result.len(), 2);
        assert!(result.keys().any(|k| k.contains("069f")));
    }

    #[test]
    fn test_is_id_like_key_recognizes_hashes_and_uuids() {
        assert!(is_id_like_key("069f84c793a14fa891f716cf09314062"));     // 32-char hex (MD5)
        assert!(is_id_like_key("507f1f77bcf86cd799439011"));             // 24-char hex (ObjectId)
        assert!(is_id_like_key("550e8400-e29b-41d4-a716-446655440000")); // UUID
        assert!(!is_id_like_key("user_id"));                              // short
        assert!(!is_id_like_key("description"));                          // letters
        assert!(!is_id_like_key("notlongenough123"));                     // 16 chars but valid
        assert!(is_id_like_key("0123456789abcdef"));                      // exactly 16 hex
    }

    #[test]
    fn test_looks_like_dynamic_key_map() {
        // 5+ hash-keyed entries → true
        let mut d = doc! {};
        for hex in [
            "069f84c793a14fa891f716cf09314062",
            "07d516cfd7fc4ec6acf175bb78cb98a2",
            "0db0e5c9a332493ea75f70d028ed36e8",
            "0df078f33aa74a2e9696e0520c1a828a",
            "0f415868bc0b4acfa9160adf1b840fa9",
        ] {
            d.insert(hex, "x");
        }
        assert!(looks_like_dynamic_key_map(&d));

        // Normal docs with named fields → false
        let normal = doc! {"name": "Alice", "age": 30, "email": "a@b.c"};
        assert!(!looks_like_dynamic_key_map(&normal));

        // Mixed but mostly named → false
        let mixed = doc! {
            "name": "Alice",
            "069f84c793a14fa891f716cf09314062": "x",
            "age": 30,
        };
        assert!(!looks_like_dynamic_key_map(&mixed));
    }


    #[test]
    fn test_cosine_similarity() {
        // Identical vectors → similarity of 1.0
        let v = [1.0_f32, 2.0, 3.0];
        let actual = cosine_similarity(&v, &v);
        assert!((actual - 1.0).abs() < 1e-5, "identical vectors: expected ~1.0, got {actual}");

        // Orthogonal vectors → similarity of 0.0
        let a = [1.0_f32, 0.0];
        let b = [0.0_f32, 1.0];
        let actual = cosine_similarity(&a, &b);
        assert!((actual - 0.0).abs() < 1e-5, "orthogonal vectors: expected ~0.0, got {actual}");

        // Opposite vectors → similarity of -1.0
        let a = [1.0_f32, 0.0];
        let b = [-1.0_f32, 0.0];
        let actual = cosine_similarity(&a, &b);
        assert!((actual - (-1.0)).abs() < 1e-5, "opposite vectors: expected ~-1.0, got {actual}");

        // Empty vectors → denominator collapses to 1e-10; result is 0.0 / 1e-10 = 0.0
        let actual = cosine_similarity(&[], &[]);
        assert!(actual.abs() < 1e-5, "empty vectors: expected ~0.0, got {actual}");
    }

    /// Tripwire: guards against accidentally setting MAX_FEEDBACK_SCAN too low or absurdly high.
    /// If you change the constant, review the comment in mongo.rs about Atlas Vector Search / FAISS.
    #[test]
    fn test_find_similar_feedback_respects_max_scan_constant() {
        assert!(
            MAX_FEEDBACK_SCAN >= 1_000 && MAX_FEEDBACK_SCAN <= 100_000,
            "MAX_FEEDBACK_SCAN={MAX_FEEDBACK_SCAN} is outside the safe range [1000, 100000]; \
             see the constant's doc comment before changing it"
        );
    }
}

/// Integration tests that require a real MongoDB at localhost:27017.
/// Tests skip gracefully if MongoDB is not reachable.
#[cfg(test)]
mod db_tests {
    use super::*;

    const TEST_DB: &str = "test_phase3_coverage";

    async fn setup(coll: &str) -> Option<MongoPool> {
        let pool = MongoPool::connect("mongodb://localhost:27017", TEST_DB)
            .await
            .ok()?;
        let _ = pool.db().collection::<Document>(coll).drop().await;
        Some(pool)
    }

    async fn cleanup(pool: &MongoPool, coll: &str) {
        let _ = pool.db().collection::<Document>(coll).drop().await;
    }

    #[tokio::test]
    async fn connect_and_list_collections() {
        let pool = match setup("t_list_coll").await { Some(p) => p, None => return };
        pool.insert_document("t_list_coll", doc! { "x": 1 }).await.unwrap();
        let colls = pool.list_collections().await.unwrap();
        assert!(colls.contains(&"t_list_coll".to_string()));
        cleanup(&pool, "t_list_coll").await;
    }

    #[tokio::test]
    async fn insert_and_find_document() {
        let pool = match setup("t_insert_find").await { Some(p) => p, None => return };
        let id = pool.insert_document("t_insert_find", doc! { "name": "Alice", "age": 30 }).await.unwrap();
        assert!(!id.is_empty());
        let docs = pool.find_documents("t_insert_find", None, None, None, None).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].get_str("name").unwrap(), "Alice");
        cleanup(&pool, "t_insert_find").await;
    }

    #[tokio::test]
    async fn count_documents_empty() {
        let pool = match setup("t_count_empty").await { Some(p) => p, None => return };
        let count = pool.count_documents("t_count_empty", None).await.unwrap();
        assert_eq!(count, 0);
        cleanup(&pool, "t_count_empty").await;
    }

    #[tokio::test]
    async fn count_documents_with_filter() {
        let pool = match setup("t_count_filter").await { Some(p) => p, None => return };
        pool.insert_document("t_count_filter", doc! { "status": "active" }).await.unwrap();
        pool.insert_document("t_count_filter", doc! { "status": "inactive" }).await.unwrap();
        pool.insert_document("t_count_filter", doc! { "status": "active" }).await.unwrap();
        let count = pool.count_documents("t_count_filter", Some(doc! { "status": "active" })).await.unwrap();
        assert_eq!(count, 2);
        cleanup(&pool, "t_count_filter").await;
    }

    #[tokio::test]
    async fn distinct_values_for_field() {
        let pool = match setup("t_distinct").await { Some(p) => p, None => return };
        pool.insert_document("t_distinct", doc! { "genre": "Action" }).await.unwrap();
        pool.insert_document("t_distinct", doc! { "genre": "Comedy" }).await.unwrap();
        pool.insert_document("t_distinct", doc! { "genre": "Action" }).await.unwrap();
        let vals = pool.distinct_values("t_distinct", "genre", None).await.unwrap();
        assert_eq!(vals.len(), 2);
        cleanup(&pool, "t_distinct").await;
    }

    #[tokio::test]
    async fn update_one_modifies_document() {
        let pool = match setup("t_update_one").await { Some(p) => p, None => return };
        pool.insert_document("t_update_one", doc! { "name": "Bob", "score": 10_i32 }).await.unwrap();
        let (matched, modified) = pool.update_documents(
            "t_update_one", doc! { "name": "Bob" }, doc! { "score": 20_i32 }, false
        ).await.unwrap();
        assert_eq!(matched, 1);
        assert_eq!(modified, 1);
        let docs = pool.find_documents("t_update_one", Some(doc! { "name": "Bob" }), None, None, None).await.unwrap();
        assert_eq!(docs[0].get_i32("score").unwrap(), 20);
        cleanup(&pool, "t_update_one").await;
    }

    #[tokio::test]
    async fn update_many_modifies_all_matching() {
        let pool = match setup("t_update_many").await { Some(p) => p, None => return };
        for _ in 0..3 {
            pool.insert_document("t_update_many", doc! { "group": "a", "done": false }).await.unwrap();
        }
        let (matched, modified) = pool.update_documents(
            "t_update_many", doc! { "group": "a" }, doc! { "done": true }, true
        ).await.unwrap();
        assert_eq!(matched, 3);
        assert_eq!(modified, 3);
        cleanup(&pool, "t_update_many").await;
    }

    #[tokio::test]
    async fn delete_one_removes_single_document() {
        let pool = match setup("t_delete_one").await { Some(p) => p, None => return };
        pool.insert_document("t_delete_one", doc! { "keep": false }).await.unwrap();
        pool.insert_document("t_delete_one", doc! { "keep": true }).await.unwrap();
        let deleted = pool.delete_documents("t_delete_one", doc! { "keep": false }, false).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(pool.count_documents("t_delete_one", None).await.unwrap(), 1);
        cleanup(&pool, "t_delete_one").await;
    }

    #[tokio::test]
    async fn delete_many_removes_all_matching() {
        let pool = match setup("t_delete_many").await { Some(p) => p, None => return };
        for _ in 0..4 {
            pool.insert_document("t_delete_many", doc! { "batch": "x" }).await.unwrap();
        }
        let deleted = pool.delete_documents("t_delete_many", doc! { "batch": "x" }, true).await.unwrap();
        assert_eq!(deleted, 4);
        cleanup(&pool, "t_delete_many").await;
    }

    #[tokio::test]
    async fn find_with_sort_and_limit() {
        let pool = match setup("t_find_sort").await { Some(p) => p, None => return };
        for i in [3_i32, 1, 4, 1, 5] {
            pool.insert_document("t_find_sort", doc! { "val": i }).await.unwrap();
        }
        let docs = pool.find_documents(
            "t_find_sort", None, Some(doc! { "val": 1 }), Some(3), None
        ).await.unwrap();
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0].get_i32("val").unwrap(), 1);
        cleanup(&pool, "t_find_sort").await;
    }

    #[tokio::test]
    async fn get_indexes_includes_id_index() {
        let pool = match setup("t_get_idx").await { Some(p) => p, None => return };
        pool.insert_document("t_get_idx", doc! { "x": 1 }).await.unwrap();
        let indexes = pool.get_indexes("t_get_idx").await.unwrap();
        assert!(indexes.iter().any(|i| i.name == "_id_"));
        cleanup(&pool, "t_get_idx").await;
    }

    #[tokio::test]
    async fn ensure_index_creates_then_detects_existing() {
        let pool = match setup("t_ensure_idx").await { Some(p) => p, None => return };
        pool.insert_document("t_ensure_idx", doc! { "email": "a@b.com" }).await.unwrap();
        let created = pool.ensure_index("t_ensure_idx", "email", 1).await.unwrap();
        assert!(created, "first call should create the index");
        let created2 = pool.ensure_index("t_ensure_idx", "email", 1).await.unwrap();
        assert!(!created2, "second call should detect existing index");
        cleanup(&pool, "t_ensure_idx").await;
    }

    #[tokio::test]
    async fn introspect_collection_discovers_field_types() {
        let pool = match setup("t_introspect").await { Some(p) => p, None => return };
        pool.insert_document("t_introspect", doc! {
            "title": "Test Movie", "year": 2020_i32, "genres": ["Action", "Drama"]
        }).await.unwrap();
        pool.insert_document("t_introspect", doc! {
            "title": "Another Movie", "year": 2021_i32, "rating": 8.5_f64
        }).await.unwrap();
        let schema = pool.introspect_collection("t_introspect").await.unwrap();
        assert_eq!(schema.name, "t_introspect");
        assert_eq!(schema.source, "introspected");
        let field_names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(field_names.contains(&"title"), "should discover title field");
        assert!(field_names.contains(&"year"), "should discover year field");
        assert!(schema.indexes.iter().any(|i| i.name == "_id_"), "should list _id_ index");
        cleanup(&pool, "t_introspect").await;
    }

    #[tokio::test]
    async fn insert_one_alias_works() {
        let pool = match setup("t_insert_one").await { Some(p) => p, None => return };
        pool.insert_one("t_insert_one", doc! { "key": "value" }).await.unwrap();
        assert_eq!(pool.count_documents("t_insert_one", None).await.unwrap(), 1);
        cleanup(&pool, "t_insert_one").await;
    }

    #[tokio::test]
    async fn find_with_projection_returns_only_requested_fields() {
        let pool = match setup("t_projection").await { Some(p) => p, None => return };
        pool.insert_document("t_projection", doc! { "name": "Eve", "secret": "hidden", "age": 25_i32 }).await.unwrap();
        let docs = pool.find_documents(
            "t_projection", None, None, None, Some(doc! { "name": 1 })
        ).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].contains_key("name"));
        assert!(!docs[0].contains_key("secret"), "projected-out field should not appear");
        cleanup(&pool, "t_projection").await;
    }
}
