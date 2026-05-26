use actix_web::{
    body::EitherBody,
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    web, Error, HttpResponse,
};
use futures::future::{ok, LocalBoxFuture, Ready};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Bearer token authentication middleware for the dashboard API.
///
/// Skips auth for:
/// - Paths starting with `/api/slack` (Slack has its own verification)
/// - OPTIONS requests (CORS preflight)
/// - If no `dashboard_token` is configured, all requests are allowed
pub struct AuthMiddleware;

impl<S, B> Transform<S, ServiceRequest> for AuthMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Transform = AuthMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(AuthMiddlewareService { service })
    }
}

pub struct AuthMiddlewareService<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for AuthMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let path = req.path().to_string();
        let method = req.method().clone();

        // Skip auth for Slack endpoints (they have their own signing-secret verification)
        if path.starts_with("/api/slack") {
            let fut = self.service.call(req);
            return Box::pin(async move {
                let res = fut.await?;
                Ok(res.map_into_left_body())
            });
        }

        // Skip auth for OPTIONS requests (CORS preflight)
        if method == actix_web::http::Method::OPTIONS {
            let fut = self.service.call(req);
            return Box::pin(async move {
                let res = fut.await?;
                Ok(res.map_into_left_body())
            });
        }

        // Get the configured dashboard token from app state
        let expected_token = req
            .app_data::<web::Data<AppState>>()
            .and_then(|state| state.config.dashboard_token.clone());

        // If no token is configured, allow all requests
        let expected_token = match expected_token {
            Some(t) if !t.is_empty() => t,
            _ => {
                let fut = self.service.call(req);
                return Box::pin(async move {
                    let res = fut.await?;
                    Ok(res.map_into_left_body())
                });
            }
        };

        // Extract the Bearer token from the Authorization header
        let auth_header = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let provided_token = extract_bearer_token(auth_header.as_deref());

        match provided_token {
            Some(token) => {
                // Constant-time comparison to prevent timing attacks
                let expected_bytes = expected_token.as_bytes();
                let provided_bytes = token.as_bytes();

                let is_valid = expected_bytes.len() == provided_bytes.len()
                    && expected_bytes.ct_eq(provided_bytes).into();

                if is_valid {
                    let fut = self.service.call(req);
                    Box::pin(async move {
                        let res = fut.await?;
                        Ok(res.map_into_left_body())
                    })
                } else {
                    Box::pin(async move {
                        let response = HttpResponse::Unauthorized()
                            .json(json!({ "error": "Invalid authentication token" }));
                        Ok(req.into_response(response).map_into_right_body())
                    })
                }
            }
            None => Box::pin(async move {
                let response = HttpResponse::Unauthorized()
                    .json(json!({ "error": "Missing Authorization header" }));
                Ok(req.into_response(response).map_into_right_body())
            }),
        }
    }
}

/// Extract the Bearer token value from an Authorization header string.
/// Returns `None` if the header is missing or does not have the `Bearer ` prefix.
pub(crate) fn extract_bearer_token(header: Option<&str>) -> Option<String> {
    header?.strip_prefix("Bearer ").map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_same() {
        let a = b"test-token-123";
        let b = b"test-token-123";
        assert!(bool::from(a.ct_eq(b)));
    }

    #[test]
    fn test_constant_time_eq_different() {
        let a = b"test-token-123";
        let b = b"wrong-token-456";
        assert!(!bool::from(a.ct_eq(b)));
    }

    #[test]
    fn extract_bearer_token_valid() {
        assert_eq!(extract_bearer_token(Some("Bearer my-secret-token")), Some("my-secret-token".into()));
    }

    #[test]
    fn extract_bearer_token_missing_header() {
        assert_eq!(extract_bearer_token(None), None);
    }

    #[test]
    fn extract_bearer_token_wrong_prefix() {
        assert_eq!(extract_bearer_token(Some("Token my-secret-token")), None);
        assert_eq!(extract_bearer_token(Some("Basic dXNlcjpwYXNz")), None);
    }

    #[test]
    fn extract_bearer_token_empty_string() {
        assert_eq!(extract_bearer_token(Some("")), None);
    }

    #[test]
    fn extract_bearer_token_bearer_prefix_only() {
        assert_eq!(extract_bearer_token(Some("Bearer ")), Some("".into()));
    }

    // ── actix-web middleware integration tests ───────────────────────────────

    async fn dummy_handler() -> actix_web::HttpResponse {
        actix_web::HttpResponse::Ok().json(serde_json::json!({ "ok": true }))
    }

    #[actix_web::test]
    async fn middleware_allows_slack_path_without_token() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let mut state = test_app_state().await;
        state.config.dashboard_token = Some("secret".into());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .wrap(AuthMiddleware)
                .route("/api/slack/commands", web::post().to(dummy_handler))
        ).await;
        let req = test::TestRequest::post().uri("/api/slack/commands").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }

    #[actix_web::test]
    async fn middleware_allows_options_without_token() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let mut state = test_app_state().await;
        state.config.dashboard_token = Some("secret".into());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .wrap(AuthMiddleware)
                // Register for all methods so OPTIONS request finds the route
                .route("/api/query", web::route().to(dummy_handler))
        ).await;
        let req = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/api/query")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }

    #[actix_web::test]
    async fn middleware_allows_all_when_no_token_configured() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let state = test_app_state().await; // dashboard_token = None
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .wrap(AuthMiddleware)
                .route("/api/health", web::get().to(dummy_handler))
        ).await;
        let req = test::TestRequest::get().uri("/api/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }

    #[actix_web::test]
    async fn middleware_rejects_missing_auth_header() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let mut state = test_app_state().await;
        state.config.dashboard_token = Some("secret".into());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .wrap(AuthMiddleware)
                .route("/api/health", web::get().to(dummy_handler))
        ).await;
        let req = test::TestRequest::get().uri("/api/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[actix_web::test]
    async fn middleware_rejects_wrong_token() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let mut state = test_app_state().await;
        state.config.dashboard_token = Some("correct-token".into());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .wrap(AuthMiddleware)
                .route("/api/health", web::get().to(dummy_handler))
        ).await;
        let req = test::TestRequest::get()
            .uri("/api/health")
            .insert_header(("Authorization", "Bearer wrong-token"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[actix_web::test]
    async fn middleware_accepts_correct_token() {
        use actix_web::{test, web, App};
        use crate::test_helpers::test_app_state;
        let mut state = test_app_state().await;
        state.config.dashboard_token = Some("correct-token".into());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state))
                .wrap(AuthMiddleware)
                .route("/api/health", web::get().to(dummy_handler))
        ).await;
        let req = test::TestRequest::get()
            .uri("/api/health")
            .insert_header(("Authorization", "Bearer correct-token"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), 200);
    }
}
