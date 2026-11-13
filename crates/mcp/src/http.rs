//! MCP Streamable HTTP transport.

use crate::protocol::{code, RpcRequest, RpcResponse, SUPPORTED_PROTOCOLS};
use crate::server::{handle_rpc, McpState};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use docforge_auth::config::Resource;
use docforge_auth::middleware::{
    insufficient_scope_response, unauthorized_response, verify_bearer,
};
use docforge_auth::AuthError;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn router(state: Arc<McpState>) -> Router {
    Router::new()
        .route(
            "/mcp",
            post(handle_post).get(handle_get).delete(handle_delete),
        )
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
        .with_state(state)
}

async fn handle_post(
    State(state): State<Arc<McpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = check_origin(&state, &headers) {
        return response;
    }
    if let Some(response) = check_protocol_header(&headers) {
        return response;
    }
    if let Err(response) = authenticate(&state, &headers) {
        return response;
    }
    if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
        let content_type = content_type.to_str().unwrap_or("");
        if !content_type
            .to_ascii_lowercase()
            .starts_with("application/json")
        {
            return json_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                json!({"error": "unsupported_media_type", "error_description": "Content-Type must be application/json"}),
                None,
                None,
            );
        }
    }

    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            let response = RpcResponse::error(
                None,
                code::PARSE_ERROR,
                format!("parse error: {error}"),
                None,
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::to_value(response).unwrap(),
                None,
                None,
            );
        }
    };
    if value.is_array() {
        let response = RpcResponse::error(
            None,
            code::INVALID_REQUEST,
            "JSON-RPC batches are not supported; send one request per HTTP POST",
            None,
        );
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::to_value(response).unwrap(),
            None,
            None,
        );
    }
    let request: RpcRequest = match serde_json::from_value(value) {
        Ok(request) => request,
        Err(error) => {
            let response = RpcResponse::error(None, code::INVALID_REQUEST, error.to_string(), None);
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::to_value(response).unwrap(),
                None,
                None,
            );
        }
    };

    let session_header = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(session_id) = &session_header {
        if !state.sessions.read().contains_key(session_id) {
            return json_response(
                StatusCode::NOT_FOUND,
                json!({"error": "session_not_found", "error_description": "unknown Mcp-Session-Id"}),
                None,
                None,
            );
        }
    }

    state.prune_sessions();
    let outcome = handle_rpc(&state, request, session_header.as_deref()).await;
    let session_id = outcome.session_id.or(session_header);
    if outcome.notification {
        let mut response = StatusCode::ACCEPTED.into_response();
        add_common_headers(
            &mut response,
            session_id.as_deref(),
            Some(outcome.protocol.as_str()),
        );
        return response;
    }
    json_response(
        StatusCode::OK,
        outcome.body.unwrap_or_else(|| json!({})),
        session_id.as_deref(),
        Some(outcome.protocol.as_str()),
    )
}

async fn handle_get(State(_state): State<Arc<McpState>>) -> Response {
    let mut response = (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({
            "error": "method_not_allowed",
            "error_description": "This server does not offer a standalone SSE stream. Use POST /mcp for JSON-RPC requests."
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::ALLOW,
        HeaderValue::from_static("POST, DELETE, OPTIONS"),
    );
    response
}

async fn handle_delete(State(state): State<Arc<McpState>>, headers: HeaderMap) -> Response {
    if let Some(response) = check_origin(&state, &headers) {
        return response;
    }
    if let Err(response) = authenticate(&state, &headers) {
        return response;
    }
    let session_id = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok());
    let Some(session_id) = session_id else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"error": "invalid_request", "error_description": "Mcp-Session-Id is required"}),
            None,
            None,
        );
    };
    let removed = state.sessions.write().remove(session_id).is_some();
    if removed {
        StatusCode::NO_CONTENT.into_response()
    } else {
        json_response(
            StatusCode::NOT_FOUND,
            json!({"error": "session_not_found"}),
            None,
            None,
        )
    }
}

#[allow(clippy::result_large_err)]
fn authenticate(state: &McpState, headers: &HeaderMap) -> Result<(), Response> {
    match verify_bearer(&state.auth, headers, Resource::Mcp, Some("mcp")) {
        Ok(_) => Ok(()),
        Err(AuthError::InsufficientScope(scope)) => Err(insufficient_scope_response(
            &state.auth,
            Resource::Mcp,
            &scope,
        )),
        Err(error) => Err(unauthorized_response(
            &state.auth,
            Resource::Mcp,
            error.code(),
            &error.description(),
            Some("mcp"),
        )),
    }
}

fn check_origin(state: &McpState, headers: &HeaderMap) -> Option<Response> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())?;
    if state.origin_allowed(origin) {
        None
    } else {
        Some(json_response(
            StatusCode::FORBIDDEN,
            json!({
                "error": "invalid_origin",
                "error_description": "the Origin header is not allowed; configure MCP_ALLOWED_ORIGINS"
            }),
            None,
            None,
        ))
    }
}

fn check_protocol_header(headers: &HeaderMap) -> Option<Response> {
    let value = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())?;
    if SUPPORTED_PROTOCOLS.contains(&value) {
        None
    } else {
        Some(json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "error": "unsupported_protocol_version",
                "error_description": format!("supported versions: {}", SUPPORTED_PROTOCOLS.join(", "))
            }),
            None,
            None,
        ))
    }
}

fn json_response(
    status: StatusCode,
    body: Value,
    session_id: Option<&str>,
    protocol: Option<&str>,
) -> Response {
    let mut response = (status, Json(body)).into_response();
    add_common_headers(&mut response, session_id, protocol);
    response
}

fn add_common_headers(response: &mut Response, session_id: Option<&str>, protocol: Option<&str>) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(session_id) = session_id {
        if let Ok(value) = HeaderValue::from_str(session_id) {
            response.headers_mut().insert("mcp-session-id", value);
        }
    }
    if let Some(protocol) = protocol {
        if let Ok(value) = HeaderValue::from_str(protocol) {
            response.headers_mut().insert("mcp-protocol-version", value);
        }
    }
    response.headers_mut().insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Mcp-Session-Id, MCP-Protocol-Version, WWW-Authenticate"),
    );
}
