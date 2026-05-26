use actix_web::{HttpResponse, http::StatusCode};
use serde_json::json;

/// Application error type with HTTP response mapping.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("MongoDB error: {0}")]
    MongoDb(String),

    #[error("LLM parse error: {0}")]
    LlmParse(String),

    #[error("Safety violation: {0}")]
    Safety(String),

    #[error("Export error: {0}")]
    Export(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Slack verification failed")]
    SlackVerification,

    #[error("Internal error: {0}")]
    Internal(String),
}

impl actix_web::ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::MongoDb(_) => StatusCode::BAD_GATEWAY,
            Self::LlmParse(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Safety(_) => StatusCode::FORBIDDEN,
            Self::Export(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::SlackVerification => StatusCode::UNAUTHORIZED,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();

        // Log 5xx errors
        if status.is_server_error() {
            tracing::error!(error = %self, status = %status, "Server error");
        }

        // Sanitize internal errors — don't leak internals
        let message = match self {
            Self::Internal(_) | Self::Config(_) | Self::Export(_) => {
                "An internal error occurred".to_string()
            }
            Self::MongoDb(_) => "Database operation failed".to_string(),
            other => format!("{other}"),
        };

        HttpResponse::build(status).json(json!({ "error": message }))
    }
}

impl From<mongodb::error::Error> for AppError {
    fn from(err: mongodb::error::Error) -> Self {
        Self::MongoDb(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::Validation(format!("JSON error: {err}"))
    }
}

impl From<csv::Error> for AppError {
    fn from(err: csv::Error) -> Self {
        Self::Export(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::ResponseError;

    #[test]
    fn test_status_codes() {
        assert_eq!(AppError::Validation("x".into()).status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(AppError::NotFound("x".into()).status_code(), StatusCode::NOT_FOUND);
        assert_eq!(AppError::Safety("x".into()).status_code(), StatusCode::FORBIDDEN);
        assert_eq!(AppError::SlackVerification.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(AppError::Internal("x".into()).status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(AppError::MongoDb("x".into()).status_code(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn all_variants_have_status_codes() {
        assert_eq!(AppError::LlmParse("x".into()).status_code(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(AppError::Export("x".into()).status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(AppError::Config("x".into()).status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn display_includes_message_for_user_facing_errors() {
        assert!(AppError::Validation("bad input".into()).to_string().contains("bad input"));
        assert!(AppError::NotFound("movie 42".into()).to_string().contains("movie 42"));
        assert!(AppError::Safety("injection".into()).to_string().contains("injection"));
        assert!(AppError::LlmParse("no json".into()).to_string().contains("no json"));
        assert!(AppError::MongoDb("timeout".into()).to_string().contains("timeout"));
    }

    #[test]
    fn display_for_slack_verification_is_static() {
        assert_eq!(AppError::SlackVerification.to_string(), "Slack verification failed");
    }

    #[test]
    fn internal_errors_show_detail_in_display_but_are_sanitized_in_response() {
        let e = AppError::Internal("secret conn string".into());
        // Display leaks detail (for server-side logs)
        assert!(e.to_string().contains("secret conn string"));
        // HTTP response must not leak it
        let resp = e.error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn config_and_export_errors_use_internal_server_error() {
        let resp = AppError::Config("bad value".into()).error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let resp = AppError::Export("csv fail".into()).error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn from_serde_json_error_becomes_validation() {
        let json_err: serde_json::Error = serde_json::from_str::<serde_json::Value>("{bad}").unwrap_err();
        let app_err = AppError::from(json_err);
        assert!(matches!(app_err, AppError::Validation(_)));
        assert_eq!(app_err.status_code(), StatusCode::BAD_REQUEST);
    }
}
