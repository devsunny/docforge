//! DocForge HTTP service: REST API, OAuth endpoints, and MCP endpoint.

pub mod api;
pub mod config;
pub mod jobs;

use anyhow::Context;
use axum::extract::State;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use docforge_auth::AuthState;
use docforge_core::catalog::Catalog;
use docforge_core::engine::EngineRegistry;
use docforge_core::error::ConvertError;
use docforge_core::graph::ConversionGraph;
use docforge_core::service::ConversionService;
use docforge_mcp::McpState;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::AppConfig>,
    pub service: Arc<ConversionService>,
    pub auth: Arc<AuthState>,
    pub jobs: Arc<jobs::JobStore>,
    pub mcp: Arc<McpState>,
    pub openapi: Arc<serde_json::Value>,
    pub semaphore: Arc<Semaphore>,
    pub started_at: DateTime<Utc>,
}

pub async fn build_state(config: config::AppConfig) -> anyhow::Result<AppState> {
    config.ensure_dirs()?;
    let catalog = Arc::new(Catalog::load()?);
    let registry = Arc::new(EngineRegistry::detect());
    let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
    let service = Arc::new(ConversionService::new(
        catalog.clone(),
        graph.clone(),
        registry.clone(),
        config.temp_dir(),
        config.conversion_timeout_secs,
    ));
    let semaphore = Arc::new(Semaphore::new(config.conversion_concurrency));
    let auth = Arc::new(
        AuthState::initialize(config.auth.clone())
            .await
            .map_err(anyhow::Error::msg)
            .context("cannot initialize OAuth provider")?,
    );
    let jobs = Arc::new(jobs::JobStore::new(config.jobs_dir()));
    let mut mcp = McpState::new(
        service.clone(),
        auth.clone(),
        config.public_base_url.clone(),
    );
    mcp.max_inline_bytes = config.mcp_max_inline_bytes;
    let mcp = Arc::new(mcp);
    let openapi_text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/openapi.yaml"
    ));
    let openapi: serde_json::Value = serde_yaml::from_str(openapi_text)
        .map_err(|error| anyhow::anyhow!("invalid docs/openapi.yaml: {error}"))?;
    Ok(AppState {
        config: Arc::new(config),
        service,
        auth,
        jobs,
        mcp,
        openapi: Arc::new(openapi),
        semaphore,
        started_at: Utc::now(),
    })
}

pub async fn run_job(state: Arc<AppState>, id: String) {
    let _permit = match state.semaphore.acquire().await {
        Ok(permit) => permit,
        Err(_) => return,
    };
    let Some(job) = state.jobs.get(&id) else {
        return;
    };
    if job.status == jobs::JobStatus::Cancelled {
        state.jobs.delete_files(&id);
        return;
    }
    state.jobs.update(&id, |job| {
        job.status = jobs::JobStatus::Running;
        job.started_at = Some(Utc::now());
    });
    let input = state.jobs.input_path(&id, job.source_format.extension());
    let output = state.jobs.output_path(&id, job.target_format.extension());
    let duration = std::time::Duration::from_secs(state.config.conversion_timeout_secs);
    let result = tokio::time::timeout(
        duration,
        state.service.convert_file(
            &input,
            &output,
            job.source_format,
            job.target_format,
            &job.options,
        ),
    )
    .await;
    let current = state.jobs.get(&id);
    if current
        .as_ref()
        .is_some_and(|job| job.status == jobs::JobStatus::Cancelled)
    {
        let _ = std::fs::remove_dir_all(state.jobs.job_dir(&id));
        return;
    }
    match result {
        Ok(Ok(outcome)) => {
            state.jobs.update(&id, |job| {
                job.status = jobs::JobStatus::Succeeded;
                job.finished_at = Some(Utc::now());
                job.output_bytes = Some(outcome.output_bytes);
                job.route = Some(outcome.plan.summary());
                job.plan = serde_json::to_value(&outcome.plan).ok();
            });
        }
        Ok(Err(error)) => {
            state.jobs.update(&id, |job| {
                job.status = jobs::JobStatus::Failed;
                job.finished_at = Some(Utc::now());
                job.error = Some(error.to_string());
            });
        }
        Err(_) => {
            state.jobs.update(&id, |job| {
                job.status = jobs::JobStatus::Failed;
                job.finished_at = Some(Utc::now());
                job.error = Some(format!(
                    "conversion timed out after {} seconds",
                    duration.as_secs()
                ));
            });
        }
    }
}

pub fn app(state: AppState) -> Router {
    let state = Arc::new(state);
    let auth_router = docforge_auth::oauth_router(state.auth.clone());
    let mcp_router = docforge_mcp::router(state.mcp.clone());
    let api_router = api::with_auth(state.clone()).with_state(state.clone());

    let public = Router::new()
        .route("/", get(root))
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/openapi.yaml", get(openapi_yaml))
        .route("/openapi.json", get(openapi_json))
        .route("/docs", get(docs))
        .with_state(state.clone());

    let cors = cors_layer(&state.config);
    Router::new()
        .merge(public)
        .merge(auth_router)
        .merge(mcp_router)
        .nest("/api/v1", api_router)
        .layer(axum::middleware::from_fn(security_headers))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(cors)
}

