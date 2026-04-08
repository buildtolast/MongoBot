use actix_web::{web, HttpResponse};

use crate::domain::executor::{self, QueryResult};
use crate::domain::export::{documents_to_csv, documents_to_json_pretty};
use crate::domain::models::{ExportFormat, ExportParams};
use crate::error::AppError;
use crate::state::AppState;

/// Export query results as CSV or JSON file download.
pub async fn handle_export(
    state: web::Data<AppState>,
    params: web::Query<ExportParams>,
) -> Result<HttpResponse, AppError> {
    // 1. Parse natural language into a structured query via LLM
    let schema_context = state.schema_context();
    let (parsed, _raw, _model) = state
        .llm
        .parse_query(&params.query, &schema_context, None)
        .await?;

    // 2. Validate query safety
    let schema = state.get_schema(&parsed.collection);
    state.safety.validate(&parsed, schema.as_ref())?;

    // 3. Execute the query
    let collection_name = parsed.collection.clone();
    let result = executor::execute(&state.mongo, &parsed, schema.as_ref()).await?;

    // 4. Build filename with timestamp
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");

    // 5. Determine format (default to JSON)
    let format = params.format.clone().unwrap_or(ExportFormat::Json);

    // 6. Extract documents from the result
    let documents = match result {
        QueryResult::Documents(docs) => docs,
        QueryResult::Count(n) => {
            vec![mongodb::bson::doc! { "count": n as i64 }]
        }
        QueryResult::Values(vals) => vals
            .into_iter()
            .map(|v| mongodb::bson::doc! { "value": v })
            .collect(),
        QueryResult::Write(w) => {
            vec![mongodb::bson::doc! {
                "operation": &w.operation,
                "collection": &w.collection,
                "matched_count": w.matched_count as i64,
                "modified_count": w.modified_count as i64,
            }]
        }
    };

    // 7. Format and return with Content-Disposition header
    match format {
        ExportFormat::Csv => {
            let csv_data = documents_to_csv(&documents)?;
            let filename = format!("{collection_name}_{timestamp}.csv");
            Ok(HttpResponse::Ok()
                .content_type("text/csv; charset=utf-8")
                .insert_header((
                    "Content-Disposition",
                    format!("attachment; filename=\"{filename}\""),
                ))
                .body(csv_data))
        }
        ExportFormat::Json => {
            let json_data = documents_to_json_pretty(&documents)?;
            let filename = format!("{collection_name}_{timestamp}.json");
            Ok(HttpResponse::Ok()
                .content_type("application/json; charset=utf-8")
                .insert_header((
                    "Content-Disposition",
                    format!("attachment; filename=\"{filename}\""),
                ))
                .body(json_data))
        }
    }
}
