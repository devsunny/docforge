//! REST API v1.

use crate::jobs::{JobRecord, JobStatus, JobStore};
use crate::{ApiError, AppState};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Extension, Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use docforge_auth::middleware::require_api;
use docforge_auth::AuthContext;
use docforge_core::format::Format;
use docforge_core::options::ConversionOptions;
use docforge_core::service::{output_filename, sanitize_filename};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::time::timeout;
use tokio_util::io::ReaderStream;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/formats", get(formats))
        .route("/conversions", get(conversions))
        .route("/conversions/{rank}", get(conversion_by_rank))
        .route("/conversions/plan", post(plan))
        .route("/engines", get(engines))
        .route("/stats", get(stats))
        .route("/convert", post(convert_sync))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/{id}", get(get_job).delete(delete_job))
        .route("/jobs/{id}/download", get(download_job))
        .layer(DefaultBodyLimit::max(128 * 1024 * 1024))
}

pub fn with_auth(state: Arc<AppState>) -> Router<Arc<AppState>> {
    router().layer(axum::middleware::from_fn_with_state(
        state.auth.clone(),
        require_api,
    ))
}

async fn formats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let formats: Vec<Value> = Format::ALL
        .iter()
        .map(|format| {
            let mut info = serde_json::to_value(format.info()).unwrap_or_default();
            let mut targets = state.service.catalog().targets_for(*format);
            for target in state.service.graph().direct_targets(*format) {
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            targets.sort();
            info["targets"] = Value::Array(
                targets
                    .into_iter()
                    .map(|target| json!(target.id()))
                    .collect(),
            );
            info
        })
        .collect();
    Json(json!({"count": formats.len(), "formats": formats}))
}

#[derive(Debug, Deserialize)]
struct ConversionQuery {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    available_only: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

async fn conversions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConversionQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100).clamp(1, 100);
    let offset = query.offset.unwrap_or(0);
    let mut conversions = Vec::new();
    for entry in state.service.catalog().entries() {
        if let Some(category) = &query.category {
            if !entry.category.eq_ignore_ascii_case(category) {
                continue;
            }
        }
        let available = state.service.graph().is_catalog_available(entry.rank);
        if query.available_only.unwrap_or(false) && !available {
            continue;
        }
        let plan = state.service.graph().catalog_plan(entry).ok();
        conversions.push(json!({
            "rank": entry.rank,
            "source": entry.source_format,
            "target": entry.target_format,
            "source_formats": entry.source_formats.iter().map(|f| f.id()).collect::<Vec<_>>(),
            "target_formats": entry.target_formats.iter().map(|f| f.id()).collect::<Vec<_>>(),
            "category": entry.category,
            "use_case": entry.use_case,
            "requires_ocr": entry.requires_ocr,
            "searchable": entry.searchable,
            "available": available,
            "direct": plan.as_ref().map(|p| p.direct).unwrap_or(false),
            "route": plan.as_ref().map(|p| p.summary()),
            "engine_steps": plan.as_ref().map(|p| p.engine_ids()).unwrap_or_default(),
            "total_cost": plan.map(|p| p.total_cost),
        }));
    }
    let total = conversions.len();
    let page: Vec<Value> = conversions.into_iter().skip(offset).take(limit).collect();
    Json(
        json!({"total": total, "count": page.len(), "limit": limit, "offset": offset, "conversions": page}),
    )
}

async fn conversion_by_rank(
    State(state): State<Arc<AppState>>,
    Path(rank): Path<u16>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = state
        .service
        .catalog()
        .get(rank)
        .ok_or_else(|| ApiError::not_found(format!("conversion rank {rank} does not exist")))?;
    let plan = state.service.graph().catalog_plan(entry).ok();
    Ok(Json(json!({
        "rank": entry.rank,
        "source": entry.source_format,
        "target": entry.target_format,
        "category": entry.category,
        "use_case": entry.use_case,
        "requires_ocr": entry.requires_ocr,
        "searchable": entry.searchable,
        "available": state.service.graph().is_catalog_available(rank),
        "route": plan.as_ref().map(|p| p.summary()),
        "plan": plan,
    })))
}

#[derive(Debug, Deserialize)]
struct PlanRequest {
    source_format: String,
    target_format: String,
    #[serde(default)]
    options: ConversionOptions,
}

async fn plan(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PlanRequest>,
) -> Result<impl IntoResponse, ApiError> {
    request.options.validate().map_err(ApiError::bad_request)?;
    let source = Format::parse(&request.source_format).map_err(ApiError::bad_request)?;
    let target = Format::parse(&request.target_format).map_err(ApiError::bad_request)?;
    let plan = state
        .service
        .plan(source, target, &request.options)
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "source": source,
        "target": target,
        "plan": plan,
        "summary": plan.summary(),
        "catalog_rank": plan.catalog_rank,
    })))
}

