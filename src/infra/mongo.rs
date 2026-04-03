use mongodb::{
    Client, Database, Collection,
    bson::{Bson, Document, doc},
    options::{ClientOptions, FindOptions},
};
use std::time::Duration;
use crate::domain::models::{CollectionSchema, FieldDef, IndexInfo, IndexKey};
use crate::error::AppError;

/// MongoDB connection pool wrapper.
#[derive(Clone)]
pub struct MongoPool {
    client: Client,
    db_name: String,
}

impl MongoPool {
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
    pub async fn find_documents(
        &self,
        collection_name: &str,
        filter: Option<Document>,
        sort: Option<Document>,
        limit: Option<i64>,
        projection: Option<Document>,
    ) -> Result<Vec<Document>, AppError> {
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

        let mut cursor = coll
            .find(filter.unwrap_or_default())
            .with_options(find_opts)
            .await?;

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
        let count = coll
            .count_documents(filter.unwrap_or_default())
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
        let coll = self.collection(collection_name);
        let values = coll
            .distinct(field, filter.unwrap_or_default())
            .await?;
        Ok(values)
    }
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

        if let Bson::Document(inner) = value {
            if inner.contains_key("$oid")
                || inner.contains_key("$date")
                || inner.contains_key("$numberLong")
            {
                types.insert(full_key, type_name);
            } else {
                collect_field_types(inner, &full_key, types);
            }
        } else {
            types.entry(full_key).or_insert(type_name);
        }
    }
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
