use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use docforge_auth::config::Resource;
use docforge_auth::pkce;
use docforge_auth::{AuthConfig, AuthState};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn app() -> (Arc<AuthState>, axum::Router) {
    let config = AuthConfig::for_test("http://127.0.0.1:18080");
    let state = Arc::new(AuthState::new(config));
    let router = docforge_auth::oauth_router(state.clone());
    (state, router)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn register(router: &axum::Router, redirect: &str) -> Value {
    let payload = serde_json::json!({
        "client_name": "Flow test",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "scope": "convert mcp offline_access"
    });
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await
}

#[tokio::test]
async fn discovery_and_full_pkce_flow() {
    let (state, router) = app();

    let metadata = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-authorization-server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);
    let meta = body_json(metadata).await;
    assert_eq!(meta["issuer"], "http://127.0.0.1:18080");
    assert_eq!(meta["code_challenge_methods_supported"][0], "S256");
    assert!(meta["registration_endpoint"].is_string());

    let protected = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-protected-resource/mcp")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let protected = body_json(protected).await;
    assert_eq!(protected["resource"], "http://127.0.0.1:18080/mcp");
    assert_eq!(
        protected["authorization_servers"][0],
        "http://127.0.0.1:18080"
    );

    let redirect = "http://127.0.0.1:9876/callback";
    let client = register(&router, redirect).await;
    let client_id = client["client_id"].as_str().unwrap().to_string();
    assert!(client_id.starts_with("fb_"));

    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = pkce::challenge_s256(verifier);
    let authorize_uri = format!(
        "/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri={}&scope=convert%20offline_access&state=xyz&code_challenge={challenge}&code_challenge_method=S256&resource={}",
        urlencoding(redirect),
        urlencoding("http://127.0.0.1:18080/api")
    );
    let login = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&authorize_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let login_html = String::from_utf8(
        login
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(login_html.contains("Sign in and authorize"));

    let form = [
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect),
        ("scope", "convert offline_access"),
        ("state", "xyz"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("resource", "http://127.0.0.1:18080/api"),
        ("username", "demo"),
        ("password", "demo"),
    ]
    .iter()
    .map(|(key, value)| format!("{}={}", key, urlencoding(value)))
    .collect::<Vec<_>>()
    .join("&");

    let authorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::SEE_OTHER);
    let location = authorized
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let parsed = url::Url::parse(&location).unwrap();
    let code = parsed
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .to_string();
    assert_eq!(
        parsed
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1,
        "xyz"
    );

    let token_form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier),
        ("resource", "http://127.0.0.1:18080/api"),
    ]
    .iter()
    .map(|(key, value)| format!("{}={}", key, urlencoding(value)))
    .collect::<Vec<_>>()
    .join("&");

    let token_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(token_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(token_response.status(), StatusCode::OK);
    assert_eq!(
        token_response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let token = body_json(token_response).await;
    let access_token = token["access_token"].as_str().unwrap();
    let claims = state
        .keys
        .verify(access_token, "http://127.0.0.1:18080/api")
        .unwrap();
    assert_eq!(claims.sub, "demo");
    assert!(claims.scopes().contains(&"convert".to_string()));
    assert_eq!(claims.aud, "http://127.0.0.1:18080/api");
    let refresh = token["refresh_token"].as_str().unwrap();

    let refresh_form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id.as_str()),
    ]
    .iter()
    .map(|(key, value)| format!("{}={}", key, urlencoding(value)))
    .collect::<Vec<_>>()
    .join("&");
    let refreshed = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(refresh_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.status(), StatusCode::OK);
    let refreshed = body_json(refreshed).await;
    assert!(refreshed["access_token"].is_string());
    assert_ne!(refreshed["refresh_token"].as_str().unwrap(), refresh);
}

#[tokio::test]
async fn pkce_mismatch_and_bad_redirect_are_rejected() {
    let (_, router) = app();
    let client = register(&router, "http://127.0.0.1:9876/callback").await;
    let client_id = client["client_id"].as_str().unwrap();

    let bad = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "redirect_uris": ["http://evil.example/callback"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let challenge = pkce::challenge_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    let form = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", "http://127.0.0.1:9876/callback"),
        ("scope", "convert"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("resource", "http://127.0.0.1:18080/api"),
        ("username", "demo"),
        ("password", "demo"),
    ]
    .iter()
    .map(|(key, value)| format!("{}={}", key, urlencoding(value)))
    .collect::<Vec<_>>()
    .join("&");
    let authorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    let location = authorized
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let code = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .to_string();

    let token_form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", "http://127.0.0.1:9876/callback"),
        ("client_id", client_id),
        (
            "code_verifier",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        ("resource", "http://127.0.0.1:18080/api"),
    ]
    .iter()
    .map(|(key, value)| format!("{}={}", key, urlencoding(value)))
    .collect::<Vec<_>>()
    .join("&");
    let denied = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(token_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    let body = body_json(denied).await;
    assert_eq!(body["error"], "invalid_grant");
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[test]
fn resource_metadata_scope_matches_resource() {
    let config = AuthConfig::for_test("http://127.0.0.1:18080");
    let api = config.protected_resource_metadata(Resource::Api);
    assert!(api["scopes_supported"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("convert")));
}