async fn engines(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "engines": state.service.registry().descriptors(),
        "available": state.service.registry().available_count(),
    }))
}

async fn stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "catalog": state.service.catalog().stats(),
        "categories": state.service.catalog().categories(),
        "engines": state.service.registry().descriptors(),
        "graph": {
            "edges": state.service.graph().edges().len(),
            "catalog_available": state.service.catalog().entries().iter().filter(|entry| state.service.graph().is_catalog_available(entry.rank)).count(),
        },
        "jobs": "in-memory; see /api/v1/jobs",
    }))
}

struct Upload {
    filename: String,
    bytes: Vec<u8>,
    target: Format,
    source: Option<Format>,
    options: ConversionOptions,
    client_mime: Option<String>,
}

async fn parse_upload(
    state: &AppState,
    mut multipart: Multipart,
    max_bytes: usize,
) -> Result<Upload, ApiError> {
    let mut file: Option<(String, Vec<u8>, Option<String>)> = None;
    let mut target: Option<String> = None;
    let mut source: Option<String> = None;
    let mut options: Option<ConversionOptions> = None;
    let mut rank: Option<u16> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::bad_request(format!("invalid multipart body: {error}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let filename = field
                    .file_name()
                    .map(sanitize_filename)
                    .unwrap_or_else(|| "document".to_string());
                let mime = field.content_type().map(str::to_string);
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|error| ApiError::bad_request(format!("cannot read file: {error}")))?;
                if bytes.len() > max_bytes {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "file_too_large",
                        format!(
                            "file is {} bytes; limit is {} bytes",
                            bytes.len(),
                            max_bytes
                        ),
                    ));
                }
                file = Some((filename, bytes.to_vec(), mime));
            }
            "target_format" | "target" => {
                target = Some(field.text().await.map_err(ApiError::from_axum)?);
            }
            "source_format" | "source" => {
                source = Some(field.text().await.map_err(ApiError::from_axum)?);
            }
            "options" => {
                let text = field.text().await.map_err(ApiError::from_axum)?;
                if !text.trim().is_empty() {
                    options = Some(serde_json::from_str(&text).map_err(|error| {
                        ApiError::bad_request(format!("options must be JSON: {error}"))
                    })?);
                }
            }
            "rank" => {
                let text = field.text().await.map_err(ApiError::from_axum)?;
                rank = Some(
                    text.trim()
                        .parse()
                        .map_err(|_| ApiError::bad_request("rank must be a number"))?,
                );
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let (filename, bytes, client_mime) =
        file.ok_or_else(|| ApiError::bad_request("multipart field 'file' is required"))?;
    if bytes.is_empty() {
        return Err(ApiError::bad_request("uploaded file is empty"));
    }
    let requested_target = target
        .map(|target| Format::parse(&target).map_err(ApiError::bad_request))
        .transpose()?;
    let mut source = match source {
        Some(source) => Some(Format::parse(&source).map_err(ApiError::bad_request)?),
        None => None,
    };
    let mut target = requested_target;
    let mut options = options.unwrap_or_default();
    if let Some(rank) = rank {
        let entry =
            state.service.catalog().get(rank).ok_or_else(|| {
                ApiError::not_found(format!("conversion rank {rank} does not exist"))
            })?;
        let detected =
            docforge_core::detect::detect_bytes(&bytes, Some(&filename), client_mime.as_deref())
                .ok()
                .map(|detected| detected.format);
        source = detected
            .filter(|format| entry.source_formats.contains(format))
            .or_else(|| entry.source_formats.first().copied());
        target = Some(entry.target_formats[0]);
        options.ocr = entry.requires_ocr || entry.searchable;
    }
    let target = target.ok_or_else(|| {
        ApiError::bad_request("multipart field 'target_format' or 'rank' is required")
    })?;
    options.validate().map_err(ApiError::bad_request)?;
    Ok(Upload {
        filename,
        bytes,
        target,
        source,
        options,
        client_mime,
    })
}

