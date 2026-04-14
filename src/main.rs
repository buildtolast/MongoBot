use actix_cors::Cors;
use actix_web::{web, App, HttpServer, HttpResponse, middleware as actix_mw, http::header};
use clap::Parser;
use tracing_actix_web::TracingLogger;

use slack_mongo_bot::config::AppConfig;
use slack_mongo_bot::frontend;
use slack_mongo_bot::handlers;
use slack_mongo_bot::middleware;
use slack_mongo_bot::state::AppState;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Parse configuration from CLI args + env vars
    let config = AppConfig::parse();
    tracing::info!(?config, "Starting server");

    // Build shared application state (connects to MongoDB, loads schemas, etc.)
    let app_state = AppState::new(&config).await?;
    let state = web::Data::new(app_state);

    let bind_addr = format!("{}:{}", config.host, config.port);
    tracing::info!(addr = %bind_addr, "Binding HTTP server");

    HttpServer::new(move || {
        App::new()
            // Structured request logging
            .wrap(TracingLogger::default())
            // CORS — handles OPTIONS preflight automatically
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
                    .allowed_headers(vec![header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
                    .max_age(3600)
            )
            // Security headers
            .wrap(
                actix_mw::DefaultHeaders::new()
                    .add((header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
                    .add((header::X_FRAME_OPTIONS, "DENY"))
                    .add((header::REFERRER_POLICY, "strict-origin-when-cross-origin"))
                    .add((
                        header::CONTENT_SECURITY_POLICY,
                        "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:",
                    ))
            )
            // Shared state
            .app_data(state.clone())

            // === Public routes (no auth) ===
            .route("/", web::get().to(serve_frontend))
            .route("/api/health", web::get().to(handlers::health::health_check))
            .route("/api/models", web::get().to(handlers::health::list_models))
            .route("/api/providers", web::get().to(handlers::health::list_providers))
            .route("/api/version", web::get().to(handlers::health::version))
            .route("/api/config", web::get().to(handlers::health::get_config))

            // === Slack webhooks (own auth via signing secret) ===
            .route(
                "/api/slack/commands",
                web::post().to(handlers::slack_commands::handle_slash_command),
            )
            .route(
                "/api/slack/events",
                web::post().to(handlers::slack_commands::handle_events),
            )
            .route(
                "/api/slack/actions",
                web::post().to(handlers::slack_actions::handle_action),
            )

            // === Authenticated Web UI API ===
            .service(
                web::scope("/api")
                    .wrap(middleware::auth::AuthMiddleware)
                    .route("/query", web::post().to(handlers::query::handle_query))
                    .route("/execute", web::post().to(handlers::query::handle_execute))
                    .route("/export", web::get().to(handlers::export::handle_export))
                    .route("/schemas", web::get().to(handlers::schemas::list_schemas))
                    .route("/schemas/refresh", web::post().to(handlers::schemas::refresh_schemas))
            )

            // === WebSocket ===
            .route("/ws/live", web::get().to(handlers::ws::ws_handler))
    })
    .bind(&bind_addr)?
    .run()
    .await?;

    Ok(())
}

/// Serve the embedded single-page frontend.
async fn serve_frontend() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(frontend::INDEX_HTML)
}
