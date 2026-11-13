//! OAuth 2.1 authorization-server endpoints, RFC 9728 metadata, and DCR.

use crate::config::{supported_scopes, Resource};
use crate::error::AuthError;
use crate::jwt::Claims;
use crate::pkce;
use crate::store::{now, random_token, AuthState, NewClient};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

pub fn router(state: Arc<AuthState>) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(auth_server_metadata),
        )
        .route(
            "/.well-known/openid-configuration",
            get(auth_server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata_root),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(resource_metadata_mcp),
        )
        .route(
            "/.well-known/oauth-protected-resource/api",
            get(resource_metadata_api),
        )
        .route("/oauth/jwks", get(jwks))
        .route(
            "/oauth/authorize",
            get(authorize_page).post(authorize_submit),
        )
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .route("/oauth/register", post(register))
        .route("/oauth/register/{client_id}", get(registration))
        .with_state(state)
}

async fn auth_server_metadata(State(state): State<Arc<AuthState>>) -> impl IntoResponse {
    let mut response = Json(state.config.authorization_server_metadata()).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

async fn resource_metadata_root(State(state): State<Arc<AuthState>>) -> impl IntoResponse {
    resource_metadata_for(state, Resource::Mcp)
}

async fn resource_metadata_mcp(State(state): State<Arc<AuthState>>) -> impl IntoResponse {
    resource_metadata_for(state, Resource::Mcp)
}

async fn resource_metadata_api(State(state): State<Arc<AuthState>>) -> impl IntoResponse {
    resource_metadata_for(state, Resource::Api)
}

fn resource_metadata_for(state: Arc<AuthState>, resource: Resource) -> Response {
    let mut response = Json(state.config.protected_resource_metadata(resource)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

async fn jwks(State(state): State<Arc<AuthState>>) -> impl IntoResponse {
    let jwks = state.external_jwks().unwrap_or_else(|| state.keys.jwks());
    let mut response = Json(jwks).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

#[derive(Debug, Clone)]
struct AuthorizeRequest {
    client: crate::store::Client,
    redirect_uri: String,
    scope: Vec<String>,
    resource_url: String,
    code_challenge: String,
    state: Option<String>,
}

fn get_param(params: &HashMap<String, String>, name: &str) -> Option<String> {
    params
        .get(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_authorize(
    state: &AuthState,
    params: &HashMap<String, String>,
) -> Result<AuthorizeRequest, AuthError> {
    let response_type = get_param(params, "response_type").unwrap_or_default();
    if response_type != "code" {
        return Err(AuthError::UnsupportedResponseType);
    }
    let client_id = get_param(params, "client_id")
        .ok_or_else(|| AuthError::InvalidRequest("client_id is required".into()))?;
    let client = state
        .store
        .client(&client_id)
        .ok_or(AuthError::InvalidClient)?;
    if !client.allows_grant("authorization_code") {
        return Err(AuthError::UnauthorizedClient(
            "client is not allowed to use authorization_code".into(),
        ));
    }
    let redirect_uri = get_param(params, "redirect_uri")
        .ok_or_else(|| AuthError::InvalidRequest("redirect_uri is required".into()))?;
    if !client.redirect_uris.iter().any(|uri| uri == &redirect_uri) {
        return Err(AuthError::InvalidRequest(
            "redirect_uri is not registered for this client".into(),
        ));
    }

    let code_challenge = get_param(params, "code_challenge")
        .ok_or_else(|| AuthError::InvalidRequest("PKCE code_challenge is required".into()))?;
    let method = get_param(params, "code_challenge_method").unwrap_or_default();
    if method != "S256" {
        return Err(AuthError::InvalidRequest(
            "code_challenge_method must be S256 (PKCE is mandatory)".into(),
        ));
    }
    pkce::validate_challenge(&code_challenge)?;

    let resource_param = get_param(params, "resource");
    let raw_scope = get_param(params, "scope");
    let resource = if let Some(resource) = &resource_param {
        state.config.resource_from_uri(resource).ok_or_else(|| {
            AuthError::InvalidRequest(format!(
                "resource '{resource}' is not a DocForge protected resource"
            ))
        })?
    } else if raw_scope
        .as_deref()
        .is_some_and(|scope| scope.split_whitespace().any(|value| value == "mcp"))
    {
        Resource::Mcp
    } else {
        Resource::Api
    };
    let scope = raw_scope
        .map(|scope| {
            scope
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            let mut scopes = vec![resource.scope().to_string()];
            if resource == Resource::Mcp {
                scopes.push("offline_access".to_string());
            }
            scopes
        });
    if scope.is_empty() {
        return Err(AuthError::InvalidScope("scope must not be empty".into()));
    }
    let supported = supported_scopes();
    if let Some(unsupported) = scope
        .iter()
        .find(|scope| !supported.contains(&scope.as_str()))
    {
        return Err(AuthError::InvalidScope(format!(
            "unsupported scope '{unsupported}'"
        )));
    }
    let client_scopes: Vec<&str> = client.scope.split_whitespace().collect();
    if let Some(denied) = scope
        .iter()
        .find(|scope| !client_scopes.contains(&scope.as_str()))
    {
        return Err(AuthError::InvalidScope(format!(
            "scope '{denied}' is not registered for this client"
        )));
    }
    if !scope.iter().any(|value| value == resource.scope()) {
        return Err(AuthError::InvalidScope(format!(
            "scope '{}' is required for this resource",
            resource.scope()
        )));
    }

    Ok(AuthorizeRequest {
        client,
        redirect_uri,
        scope,
        resource_url: state.config.resource_url(resource).to_string(),
        code_challenge,
        state: get_param(params, "state"),
    })
}

async fn authorize_page(
    State(state): State<Arc<AuthState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if state.is_external() {
        return external_authorization_server();
    }
    match validate_authorize(&state, &params) {
        Ok(request) => login_page(&state, &request, None),
        Err(error) => authorize_failure(&state, &params, error),
    }
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    client_id: String,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
    code_challenge: String,
    code_challenge_method: String,
    resource: String,
    username: String,
    password: String,
}

async fn authorize_submit(
    State(state): State<Arc<AuthState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    if state.is_external() {
        return external_authorization_server();
    }
    let mut params = HashMap::new();
    params.insert("response_type".to_string(), "code".to_string());
    params.insert("client_id".to_string(), form.client_id);
    params.insert("redirect_uri".to_string(), form.redirect_uri);
    params.insert("scope".to_string(), form.scope);
    if let Some(value) = form.state {
        params.insert("state".to_string(), value);
    }
    params.insert("code_challenge".to_string(), form.code_challenge);
    params.insert(
        "code_challenge_method".to_string(),
        form.code_challenge_method,
    );
    params.insert("resource".to_string(), form.resource);

    let request = match validate_authorize(&state, &params) {
        Ok(request) => request,
        Err(error) => return authorize_failure(&state, &params, error),
    };
    if !state.check_password(&form.username, &form.password) {
        return login_page(&state, &request, Some("Invalid username or password."));
    }

    let scope = request.scope.join(" ");
    let code = state.store.create_code(
        &request.client.client_id,
        &request.redirect_uri,
        &request.code_challenge,
        &scope,
        &request.resource_url,
        &form.username,
        &form.username,
        state.config.authorization_code_ttl.as_secs(),
    );
    let mut url = match Url::parse(&request.redirect_uri) {
        Ok(url) => url,
        Err(_) => {
            return oauth_json_error(AuthError::ServerError(
                "registered redirect_uri is not a valid URL".into(),
            ))
        }
    };
    url.query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("iss", &state.config.issuer);
    if let Some(state_param) = &request.state {
        url.query_pairs_mut().append_pair("state", state_param);
    }
    Redirect::to(url.as_str()).into_response()
}

fn authorize_failure(
    state: &AuthState,
    params: &HashMap<String, String>,
    error: AuthError,
) -> Response {
    let client = get_param(params, "client_id").and_then(|id| state.store.client(&id));
    let redirect_uri = get_param(params, "redirect_uri");
    if let (Some(client), Some(redirect_uri)) = (client, redirect_uri) {
        if client.redirect_uris.iter().any(|uri| uri == &redirect_uri) {
            if let Ok(mut url) = Url::parse(&redirect_uri) {
                url.query_pairs_mut()
                    .append_pair("error", error.code())
                    .append_pair("error_description", &error.description())
                    .append_pair("iss", &state.config.issuer);
                if let Some(state_param) = get_param(params, "state") {
                    url.query_pairs_mut().append_pair("state", &state_param);
                }
                return Redirect::to(url.as_str()).into_response();
            }
        }
    }
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorization error</title></head><body><h1>Authorization error</h1><p><strong>{}</strong></p><p>{}</p></body></html>",
        html_escape(error.code()),
        html_escape(&error.description())
    );
    (error.status(), Html(body)).into_response()
}

fn login_page(state: &AuthState, request: &AuthorizeRequest, error: Option<&str>) -> Response {
    let scope_descriptions = request
        .scope
        .iter()
        .map(|scope| {
            let description = match scope.as_str() {
                "convert" => "Create conversions and jobs",
                "convert:read" => "Read conversion formats and jobs",
                "mcp" => "Use the DocForge MCP tools",
                "offline_access" => "Stay signed in with a refresh token",
                other => other,
            };
            format!(
                "<li><code>{}</code> — {}</li>",
                html_escape(scope),
                html_escape(description)
            )
        })
        .collect::<String>();
    let error_html = error
        .map(|message| format!("<div class=\"error\">{}</div>", html_escape(message)))
        .unwrap_or_default();
    let demo_hint = if state.config.demo_mode {
        format!(
            "<p class=\"hint\">Demo mode. Username <code>{}</code>; set <code>AUTH_PASSWORD</code> to change the password.</p>",
            html_escape(&state.config.username)
        )
    } else {
        String::new()
    };
    let state_value = request.state.as_deref().unwrap_or("");
    let body = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Authorize {client_name} · DocForge</title>
<style>
:root{{color-scheme:light dark;--bg:#f6f7f9;--card:#fff;--text:#1d232a;--muted:#5b6773;--line:#d9dfe6;--brand:#2f6fed;--danger:#b42318}}
@media(prefers-color-scheme:dark){{:root{{--bg:#11161d;--card:#18202a;--text:#e9eef5;--muted:#a5b2c1;--line:#2c3947;--brand:#6f9bff;--danger:#ff8b80}}}}
*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);font:15px/1.5 system-ui,-apple-system,Segoe UI,sans-serif;padding:24px}}
.card{{width:min(560px,100%);background:var(--card);border:1px solid var(--line);border-radius:14px;padding:28px;box-shadow:0 18px 50px rgba(15,23,42,.08)}}
.brand{{font-weight:700;letter-spacing:.02em;color:var(--brand);margin-bottom:20px}}h1{{font-size:1.4rem;margin:0 0 6px}}p{{color:var(--muted)}}.scopes{{background:var(--bg);border:1px solid var(--line);border-radius:10px;padding:12px 16px;margin:18px 0}}.scopes ul{{margin:8px 0 0;padding-left:20px}}code{{font-family:ui-monospace,Consolas,monospace;font-size:.92em}}
label{{display:block;margin:12px 0 5px;font-weight:600}}input[type=text],input[type=password]{{width:100%;padding:10px 12px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--text);font:inherit}}
button{{margin-top:18px;width:100%;padding:11px 16px;border:0;border-radius:8px;background:var(--brand);color:#fff;font:inherit;font-weight:650;cursor:pointer}}.error{{background:color-mix(in srgb,var(--danger) 12%,transparent);border:1px solid color-mix(in srgb,var(--danger) 40%,var(--line));color:var(--danger);padding:9px 12px;border-radius:8px;margin-bottom:10px}}.hint{{font-size:.86rem;margin-top:16px}}.meta{{margin-top:18px;font-size:.82rem;color:var(--muted)}}.meta code{{word-break:break-all}}
</style></head>
<body><main class="card">
<div class="brand">DocForge</div>
<h1>Authorize {client_name}</h1>
<p><strong>{client_name}</strong> is requesting access to your DocForge workspace.</p>
<div class="scopes"><strong>Requested permissions</strong><ul>{scope_descriptions}</ul></div>
{error_html}
<form method="post" action="/oauth/authorize">
<input type="hidden" name="client_id" value="{client_id}">
<input type="hidden" name="redirect_uri" value="{redirect_uri}">
<input type="hidden" name="scope" value="{scope}">
<input type="hidden" name="state" value="{state_value}">
<input type="hidden" name="code_challenge" value="{code_challenge}">
<input type="hidden" name="code_challenge_method" value="S256">
<input type="hidden" name="resource" value="{resource}">
<label for="username">Username</label>
<input id="username" name="username" type="text" autocomplete="username" required autofocus>
<label for="password">Password</label>
<input id="password" name="password" type="password" autocomplete="current-password" required>
<button type="submit">Sign in and authorize</button>
</form>
<p class="hint">Authorization code flow with PKCE (S256). You can revoke access at any time by rotating your credentials.</p>
{demo_hint}
<p class="meta">Resource: <code>{resource}</code><br>Redirect: <code>{redirect_uri}</code></p>
</main></body></html>"#,
        client_name = html_escape(&request.client.client_name),
        scope_descriptions = scope_descriptions,
        error_html = error_html,
        client_id = html_escape(&request.client.client_id),
        redirect_uri = html_escape(&request.redirect_uri),
        scope = html_escape(&request.scope.join(" ")),
        state_value = html_escape(state_value),
        code_challenge = html_escape(&request.code_challenge),
        resource = html_escape(&request.resource_url),
        demo_hint = demo_hint,
    );
    (StatusCode::OK, Html(body)).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct TokenForm {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    scope: String,
}

async fn token(
    State(state): State<Arc<AuthState>>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    if state.is_external() {
        return external_authorization_server();
    }
    match token_inner(&state, &headers, form).await {
        Ok((response, headers)) => {
            let mut response = Json(response).into_response();
            for (name, value) in headers {
                response.headers_mut().insert(name, value);
            }
            response
        }
        Err(error) => oauth_json_error(error),
    }
}

async fn token_inner(
    state: &AuthState,
    headers: &HeaderMap,
    form: TokenForm,
) -> Result<(TokenResponse, Vec<(header::HeaderName, HeaderValue)>), AuthError> {
    let (basic_id, basic_secret) = basic_credentials(headers)?;
    let client_id = basic_id
        .clone()
        .or_else(|| form.client_id.clone())
        .ok_or(AuthError::InvalidClient)?;
    let client_secret = basic_secret.as_deref().or(form.client_secret.as_deref());
    let client = state.store.authenticate_client(&client_id, client_secret)?;

    let grant_type = form.grant_type.as_str();
    if grant_type.is_empty() {
        return Err(AuthError::InvalidRequest("grant_type is required".into()));
    }
    match grant_type {
        "authorization_code" => {
            if !client.allows_grant("authorization_code") {
                return Err(AuthError::UnauthorizedClient(
                    "client may not use authorization_code".into(),
                ));
            }
            let code = form
                .code
                .as_deref()
                .ok_or_else(|| AuthError::InvalidRequest("code is required".into()))?;
            let redirect_uri = form
                .redirect_uri
                .as_deref()
                .ok_or_else(|| AuthError::InvalidRequest("redirect_uri is required".into()))?;
            let verifier = form
                .code_verifier
                .as_deref()
                .ok_or_else(|| AuthError::InvalidRequest("code_verifier is required".into()))?;
            let code_entry = state.store.take_code(
                code,
                &client.client_id,
                redirect_uri,
                form.resource.as_deref(),
            )?;
            pkce::verify_s256(verifier, &code_entry.code_challenge)?;
            let (access_token, expires_in) = issue_access(
                state,
                &code_entry.subject,
                &client.client_id,
                &code_entry.scope,
                &code_entry.resource,
                Some(&code_entry.username),
            )?;
            let refresh_token = if code_entry
                .scope
                .split_whitespace()
                .any(|scope| scope == "offline_access")
                || client.allows_grant("refresh_token")
            {
                Some(state.store.issue_refresh(
                    &client.client_id,
                    &code_entry.subject,
                    &code_entry.username,
                    &code_entry.scope,
                    &code_entry.resource,
                    state.config.refresh_token_ttl.as_secs(),
                ))
            } else {
                None
            };
            Ok((
                TokenResponse {
                    access_token,
                    token_type: "Bearer",
                    expires_in,
                    refresh_token,
                    scope: code_entry.scope,
                },
                no_store_headers(),
            ))
        }
        "refresh_token" => {
            if !client.allows_grant("refresh_token") {
                return Err(AuthError::UnauthorizedClient(
                    "client may not use refresh_token".into(),
                ));
            }
            let token = form
                .refresh_token
                .as_deref()
                .ok_or_else(|| AuthError::InvalidRequest("refresh_token is required".into()))?;
            let (entry, new_token) = state.store.rotate_refresh(
                token,
                &client.client_id,
                form.scope.as_deref(),
                state.config.refresh_token_ttl.as_secs(),
            )?;
            if let Some(resource) = &form.resource {
                if !crate::store::constant_time_eq(
                    resource.trim_end_matches('/'),
                    entry.resource.trim_end_matches('/'),
                ) {
                    return Err(AuthError::InvalidGrant(
                        "resource does not match the refresh token".into(),
                    ));
                }
            }
            let (access_token, expires_in) = issue_access(
                state,
                &entry.subject,
                &client.client_id,
                &entry.scope,
                &entry.resource,
                Some(&entry.username),
            )?;
            Ok((
                TokenResponse {
                    access_token,
                    token_type: "Bearer",
                    expires_in,
                    refresh_token: Some(new_token),
                    scope: entry.scope,
                },
                no_store_headers(),
            ))
        }
        "client_credentials" => {
            if client.is_public() {
                return Err(AuthError::UnauthorizedClient(
                    "public clients cannot use client_credentials".into(),
                ));
            }
            if !client.allows_grant("client_credentials") {
                return Err(AuthError::UnauthorizedClient(
                    "client may not use client_credentials".into(),
                ));
            }
            let requested = form
                .scope
                .as_deref()
                .unwrap_or(&client.scope)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let allowed: Vec<&str> = client.scope.split_whitespace().collect();
            if requested
                .iter()
                .any(|scope| !allowed.contains(&scope.as_str()))
            {
                return Err(AuthError::InvalidScope(
                    "requested scope exceeds client registration".into(),
                ));
            }
            let resource = form
                .resource
                .clone()
                .unwrap_or_else(|| state.config.rest_resource.clone());
            let resource = state
                .config
                .resource_from_uri(&resource)
                .ok_or_else(|| AuthError::InvalidRequest("unknown resource".into()))?;
            let scope = requested.join(" ");
            let (access_token, expires_in) = issue_access(
                state,
                &client.client_id,
                &client.client_id,
                &scope,
                state.config.resource_url(resource),
                None,
            )?;
            Ok((
                TokenResponse {
                    access_token,
                    token_type: "Bearer",
                    expires_in,
                    refresh_token: None,
                    scope,
                },
                no_store_headers(),
            ))
        }
        _ => Err(AuthError::UnsupportedGrantType),
    }
}

fn issue_access(
    state: &AuthState,
    subject: &str,
    client_id: &str,
    scope: &str,
    resource: &str,
    username: Option<&str>,
) -> Result<(String, u64), AuthError> {
    let now = now();
    let expires_in = state.config.access_token_ttl.as_secs();
    let claims = Claims {
        iss: state.config.issuer.clone(),
        sub: subject.to_string(),
        aud: resource.to_string(),
        client_id: client_id.to_string(),
        scope: scope.to_string(),
        exp: now + expires_in as i64,
        iat: now,
        jti: random_token(24),
        resource: Some(resource.to_string()),
        username: username.map(str::to_string),
    };
    let token = state.keys.sign(&claims)?;
    Ok((token, expires_in))
}

fn basic_credentials(headers: &HeaderMap) -> Result<(Option<String>, Option<String>), AuthError> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Ok((None, None));
    };
    let value = value.to_str().map_err(|_| AuthError::InvalidClient)?;
    let Some(encoded) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    else {
        return Ok((None, None));
    };
    let decoded = BASE64
        .decode(encoded.trim())
        .map_err(|_| AuthError::InvalidClient)?;
    let decoded = String::from_utf8(decoded).map_err(|_| AuthError::InvalidClient)?;
    let (id, secret) = decoded.split_once(':').ok_or(AuthError::InvalidClient)?;
    Ok((Some(percent_decode(id)), Some(percent_decode(secret))))
}

fn percent_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

fn no_store_headers() -> Vec<(header::HeaderName, HeaderValue)> {
    vec![
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (header::PRAGMA, HeaderValue::from_static("no-cache")),
    ]
}

fn external_authorization_server() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "not_found",
            "error_description": "DocForge is configured with an external authorization server (Keycloak); use its authorize/token/registration endpoints."
        })),
    )
        .into_response()
}

fn oauth_json_error(error: AuthError) -> Response {
    let mut response = (error.status(), Json(error.oauth_json())).into_response();
    if error.status() == StatusCode::UNAUTHORIZED {
        // RFC 6749 says invalid_client challenges use the Basic scheme.
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"docforge\""),
        );
    }
    response
}

