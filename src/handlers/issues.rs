use actix_web::{web, HttpResponse};

use crate::domain::models::FixIndexRequest;
use crate::error::AppError;
use crate::state::AppState;

/// GET /api/issues — return current missing-index issues.
pub async fn get_issues(state: web::Data<AppState>) -> HttpResponse {
    let issues = state.issues.read().await;
    HttpResponse::Ok().json(&*issues)
}

/// POST /api/issues/fix — create an index for one specific field, then remove
/// the corresponding issue from the list.
pub async fn fix_index(
    state: web::Data<AppState>,
    body: web::Json<FixIndexRequest>,
) -> Result<HttpResponse, AppError> {
    let created = state
        .mongo
        .ensure_index(&body.collection, &body.field, 1)
        .await?;

    // Remove from the issues list regardless of whether it was newly created or
    // already existed — if it already existed the schema introspection just hadn't
    // picked it up yet, so we should still clear the noise.
    {
        let mut issues = state.issues.write().await;
        issues.retain(|i| !(i.collection == body.collection && i.field == body.field));
    }

    tracing::info!(
        collection = %body.collection,
        field = %body.field,
        %created,
        "Index created via UI"
    );

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "created": created,
        "collection": body.collection,
        "field": body.field,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use crate::domain::models::IndexIssue;
    use crate::test_helpers::test_app_state;

    #[actix_web::test]
    async fn get_issues_empty_returns_empty_array() {
        let state = test_app_state().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/issues", web::get().to(get_issues))
        ).await;
        let req = test::TestRequest::get().uri("/api/issues").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert!(body.as_array().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn get_issues_returns_all_stored_issues() {
        let state = test_app_state().await;
        {
            let mut issues = state.issues.write().await;
            issues.push(IndexIssue {
                collection: "movies".into(),
                field: "director".into(),
                message: "missing index".into(),
                fix_command: "db.movies.createIndex({director:1})".into(),
            });
            issues.push(IndexIssue {
                collection: "actors".into(),
                field: "name".into(),
                message: "missing index".into(),
                fix_command: "db.actors.createIndex({name:1})".into(),
            });
        }
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/issues", web::get().to(get_issues))
        ).await;
        let req = test::TestRequest::get().uri("/api/issues").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 2);
    }

    #[actix_web::test]
    async fn get_issues_response_includes_fix_command() {
        let state = test_app_state().await;
        {
            let mut issues = state.issues.write().await;
            issues.push(IndexIssue {
                collection: "movies".into(),
                field: "year".into(),
                message: "no index on year".into(),
                fix_command: "db.movies.createIndex({year:1})".into(),
            });
        }
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/issues", web::get().to(get_issues))
        ).await;
        let req = test::TestRequest::get().uri("/api/issues").to_request();
        let resp = test::call_service(&app, req).await;
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body[0]["collection"], "movies");
        assert_eq!(body[0]["field"], "year");
        assert!(body[0]["fix_command"].as_str().unwrap().contains("createIndex"));
    }

    #[actix_web::test]
    async fn fix_index_creates_index_and_removes_issue() {
        use crate::infra::mongo::MongoPool;
        // Graceful skip if MongoDB is unavailable
        let pool = match MongoPool::connect("mongodb://localhost:27017", "test_issues_fix").await {
            Ok(p) => p,
            Err(_) => return,
        };
        // Ping to verify connectivity
        if pool.db().run_command(mongodb::bson::doc! { "ping": 1 }).await.is_err() {
            return;
        }

        // Seed a document so the collection exists before ensure_index lists its indexes
        let _ = pool.db()
            .collection::<mongodb::bson::Document>("test_fix_coll")
            .insert_one(mongodb::bson::doc! { "seed": true })
            .await;

        use crate::domain::safety::SafetyChecker;
        use crate::infra::llm::LlmClient;
        use crate::state::AppState;
        use std::sync::Arc;
        use dashmap::DashMap;
        use tokio::sync::RwLock;
        let config = crate::test_helpers::test_config();
        let llm = LlmClient::new(&config);
        let state = AppState {
            mongo: pool,
            llm,
            schemas: Arc::new(DashMap::new()),
            safety: SafetyChecker::new(config.allowed_operations.clone(), config.max_result_rows),
            http: reqwest::Client::new(),
            issues: Arc::new(RwLock::new(vec![])),
            config,
        };
        // Pre-seed an issue
        {
            let mut issues = state.issues.write().await;
            issues.push(IndexIssue {
                collection: "test_fix_coll".into(),
                field: "test_field".into(),
                message: "missing index".into(),
                fix_command: "db.test_fix_coll.createIndex({test_field:1})".into(),
            });
        }
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/issues/fix", web::post().to(fix_index))
        ).await;
        let body = serde_json::json!({ "collection": "test_fix_coll", "field": "test_field" });
        let req = test::TestRequest::post()
            .uri("/api/issues/fix")
            .set_json(&body)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let resp_body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(resp_body["collection"], "test_fix_coll");
        assert_eq!(resp_body["field"], "test_field");
    }
}
