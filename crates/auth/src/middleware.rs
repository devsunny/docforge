//! Resource-server middleware for the REST API and MCP endpoint.

use crate::config::Resource;
use crate::error::AuthError;
use crate::jwt::AuthContext;
use crate::store::AuthState;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;

pub async fn require_api(
    State(state): State<Arc<AuthState>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS {
        return next.run(request).await;
    }
    let required = if matches!(*request.method(), Method::GET | Method::HEAD) {
        None
    } else {
        Some("convert")
    };
    authorize_request(state, request, next, Resource::Api, required).await
}

pub async fn require_mcp(
    State(state): State<Arc<AuthState>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS {
        return next.run(request).await;
    }
    authorize_request(state, request, next, Resource::Mcp, Some("mcp")).await
}

async fn authorize_request(
    state: Arc<AuthState>,
    mut request: Request,
    next: Next,
    resource: Resource,
    required_scope: Option<&'static str>,
) -> Response {
    let context = match verify_bearer(&state, request.headers(), resource, required_scope) {
        Ok(context) => context,
        Err(error) => {
            return match error {
                AuthError::InsufficientScope(scope) => {
                    insufficient_scope_response(&state, resource, &scope)
                }
                other => unauthorized_response(
                    &state,
                    resource,
                    other.code(),
                    &other.description(),
                    required_scope,
                ),
            }
        }
    };
    request.extensions_mut().insert(context);
    next.run(request).await
}

/// Verify a Bearer token for a protected resource. This is public so the MCP
/// endpoint and other transports can share exactly the same validation.
pub fn verify_bearer(
    state: &AuthState,
    headers: &axum::http::HeaderMap,
    resource: Resource,
    required_scope: Option<&str>,
) -> Result<AuthContext, AuthError> {
    let token = bearer_token(headers)
        .ok_or_else(|| AuthError::InvalidToken("a Bearer access token is required".into()))?;
    let claims = state.verify_access(token, resource)?;
    if state.store.is_revoked(&claims.jti) {
        return Err(AuthError::InvalidToken(
            "the access token has been revoked".into(),
        ));
    }
    let context = AuthContext::from_claims(&claims);
    if let Some(scope) = required_scope {
        if !context.has_scope(scope) {
            return Err(AuthError::InsufficientScope(scope.to_string()));
        }
    }
    if resource == Resource::Api
        && required_scope.is_none()
        && !context.has_scope("convert")
        && !context.has_scope("convert:read")
    {
        return Err(AuthError::InsufficientScope("convert:read".into()));
    }
    Ok(context)
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

pub fn unauthorized_response(
    state: &AuthState,
    resource: Resource,
    error: &str,
    description: &str,
    scope: Option<&str>,
) -> Response {
    let challenge = challenge_header(state, resource, Some((error, description)), scope);
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": error,
            "error_description": description,
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

pub fn insufficient_scope_response(state: &AuthState, resource: Resource, scope: &str) -> Response {
    let challenge = challenge_header(
        state,
        resource,
        Some((
            "insufficient_scope",
            "the access token lacks the required scope",
        )),
        Some(scope),
    );
    let mut response = (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "insufficient_scope",
            "error_description": "the access token lacks the required scope",
            "required_scope": scope,
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

fn challenge_header(
    state: &AuthState,
    resource: Resource,
    error: Option<(&str, &str)>,
    scope: Option<&str>,
) -> String {
    let metadata = state.config.resource_metadata_url(resource);
    let mut challenge = format!("Bearer resource_metadata=\"{metadata}\"");
    if let Some(scope) = scope {
        challenge.push_str(&format!(", scope=\"{scope}\""));
    }
    if let Some((error, description)) = error {
        challenge.push_str(&format!(
            ", error=\"{}\", error_description=\"{}\"",
            sanitize_header(error),
            sanitize_header(description)
        ));
    }
    challenge
}

fn sanitize_header(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_control() || ch == '"' {
                '\''
            } else {
                ch
            }
        })
        .collect()
}

/// Extract the authenticated context from a request extension.
pub fn context(request: &Request) -> Result<AuthContext, AuthError> {
    request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .ok_or_else(|| AuthError::InvalidToken("authentication context is missing".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_has_rfc9728_metadata() {
        let state = AuthState::new(crate::config::AuthConfig::for_test("http://127.0.0.1:8080"));
        let challenge = challenge_header(
            &state,
            Resource::Mcp,
            Some(("invalid_token", "expired")),
            None,
        );
        assert!(challenge.contains(
            "resource_metadata=\"http://127.0.0.1:8080/.well-known/oauth-protected-resource/mcp\""
        ));
        assert!(challenge.contains("error=\"invalid_token\""));
    }

    #[test]
    fn bearer_parser_is_case_insensitive() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer abc"),
        );
        assert_eq!(bearer_token(&headers), Some("abc"));
    }
}
