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

        let provided_token = auth_header
            .as_deref()
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(|t| t.to_string());

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
}