fn cors_layer(config: &config::AppConfig) -> CorsLayer {
    let mut cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .max_age(Duration::from_secs(3600))
        .expose_headers([
            HeaderName::from_static("mcp-session-id"),
            HeaderName::from_static("mcp-protocol-version"),
            header::WWW_AUTHENTICATE,
            header::CONTENT_DISPOSITION,
            HeaderName::from_static("x-docforge-route"),
            HeaderName::from_static("x-docforge-duration-ms"),
        ]);
    if config.cors_allow_any {
        cors = cors.allow_origin(Any);
    } else {
        let origins: Vec<HeaderValue> = config
            .cors_origins
            .iter()
            .filter_map(|origin| HeaderValue::from_str(origin).ok())
            .collect();
        if !origins.is_empty() {
            cors = cors.allow_origin(AllowOrigin::list(origins));
        }
    }
    cors
}

async fn security_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}

async fn root(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "name": "DocForge",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Document conversion REST API and MCP server",
        "endpoints": {
            "rest_api": format!("{}/api/v1", state.config.public_base_url),
            "mcp": format!("{}/mcp", state.config.public_base_url),
            "oauth_authorization_server": format!(
                "{}/.well-known/oauth-authorization-server",
                state.config.auth.authorization_server_issuer()
            ),
            "protected_resource_metadata": state
                .config
                .auth
                .resource_metadata_url(docforge_auth::Resource::Mcp),
            "openapi": format!("{}/openapi.yaml", state.config.public_base_url),
            "docs": format!("{}/docs", state.config.public_base_url),
        },
        "catalog": state.service.catalog().stats(),
        "engines": state.service.registry().descriptors(),
    }))
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dirs_ok = state.config.jobs_dir().is_dir() && state.config.temp_dir().is_dir();
    let engines = state.service.registry().available_count();
    let status = if dirs_ok && engines > 0 {
        "ready"
    } else {
        "degraded"
    };
    (
        if dirs_ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": status,
            "directories": dirs_ok,
            "engines_available": engines,
            "catalog_loaded": state.service.catalog().entries().len(),
            "uptime_seconds": (Utc::now() - state.started_at).num_seconds(),
        })),
    )
}

async fn openapi_yaml() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/openapi.yaml"
        )),
    )
}

async fn openapi_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json((*state.openapi).clone())
}

async fn docs() -> impl IntoResponse {
    Html(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>DocForge API</title>
<link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
<style>body{margin:0;background:#fafbfc}.topbar{display:none}.swagger-ui .info .title{font-family:system-ui,sans-serif}</style>
</head><body><div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js" crossorigin></script>
<script>SwaggerUIBundle({url:'/openapi.yaml',dom_id:'#swagger-ui',deepLinking:true,persistAuthorization:true});</script>
</body></html>"#,
    )
}

/// Map a conversion error onto an HTTP API error.
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn bad_request(message: impl ToString) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            message.to_string(),
        )
    }

    pub fn not_found(message: impl ToString) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message.to_string())
    }

    pub fn internal(message: impl ToString) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            message.to_string(),
        )
    }

    pub fn from_axum(error: axum::extract::multipart::MultipartError) -> Self {
        Self::bad_request(format!("multipart error: {error}"))
    }
}

impl From<ConvertError> for ApiError {
    fn from(error: ConvertError) -> Self {
        let status = match &error {
            ConvertError::NoRoute { .. }
            | ConvertError::Unsupported { .. }
            | ConvertError::InvalidInput(_)
            | ConvertError::InvalidFormat { .. }
            | ConvertError::Format(_)
            | ConvertError::Archive(_)
            | ConvertError::Serialization(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ConvertError::TooLarge { .. } | ConvertError::InvalidFileName(_) => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            ConvertError::Timeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            ConvertError::ToolMissing { .. } => StatusCode::SERVICE_UNAVAILABLE,
            ConvertError::Cancelled => StatusCode::CONFLICT,
            ConvertError::Engine { .. }
            | ConvertError::MissingOutput
            | ConvertError::Io(_)
            | ConvertError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error.code().to_string(),
            message: error.to_string(),
            details: None,
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": {
                    "code": self.code,
                    "message": self.message,
                    "details": self.details,
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docforge_core::format::Format;

    #[test]
    fn conversion_errors_map_to_status_codes() {
        let error = ConvertError::Unsupported {
            from: Format::Doc,
            to: Format::Pdf,
        };
        assert_eq!(
            ApiError::from(error).status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let timeout = ConvertError::Timeout { seconds: 1 };
        assert_eq!(ApiError::from(timeout).status, StatusCode::GATEWAY_TIMEOUT);
    }
}
