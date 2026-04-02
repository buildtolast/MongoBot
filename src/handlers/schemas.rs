use actix_web::{web, HttpResponse};

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
