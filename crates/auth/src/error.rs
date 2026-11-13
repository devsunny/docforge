//! OAuth 2.1 / RFC 9728 errors.

use axum::http::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid_request: {0}")]
    InvalidRequest(String),
    #[error("invalid_client")]
    InvalidClient,
    #[error("invalid_grant: {0}")]
    InvalidGrant(String),
    #[error("unauthorized_client: {0}")]
    UnauthorizedClient(String),
    #[error("unsupported_grant_type")]
    UnsupportedGrantType,
    #[error("unsupported_response_type")]
    UnsupportedResponseType,
    #[error("invalid_scope: {0}")]
    InvalidScope(String),
    #[error("access_denied: {0}")]
    AccessDenied(String),
    #[error("invalid_token: {0}")]
    InvalidToken(String),
    #[error("insufficient_scope: {0}")]
    InsufficientScope(String),
    #[error("server_error: {0}")]
    ServerError(String),
    #[error("temporarily_unavailable: {0}")]
    TemporarilyUnavailable(String),
}

impl AuthError {
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::InvalidRequest(_) => "invalid_request",
            AuthError::InvalidClient => "invalid_client",
            AuthError::InvalidGrant(_) => "invalid_grant",
            AuthError::UnauthorizedClient(_) => "unauthorized_client",
            AuthError::UnsupportedGrantType => "unsupported_grant_type",
            AuthError::UnsupportedResponseType => "unsupported_response_type",
            AuthError::InvalidScope(_) => "invalid_scope",
            AuthError::AccessDenied(_) => "access_denied",
            AuthError::InvalidToken(_) => "invalid_token",
            AuthError::InsufficientScope(_) => "insufficient_scope",
            AuthError::ServerError(_) => "server_error",
            AuthError::TemporarilyUnavailable(_) => "temporarily_unavailable",
        }
    }

    pub fn description(&self) -> String {
        match self {
            AuthError::InvalidRequest(message)
            | AuthError::InvalidGrant(message)
            | AuthError::UnauthorizedClient(message)
            | AuthError::InvalidScope(message)
            | AuthError::AccessDenied(message)
            | AuthError::InvalidToken(message)
            | AuthError::InsufficientScope(message)
            | AuthError::ServerError(message)
            | AuthError::TemporarilyUnavailable(message) => message.clone(),
            AuthError::InvalidClient => "client authentication failed".to_string(),
            AuthError::UnsupportedGrantType => "unsupported grant type".to_string(),
            AuthError::UnsupportedResponseType => {
                "only response_type=code is supported".to_string()
            }
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AuthError::InvalidClient => StatusCode::UNAUTHORIZED,
            AuthError::InvalidToken(_) | AuthError::InsufficientScope(_) => {
                StatusCode::UNAUTHORIZED
            }
            AuthError::ServerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    pub fn oauth_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": self.code(),
            "error_description": self.description(),
        })
    }
}