#[derive(Debug, Deserialize)]
struct RevokeForm {
    token: String,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn revoke(
    State(state): State<Arc<AuthState>>,
    headers: HeaderMap,
    Form(form): Form<RevokeForm>,
) -> Response {
    if state.is_external() {
        return external_authorization_server();
    }
    let (basic_id, basic_secret) = match basic_credentials(&headers) {
        Ok(value) => value,
        Err(error) => return oauth_json_error(error),
    };
    let client_id = basic_id.or(form.client_id);
    if let Some(client_id) = client_id {
        let secret = basic_secret.as_deref().or(form.client_secret.as_deref());
        if state.store.authenticate_client(&client_id, secret).is_err() {
            return oauth_json_error(AuthError::InvalidClient);
        }
    }
    let claims_jti = state
        .keys
        .verify_signature_only(&form.token)
        .ok()
        .map(|claims| claims.jti);
    state.store.revoke(&form.token, claims_jti.as_deref());
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// Dynamic Client Registration (RFC 7591)
// ---------------------------------------------------------------------------

async fn register(
    State(state): State<Arc<AuthState>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.is_external() {
        return external_authorization_server();
    }
    if !state.config.dcr_enabled {
        return oauth_json_error(AuthError::AccessDenied(
            "dynamic client registration is disabled".into(),
        ));
    }
    if let Some(expected) = &state.config.dcr_initial_access_token {
        let provided = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if !provided.is_some_and(|value| crate::store::constant_time_eq(value, expected)) {
            return oauth_json_error(AuthError::AccessDenied(
                "a valid initial access token is required".into(),
            ));
        }
    }
    match register_inner(&state, &body) {
        Ok(registered) => {
            let client_id = registered.client.client_id.clone();
            let mut response = (
                StatusCode::CREATED,
                Json(registration_response(&state, registered)),
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            let _ = client_id;
            response
        }
        Err(error) => oauth_json_error(error),
    }
}

fn register_inner(
    state: &AuthState,
    body: &serde_json::Value,
) -> Result<crate::store::RegisteredClient, AuthError> {
    let redirect_uris = body
        .get("redirect_uris")
        .and_then(|value| value.as_array())
        .ok_or_else(|| AuthError::InvalidRequest("redirect_uris must be an array".into()))?
        .iter()
        .filter_map(|value| value.as_str())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if redirect_uris.is_empty() || redirect_uris.len() > 10 {
        return Err(AuthError::InvalidRequest(
            "redirect_uris must contain 1..=10 entries".into(),
        ));
    }
    for uri in &redirect_uris {
        validate_redirect_uri(uri)?;
    }
    let mut grant_types = body
        .get("grant_types")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ]
        });
    if grant_types.is_empty() {
        return Err(AuthError::InvalidRequest(
            "grant_types must not be empty".into(),
        ));
    }
    for grant in &grant_types {
        if !matches!(
            grant.as_str(),
            "authorization_code" | "refresh_token" | "client_credentials"
        ) {
            return Err(AuthError::InvalidRequest(format!(
                "unsupported grant_type '{grant}'"
            )));
        }
    }
    if !grant_types
        .iter()
        .any(|grant| grant == "authorization_code")
    {
        return Err(AuthError::InvalidRequest(
            "authorization_code grant is required for PKCE clients".into(),
        ));
    }
    let response_types = body
        .get("response_types")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["code".to_string()]);
    if response_types.iter().any(|value| value != "code") {
        return Err(AuthError::InvalidRequest(
            "only response_type=code is supported".into(),
        ));
    }
    let auth_method = body
        .get("token_endpoint_auth_method")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    if !matches!(
        auth_method,
        "none" | "client_secret_basic" | "client_secret_post"
    ) {
        return Err(AuthError::InvalidRequest(
            "token_endpoint_auth_method must be none, client_secret_basic, or client_secret_post"
                .into(),
        ));
    }
    let scope = body
        .get("scope")
        .and_then(|value| value.as_str())
        .unwrap_or("convert mcp offline_access")
        .to_string();
    let supported = supported_scopes();
    for scope in scope.split_whitespace() {
        if !supported.contains(&scope) {
            return Err(AuthError::InvalidScope(format!(
                "unsupported scope '{scope}'"
            )));
        }
    }
    if grant_types
        .iter()
        .any(|grant| grant == "client_credentials")
        && auth_method == "none"
    {
        return Err(AuthError::InvalidRequest(
            "client_credentials requires a confidential client".into(),
        ));
    }
    grant_types.sort();
    grant_types.dedup();

    let mut metadata = serde_json::Map::new();
    for key in [
        "client_uri",
        "logo_uri",
        "contacts",
        "tos_uri",
        "policy_uri",
        "software_id",
        "software_version",
        "application_type",
    ] {
        if let Some(value) = body.get(key) {
            metadata.insert(key.to_string(), value.clone());
        }
    }
    let client_name = body
        .get("client_name")
        .and_then(|value| value.as_str())
        .unwrap_or("DocForge client")
        .chars()
        .take(120)
        .collect::<String>();

    state.store.register_client(NewClient {
        client_name,
        redirect_uris,
        grant_types,
        response_types,
        token_endpoint_auth_method: auth_method.to_string(),
        scope,
        metadata: serde_json::Value::Object(metadata),
    })
}

