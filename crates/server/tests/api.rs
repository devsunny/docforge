use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use docforge_auth::{AuthConfig, Claims};
use docforge_server::config::AppConfig;
use docforge_server::{app, build_state, AppState};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

async fn test_state() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig::from_env().unwrap();
    config.data_dir = dir.path().to_path_buf();
    config.public_base_url = "http://127.0.0.1:18080".into();
    config.auth = AuthConfig::for_test("http://127.0.0.1:18080");
    config.auth.rest_resource = "http://127.0.0.1:18080/api".into();
    config.auth.mcp_resource = "http://127.0.0.1:18080/mcp".into();
    config.ensure_dirs().unwrap();
    (Arc::new(build_state(config).await.unwrap()), dir)
}

fn token(state: &AppState, audience: &str, scope: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        iss: state.auth.config.issuer.clone(),
        sub: "tester".into(),
        aud: audience.into(),
        client_id: "test-client".into(),
        scope: scope.into(),
        exp: now + 600,
        iat: now,
        jti: format!("test-{}", uuid::Uuid::new_v4()),
        resource: Some(audience.into()),
        username: Some("demo".into()),
    };
    state.auth.keys.sign(&claims).unwrap()
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn discovery_is_public_and_api_requires_oauth() {
    let (state, _dir) = test_state().await;
    let router = app((*state).clone());

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

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/formats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let challenge = unauthorized
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(challenge.contains(
        "resource_metadata=\"http://127.0.0.1:18080/.well-known/oauth-protected-resource/api\""
    ));

    let access = token(&state, "http://127.0.0.1:18080/api", "convert convert:read");
    let formats = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/formats")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(formats.status(), StatusCode::OK);
    let formats = json(formats).await;
    assert!(formats["count"].as_u64().unwrap() > 30);
}

#[tokio::test]
async fn plan_and_catalog_endpoints() {
    let (state, _dir) = test_state().await;
    let router = app((*state).clone());
    let access = token(&state, "http://127.0.0.1:18080/api", "convert convert:read");

    let conversions = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/conversions?limit=100")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conversions.status(), StatusCode::OK);
    let conversions = json(conversions).await;
    assert_eq!(conversions["total"], 100);

    let plan = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/conversions/plan")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"source_format":"txt","target_format":"pdf"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan.status(), StatusCode::OK);
    let plan = json(plan).await;
    assert!(!plan["plan"]["steps"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn mcp_requires_mcp_scope_and_initializes() {
    let (state, _dir) = test_state().await;
    let router = app((*state).clone());
    let init_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}
        }
    })
    .to_string();

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(init_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert!(unauthorized
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("/.well-known/oauth-protected-resource/mcp"));

    let api_token = token(&state, "http://127.0.0.1:18080/api", "convert");
    let wrong_scope = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {api_token}"))
                .body(Body::from(init_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_scope.status(), StatusCode::UNAUTHORIZED);

    let mcp_token = token(&state, "http://127.0.0.1:18080/mcp", "mcp");
    let init = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {mcp_token}"))
                .body(Body::from(init_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(init.status(), StatusCode::OK);
    assert!(init.headers().get("mcp-session-id").is_some());
    let body = json(init).await;
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn openapi_is_served() {
    let (state, _dir) = test_state().await;
    let router = app((*state).clone());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["openapi"], "3.1.0");
}