async fn convert_sync(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let upload = parse_upload(&state, multipart, state.config.max_sync_bytes).await?;
    if upload.bytes.len() > state.config.max_sync_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            "synchronous conversions are limited to the configured MAX_SYNC_MB",
        ));
    }
    let _ = auth;
    let dir = tempfile::Builder::new()
        .prefix("sync-")
        .tempdir_in(state.config.temp_dir())
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let from = match upload.source {
        Some(source) => source,
        None => {
            docforge_core::detect::detect_bytes(
                &upload.bytes,
                Some(&upload.filename),
                upload.client_mime.as_deref(),
            )
            .map_err(ApiError::from)?
            .format
        }
    };
    let input_path = dir.path().join(format!("input.{}", from.extension()));
    std::fs::write(&input_path, &upload.bytes).map_err(ApiError::from)?;
    let output_path = dir
        .path()
        .join(format!("output.{}", upload.target.extension()));
    let duration = std::time::Duration::from_secs(state.config.conversion_timeout_secs);
    let outcome = timeout(
        duration,
        state.service.convert_file(
            &input_path,
            &output_path,
            from,
            upload.target,
            &upload.options,
        ),
    )
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "conversion timed out",
        )
    })?
    .map_err(ApiError::from)?;
    let bytes = std::fs::read(&output_path).map_err(ApiError::from)?;
    let name = output_filename(&upload.filename, upload.target);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(upload.target.mime())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .unwrap_or(HeaderValue::from_static("attachment")),
    );
    headers.insert(
        "x-docforge-route",
        HeaderValue::from_str(&outcome.plan.summary())
            .unwrap_or(HeaderValue::from_static("unknown")),
    );
    headers.insert(
        "x-docforge-duration-ms",
        HeaderValue::from_str(&outcome.duration_ms.to_string())
            .unwrap_or(HeaderValue::from_static("0")),
    );
    Ok(response)
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let upload = parse_upload(&state, multipart, state.config.max_upload_bytes).await?;
    let source = match upload.source {
        Some(source) => source,
        None => {
            docforge_core::detect::detect_bytes(
                &upload.bytes,
                Some(&upload.filename),
                upload.client_mime.as_deref(),
            )
            .map_err(ApiError::from)?
            .format
        }
    };
    let mut record = JobStore::new_record(
        &auth.subject,
        &auth.client_id,
        source,
        upload.target,
        upload.filename.clone(),
        upload.bytes.len() as u64,
        &state.config.public_base_url,
    );
    record.output_name = output_filename(&upload.filename, upload.target);
    record.options = upload.options.clone();
    std::fs::create_dir_all(state.jobs.job_dir(&record.id)).map_err(ApiError::from)?;
    let input_path = state.jobs.input_path(&record.id, source.extension());
    std::fs::write(&input_path, &upload.bytes).map_err(ApiError::from)?;
    let job_id = record.id.clone();
    state.jobs.insert(record.clone());
    let state_clone = state.clone();
    tokio::spawn(async move {
        crate::run_job(state_clone, job_id).await;
    });
    let mut response = (StatusCode::ACCEPTED, Json(record.clone())).into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/api/v1/jobs/{}", record.id))
            .unwrap_or(HeaderValue::from_static("/api/v1/jobs")),
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct JobsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    status: Option<String>,
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<JobsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let status = match query.status.as_deref() {
        None => None,
        Some("queued") => Some(JobStatus::Queued),
        Some("running") => Some(JobStatus::Running),
        Some("succeeded") => Some(JobStatus::Succeeded),
        Some("failed") => Some(JobStatus::Failed),
        Some("cancelled") => Some(JobStatus::Cancelled),
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "unknown job status '{other}'"
            )))
        }
    };
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let offset = query.offset.unwrap_or(0);
    let jobs = state.jobs.list(&auth.subject, status, limit, offset);
    Ok(Json(
        json!({"jobs": jobs, "count": jobs.len(), "limit": limit, "offset": offset}),
    ))
}

fn owned_job(state: &AppState, auth: &AuthContext, id: &str) -> Result<JobRecord, ApiError> {
    let job = state
        .jobs
        .get(id)
        .ok_or_else(|| ApiError::not_found("job not found"))?;
    if job.owner != auth.subject {
        return Err(ApiError::not_found("job not found"));
    }
    Ok(job)
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(owned_job(&state, &auth, &id)?))
}

async fn delete_job(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let job = owned_job(&state, &auth, &id)?;
    if job.status == JobStatus::Running || job.status == JobStatus::Queued {
        state.jobs.update(&id, |job| {
            job.status = JobStatus::Cancelled;
            job.finished_at = Some(chrono::Utc::now());
            job.error = Some("cancelled by the client".into());
        });
        return Ok(StatusCode::ACCEPTED.into_response());
    }
    state.jobs.remove(&id);
    state.jobs.delete_files(&id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn download_job(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let job = owned_job(&state, &auth, &id)?;
    if job.status != JobStatus::Succeeded {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "job_not_ready",
            format!("job is {:?}", job.status),
        ));
    }
    let path = state.jobs.output_path(&id, job.target_format.extension());
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("output file is no longer available"))?;
    let metadata = file.metadata().await.map_err(ApiError::from)?;
    let stream = ReaderStream::new(file);
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(job.target_format.mime())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&metadata.len().to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    let disposition = format!(
        "attachment; filename=\"{}\"",
        job.output_name.replace('"', "")
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).unwrap_or(HeaderValue::from_static("attachment")),
    );
    Ok(response)
}
