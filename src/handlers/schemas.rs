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
