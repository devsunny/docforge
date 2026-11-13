use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use docforge_auth::{AuthConfig, AuthState};
use docforge_core::catalog::Catalog;
use docforge_core::engine::EngineRegistry;
use docforge_core::graph::ConversionGraph;
use docforge_core::service::ConversionService;
use docforge_mcp::protocol::RpcRequest;
use docforge_mcp::server::{handle_rpc, McpState};
use serde_json::json;
use std::sync::Arc;

fn state() -> McpState {
    let catalog = Arc::new(Catalog::load().unwrap());
    let registry = Arc::new(EngineRegistry::native_only());
    let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
    let service = Arc::new(ConversionService::new(
        catalog,
        graph,
        registry,
        std::env::temp_dir(),
        120,
    ));
    let auth = Arc::new(AuthState::new(AuthConfig::for_test(
        "http://127.0.0.1:18080",
    )));
    McpState::new(service, auth, "http://127.0.0.1:18080")
}

fn request(id: u64, method: &str, params: serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(id)),
        method: method.into(),
        params: Some(params),
    }
}

#[tokio::test]
async fn initialize_and_tools_list() {
    let state = state();
    let init = handle_rpc(
        &state,
        request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}
            }),
        ),
        None,
    )
    .await;
    let body = init.body.unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    let session = init.session_id.expect("initialize returns a session");
    assert_eq!(body["result"]["_meta"]["sessionId"], session);

    let tools = handle_rpc(&state, request(2, "tools/list", json!({})), Some(&session)).await;
    let tools_body = tools.body.unwrap();
    let names: Vec<&str> = tools_body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"list_formats"));
    assert!(names.contains(&"convert_document"));
    assert!(names.contains(&"plan_conversion"));
}

#[tokio::test]
async fn converts_text_to_pdf_inline() {
    let state = state();
    let result = handle_rpc(
        &state,
        request(
            3,
            "tools/call",
            json!({
                "name": "convert_document",
                "arguments": {
                    "filename": "hello.txt",
                    "source_format": "txt",
                    "target_format": "pdf",
                    "text": "Hello MCP"
                }
            }),
        ),
        None,
    )
    .await;
    let body = result.body.unwrap();
    assert_eq!(body["result"]["isError"], false);
    let content = body["result"]["content"].as_array().unwrap();
    let resource = content
        .iter()
        .find(|block| block["type"] == "resource")
        .expect("resource content");
    let blob = resource["resource"]["blob"].as_str().unwrap();
    let bytes = BASE64.decode(blob).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert_eq!(resource["resource"]["mimeType"], "application/pdf");
}

#[tokio::test]
async fn converts_outlook_msg_to_markdown_inline() {
    let msg = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/sample.msg"),
    )
    .unwrap();
    let state = state();
    let result = handle_rpc(
        &state,
        request(
            7,
            "tools/call",
            json!({
                "name": "convert_document",
                "arguments": {
                    "filename": "mail.msg",
                    "source_format": "msg",
                    "target_format": "md",
                    "content_base64": BASE64.encode(msg)
                }
            }),
        ),
        None,
    )
    .await;
    let body = result.body.unwrap();
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(body["result"]["structuredContent"]["target_format"], "md");
    let resource = body["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "resource")
        .unwrap();
    let markdown = BASE64
        .decode(resource["resource"]["blob"].as_str().unwrap())
        .unwrap();
    let markdown = String::from_utf8(markdown).unwrap();
    assert!(markdown.starts_with('#'), "markdown: {markdown}");
    assert!(markdown.contains("From:"));
}

#[tokio::test]
async fn lists_catalog_and_resources() {
    let state = state();
    let formats = handle_rpc(
        &state,
        request(
            4,
            "tools/call",
            json!({"name": "list_formats", "arguments": {}}),
        ),
        None,
    )
    .await;
    let structured = &formats.body.unwrap()["result"]["structuredContent"];
    assert!(structured["count"].as_u64().unwrap() >= 30);

    let conversions = handle_rpc(
        &state,
        request(
            5,
            "tools/call",
            json!({"name": "list_conversions", "arguments": {"limit": 10}}),
        ),
        None,
    )
    .await;
    assert_eq!(
        conversions.body.unwrap()["result"]["structuredContent"]["count"],
        10
    );

    let read = handle_rpc(
        &state,
        request(
            6,
            "resources/read",
            json!({"uri": "docforge://conversions/1"}),
        ),
        None,
    )
    .await;
    let text = read.body.unwrap()["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.contains("\"rank\": 1"));
}