fn registration_response(
    state: &AuthState,
    registered: crate::store::RegisteredClient,
) -> serde_json::Value {
    let mut value = registered.client.public_json();
    if let Some(secret) = &registered.client_secret {
        value["client_secret"] = serde_json::Value::String(secret.clone());
        value["client_secret_expires_at"] = serde_json::json!(0);
    }
    if let Some(token) = &registered.registration_access_token {
        value["registration_access_token"] = serde_json::Value::String(token.clone());
    }
    value["registration_client_uri"] = serde_json::Value::String(format!(
        "{}/oauth/register/{}",
        state.config.issuer, registered.client.client_id
    ));
    value
}

fn validate_redirect_uri(value: &str) -> Result<(), AuthError> {
    if value.len() > 2048 {
        return Err(AuthError::InvalidRequest("redirect_uri is too long".into()));
    }
    let url = Url::parse(value)
        .map_err(|_| AuthError::InvalidRequest(format!("invalid redirect_uri '{value}'")))?;
    if url.fragment().is_some() {
        return Err(AuthError::InvalidRequest(
            "redirect_uri must not contain a fragment".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AuthError::InvalidRequest(
            "redirect_uri must not contain user information".into(),
        ));
    }
    match url.scheme() {
        "https" => {
            if url.host_str().is_none() {
                return Err(AuthError::InvalidRequest(
                    "https redirect_uri must have a host".into(),
                ));
            }
        }
        "http" => {
            let host = url.host_str().unwrap_or("");
            if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
                return Err(AuthError::InvalidRequest(
                    "http redirect_uri is only allowed for loopback hosts".into(),
                ));
            }
        }
        scheme
            if matches!(
                scheme,
                "javascript" | "data" | "file" | "vbscript" | "about"
            ) =>
        {
            return Err(AuthError::InvalidRequest(format!(
                "unsafe redirect_uri scheme '{scheme}'"
            )));
        }
        "" => {
            return Err(AuthError::InvalidRequest(
                "redirect_uri must be absolute".into(),
            ));
        }
        _ => {
            // Custom scheme for native apps; require a non-empty path or host.
            if url.path().is_empty() && url.host_str().is_none() {
                return Err(AuthError::InvalidRequest(
                    "native redirect_uri must include a path or host".into(),
                ));
            }
        }
    }
    Ok(())
}

