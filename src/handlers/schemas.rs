use actix_web::{web, HttpResponse};
use serde_json::json;

use crate::state::AppState;

/// List all cached collection schemas.
pub async fn list_schemas(state: web::Data<AppState>) -> HttpResponse {
    let schemas: Vec<_> = state
        .schemas
        .iter()
        .map(|entry| entry.value().clone())
        .collect();

    HttpResponse::Ok().json(schemas)
}

/// POST /api/schemas/refresh — Re-introspect MongoDB for new/changed collections.
pub async fn refresh_schemas(state: web::Data<AppState>) -> HttpResponse {
    let total = state.refresh_schemas().await;
    let schemas: Vec<_> = state
        .schemas
        .iter()
        .map(|entry| entry.value().clone())
        .collect();

    HttpResponse::Ok().json(json!({
        "message": format!("Schemas refreshed: {} collections", total),
        "total": total,
        "schemas": schemas,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};
    use crate::domain::models::{CollectionSchema, FieldDef};
    use crate::test_helpers::test_app_state_with_schemas;

    fn make_schema(name: &str) -> CollectionSchema {
        CollectionSchema {
            name: name.to_string(),
            description: String::new(),
            fields: vec![FieldDef { name: "title".into(), field_type: "string".into(), description: None }],
            indexes: vec![],
            source: "test".into(),
        }
    }

    #[actix_web::test]
    async fn list_schemas_empty_returns_empty_array() {
        let state = test_app_state_with_schemas(vec![]).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/schemas", web::get().to(list_schemas))
        ).await;
        let req = test::TestRequest::get().uri("/api/schemas").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert!(body.as_array().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn list_schemas_returns_all_schemas() {
        let schemas = vec![make_schema("movies"), make_schema("actors")];
        let state = test_app_state_with_schemas(schemas).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/schemas", web::get().to(list_schemas))
        ).await;
        let req = test::TestRequest::get().uri("/api/schemas").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 2);
    }

    #[actix_web::test]
    async fn list_schemas_response_includes_field_names() {
        let state = test_app_state_with_schemas(vec![make_schema("movies")]).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/schemas", web::get().to(list_schemas))
        ).await;
        let req = test::TestRequest::get().uri("/api/schemas").to_request();
        let resp = test::call_service(&app, req).await;
        let body: serde_json::Value = test::read_body_json(resp).await;
        let first = &body.as_array().unwrap()[0];
        assert_eq!(first["name"], "movies");
        assert!(first["fields"].as_array().unwrap().iter().any(|f| f["name"] == "title"));
    }

    #[actix_web::test]
    async fn refresh_schemas_returns_ok_with_total_field() {
        let state = test_app_state_with_schemas(vec![]).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .route("/api/schemas/refresh", web::post().to(refresh_schemas))
        ).await;
        let req = test::TestRequest::post().uri("/api/schemas/refresh").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert!(body.get("total").is_some());
        assert!(body.get("schemas").is_some());
        assert!(body.get("message").is_some());
    }
}
