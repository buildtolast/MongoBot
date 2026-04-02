use mongodb::{
    Client, Database, Collection,
    bson::{Bson, Document, doc},
    options::{ClientOptions, FindOptions},
};
use std::time::Duration;
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