async fn registration(
    State(state): State<Arc<AuthState>>,
    Path(client_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if state.is_external() {
        return external_authorization_server();
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token else {
        return oauth_json_error(AuthError::InvalidToken(
            "registration access token is required".into(),
        ));
    };
    if !state.store.registration_token_valid(&client_id, token) {
        return oauth_json_error(AuthError::InvalidToken(
            "registration access token is invalid".into(),
        ));
    }
    let Some(client) = state.store.client(&client_id) else {
        return oauth_json_error(AuthError::InvalidToken("unknown client".into()));
    };
    let mut value = client.public_json();
    value["registration_client_uri"] = serde_json::Value::String(format!(
        "{}/oauth/register/{}",
        state.config.issuer, client.client_id
    ));
    Json(value).into_response()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::scopes_for;
    use crate::store::hash_token;

    #[test]
    fn validates_loopback_redirects() {
        validate_redirect_uri("http://127.0.0.1:8765/callback").unwrap();
        validate_redirect_uri("https://example.com/cb").unwrap();
        validate_redirect_uri("myapp://callback").unwrap();
        assert!(validate_redirect_uri("http://evil.example/cb").is_err());
        assert!(validate_redirect_uri("https://example.com/cb#frag").is_err());
        assert!(validate_redirect_uri("javascript:alert(1)").is_err());
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(hash_token("abc"), hash_token("abc"));
        assert_ne!(hash_token("abc"), hash_token("abd"));
        assert_eq!(scopes_for(Resource::Mcp), vec!["mcp"]);
        assert!(now() > 0);
    }
}
