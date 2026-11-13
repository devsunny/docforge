//! MCP request handling and tools.

use crate::protocol::{
    code, tool_error, tool_success, RpcRequest, RpcResponse, LATEST_PROTOCOL, SUPPORTED_PROTOCOLS,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use docforge_auth::AuthState;
use docforge_core::format::Format;
use docforge_core::options::ConversionOptions;
use docforge_core::service::{convert_bytes, output_filename, ConversionService};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub protocol: String,
    pub initialized: bool,
    pub created_at: i64,
}

pub struct McpState {
    pub service: Arc<ConversionService>,
    pub auth: Arc<AuthState>,
    pub base_url: String,
    pub allowed_origins: Vec<String>,
    pub max_inline_bytes: usize,
    pub sessions: RwLock<HashMap<String, Session>>,
}

impl McpState {
    pub fn new(
        service: Arc<ConversionService>,
        auth: Arc<AuthState>,
        base_url: impl Into<String>,
    ) -> Self {
        let allowed_origins = auth.config.allowed_origins.clone();
        Self {
            service,
            auth,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            allowed_origins,
            max_inline_bytes: 16 * 1024 * 1024,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn prune_sessions(&self) {
        let cutoff = now() - 3600;
        self.sessions
            .write()
            .retain(|_, session| session.created_at >= cutoff);
    }

    pub fn origin_allowed(&self, origin: &str) -> bool {
        if self.allowed_origins.iter().any(|allowed| allowed == origin) {
            return true;
        }
        if let (Ok(origin), Ok(base)) = (url::Url::parse(origin), url::Url::parse(&self.base_url)) {
            if origin.origin() == base.origin() {
                return true;
            }
            if matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
                return true;
            }
        }
        false
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub struct RpcOutcome {
    pub body: Option<Value>,
    pub session_id: Option<String>,
    pub protocol: String,
    pub notification: bool,
}

pub async fn handle_rpc(
    state: &McpState,
    request: RpcRequest,
    session_id: Option<&str>,
) -> RpcOutcome {
    let protocol = session_id
        .and_then(|id| {
            state
                .sessions
                .read()
                .get(id)
                .map(|session| session.protocol.clone())
        })
        .unwrap_or_else(|| LATEST_PROTOCOL.to_string());
    let notification = request.id.is_none();
    let id = request.id.clone();

    if request.jsonrpc != "2.0" {
        return RpcOutcome {
            body: Some(
                serde_json::to_value(RpcResponse::error(
                    id,
                    code::INVALID_REQUEST,
                    "jsonrpc must be \"2.0\"",
                    None,
                ))
                .unwrap(),
            ),
            session_id: None,
            protocol,
            notification: false,
        };
    }

    if notification {
        if request.method == "notifications/initialized" {
            if let Some(session_id) = session_id {
                if let Some(session) = state.sessions.write().get_mut(session_id) {
                    session.initialized = true;
                }
            }
        }
        return RpcOutcome {
            body: None,
            session_id: None,
            protocol,
            notification: true,
        };
    }

    let mut new_session = None;
    let response = match request.method.as_str() {
        "initialize" => {
            let response = initialize(state, id, request.params);
            new_session = response
                .result
                .as_ref()
                .and_then(|result| result.pointer("/_meta/sessionId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            response
        }
        "ping" => RpcResponse::success(id, json!({})),
        "tools/list" => RpcResponse::success(id, json!({"tools": tools()})),
        "tools/call" => match call_tool(state, request.params).await {
            Ok(result) => RpcResponse::success(id, result),
            Err((code_value, message, data)) => RpcResponse::error(id, code_value, message, data),
        },
        "resources/list" => RpcResponse::success(
            id,
            json!({
                "resources": [
                    {"uri": "docforge://formats", "name": "Formats", "description": "All canonical formats and their catalog targets", "mimeType": "application/json"},
                    {"uri": "docforge://conversions", "name": "Conversion catalog", "description": "The 100 ranked conversions with availability", "mimeType": "application/json"},
                    {"uri": "docforge://engines", "name": "Conversion engines", "description": "Installed and optional conversion engines", "mimeType": "application/json"},
                    {"uri": "docforge://server", "name": "Server information", "description": "Version, limits, and discovery URLs", "mimeType": "application/json"}
                ]
            }),
        ),
        "resources/templates/list" => RpcResponse::success(
            id,
            json!({
                "resourceTemplates": [
                    {"uriTemplate": "docforge://conversions/{rank}", "name": "Conversion by rank", "mimeType": "application/json"},
                    {"uriTemplate": "docforge://formats/{format}", "name": "Format details", "mimeType": "application/json"}
                ]
            }),
        ),
        "resources/read" => match read_resource(state, request.params) {
            Ok(result) => RpcResponse::success(id, result),
            Err((code_value, message)) => RpcResponse::error(id, code_value, message, None),
        },
        "prompts/list" => RpcResponse::success(
            id,
            json!({
                "prompts": [
                    {
                        "name": "convert_document",
                        "description": "Draft a request to convert a document with DocForge",
                        "arguments": [
                            {"name": "filename", "description": "Input file name", "required": true},
                            {"name": "target_format", "description": "Target format ID (pdf, docx, md, ...)", "required": true},
                            {"name": "source_format", "description": "Optional source format hint", "required": false}
                        ]
                    },
                    {
                        "name": "choose_format",
                        "description": "Explain the best target format for a use case",
                        "arguments": [
                            {"name": "use_case", "description": "What you want to do", "required": true}
                        ]
                    }
                ]
            }),
        ),
        "prompts/get" => prompts_get(id, request.params),
        "logging/setLevel" => RpcResponse::success(id, json!({})),
        "completion/complete" => RpcResponse::success(
            id,
            json!({"completion": {"values": [], "total": 0, "hasMore": false}}),
        ),
        other => RpcResponse::error(
            id,
            code::METHOD_NOT_FOUND,
            format!("method '{other}' is not supported"),
            None,
        ),
    };

    RpcOutcome {
        body: Some(serde_json::to_value(response).unwrap()),
        session_id: new_session,
        protocol,
        notification: false,
    }
}

fn initialize(state: &McpState, id: Option<Value>, params: Option<Value>) -> RpcResponse {
    let requested = params
        .as_ref()
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(LATEST_PROTOCOL);
    let protocol = if SUPPORTED_PROTOCOLS.contains(&requested) {
        requested
    } else {
        LATEST_PROTOCOL
    };
    let session_id = Uuid::new_v4().to_string();
    state.sessions.write().insert(
        session_id.clone(),
        Session {
            id: session_id.clone(),
            protocol: protocol.to_string(),
            initialized: false,
            created_at: now(),
        },
    );
    let result = json!({
        "protocolVersion": protocol,
        "capabilities": {
            "tools": {"listChanged": false},
            "resources": {"subscribe": false, "listChanged": false},
            "prompts": {"listChanged": false},
            "logging": {}
        },
        "serverInfo": {
            "name": "docforge",
            "title": "DocForge Document Converter",
            "version": env!("CARGO_PKG_VERSION"),
            "websiteUrl": state.base_url
        },
        "instructions": "DocForge converts documents between PDF, Office, Markdown, HTML, spreadsheets, images, e-books, structured data, and Outlook MSG. Use tools/list, then call convert_document with content_base64 (or text). Errors and availability are returned per tool. HTTP MCP requires the `mcp` OAuth scope; discover the authorization server and its registration endpoint from /.well-known/oauth-protected-resource/mcp."
    });
    let mut response = RpcResponse::success(id, result);
    if let Some(result) = response.result.as_mut() {
        result["_meta"] = json!({"sessionId": session_id});
    }
    response
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "list_formats",
            "title": "List formats",
            "description": "List every canonical format, its MIME type, extensions, and catalog targets.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        }),
        json!({
            "name": "list_conversions",
            "title": "List conversions",
            "description": "List the 100 ranked conversions with category, use case, and live availability.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "category": {"type": "string", "description": "Filter by exact category name"},
                    "available_only": {"type": "boolean", "default": false},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 100}
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "plan_conversion",
            "title": "Plan a conversion",
            "description": "Show the shortest engine route from a source format to a target format, including multi-hop fallbacks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_format": {"type": "string"},
                    "target_format": {"type": "string"},
                    "options": {"type": "object", "description": "ConversionOptions overrides"}
                },
                "required": ["source_format", "target_format"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "convert_document",
            "title": "Convert a document",
            "description": "Convert base64 or plain-text content and return the result inline as a base64 resource. Files larger than 16 MiB are rejected; use the REST job API for larger inputs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filename": {"type": "string", "description": "Input file name including extension"},
                    "source_format": {"type": "string", "description": "Optional source format; detected when omitted"},
                    "target_format": {"type": "string", "description": "Target format ID (pdf, docx, md, png, ...)"},
                    "content_base64": {"type": "string", "description": "Base64 file bytes; mutually exclusive with text"},
                    "text": {"type": "string", "description": "Plain text input; mutually exclusive with content_base64"},
                    "options": {"type": "object", "description": "ConversionOptions overrides"}
                },
                "required": ["filename", "target_format"],
                "anyOf": [{"required": ["content_base64"]}, {"required": ["text"]}],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "server_info",
            "title": "Server information",
            "description": "Return service version, limits, and OAuth discovery URLs.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        }),
    ]
}

async fn call_tool(
    state: &McpState,
    params: Option<Value>,
) -> Result<Value, (i64, String, Option<Value>)> {
    let params = params.ok_or((code::INVALID_PARAMS, "params are required".into(), None))?;
    let name = params.get("name").and_then(Value::as_str).ok_or((
        code::INVALID_PARAMS,
        "tool name is required".into(),
        None,
    ))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match name {
        "list_formats" => Ok(list_formats(state)),
        "list_conversions" => Ok(list_conversions(state, &args)),
        "plan_conversion" => Ok(plan_conversion(state, &args)),
        "convert_document" => Ok(convert_document(state, &args).await),
        "server_info" => Ok(server_info(state)),
        other => Err((
            code::INVALID_PARAMS,
            format!("unknown tool '{other}'"),
            Some(json!({"tool": other})),
        )),
    }
}

fn list_formats(state: &McpState) -> Value {
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
    tool_success(
        format!("{} formats are available.", formats.len()),
        json!({"count": formats.len(), "formats": formats}),
        vec![],
    )
}

fn list_conversions(state: &McpState, args: &Value) -> Value {
    let category = args.get("category").and_then(Value::as_str);
    let available_only = args
        .get("available_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 100) as usize;
    let mut conversions = Vec::new();
    for entry in state.service.catalog().entries() {
        if let Some(category) = category {
            if !entry.category.eq_ignore_ascii_case(category) {
                continue;
            }
        }
        let available = state.service.graph().is_catalog_available(entry.rank);
        if available_only && !available {
            continue;
        }
        let route = state
            .service
            .graph()
            .catalog_plan(entry)
            .ok()
            .map(|plan| plan.summary());
        conversions.push(json!({
            "rank": entry.rank,
            "source": entry.source_format,
            "target": entry.target_format,
            "category": entry.category,
            "use_case": entry.use_case,
            "requires_ocr": entry.requires_ocr,
            "searchable": entry.searchable,
            "available": available,
            "route": route,
        }));
        if conversions.len() >= limit {
            break;
        }
    }
    let count = conversions.len();
    tool_success(
        format!("{count} conversion(s) returned."),
        json!({"count": count, "conversions": conversions}),
        vec![],
    )
}

fn plan_conversion(state: &McpState, args: &Value) -> Value {
    let source = args
        .get("source_format")
        .and_then(Value::as_str)
        .and_then(|value| Format::parse(value).ok());
    let target = args
        .get("target_format")
        .and_then(Value::as_str)
        .and_then(|value| Format::parse(value).ok());
    let (Some(source), Some(target)) = (source, target) else {
        return tool_error("source_format and target_format must be valid format IDs");
    };
    let options: ConversionOptions = args
        .get("options")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    match state.service.plan(source, target, &options) {
        Ok(plan) => tool_success(
            format!(
                "{} -> {}: {}",
                source.label(),
                target.label(),
                plan.summary()
            ),
            serde_json::to_value(&plan).unwrap_or_default(),
            vec![],
        ),
        Err(error) => tool_error(error.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct ConvertArgs {
    filename: String,
    target_format: String,
    #[serde(default)]
    source_format: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    options: ConversionOptions,
}

async fn convert_document(state: &McpState, args: &Value) -> Value {
    let args: ConvertArgs = match serde_json::from_value(args.clone()) {
        Ok(args) => args,
        Err(error) => return tool_error(format!("invalid arguments: {error}")),
    };
    let target = match Format::parse(&args.target_format) {
        Ok(target) => target,
        Err(error) => return tool_error(error.to_string()),
    };
    let source = match args.source_format.as_deref() {
        Some(value) => match Format::parse(value) {
            Ok(source) => Some(source),
            Err(error) => return tool_error(error.to_string()),
        },
        None => None,
    };
    let bytes = match (args.content_base64, args.text) {
        (Some(_), Some(_)) => return tool_error("content_base64 and text are mutually exclusive"),
        (Some(encoded), None) => match BASE64.decode(encoded) {
            Ok(bytes) => bytes,
            Err(error) => return tool_error(format!("content_base64 is invalid: {error}")),
        },
        (None, Some(text)) => text.into_bytes(),
        (None, None) => return tool_error("content_base64 or text is required"),
    };
    if bytes.len() > state.max_inline_bytes {
        return tool_error(format!(
            "input is {} bytes; the MCP inline limit is {} bytes. Use the REST job API for larger files.",
            bytes.len(),
            state.max_inline_bytes
        ));
    }
    if let Err(error) = args.options.validate() {
        return tool_error(format!("invalid options: {error}"));
    }
    let result = convert_bytes(
        &state.service,
        &bytes,
        &args.filename,
        source,
        target,
        &args.options,
    )
    .await;
    let (output, outcome, detected) = match result {
        Ok(value) => value,
        Err(error) => return tool_error(error.to_string()),
    };
    if output.len() > state.max_inline_bytes {
        return tool_error(format!(
            "output is {} bytes; the MCP inline limit is {} bytes. Use the REST job API to download large results.",
            output.len(),
            state.max_inline_bytes
        ));
    }
    let filename = output_filename(&args.filename, target);
    let uri = format!(
        "docforge://conversions/{}/{}",
        Uuid::new_v4(),
        urlencoding(&filename)
    );
    let summary = format!(
        "Converted {} ({} bytes) to {} ({} bytes). Route: {}",
        detected.label(),
        bytes.len(),
        target.label(),
        output.len(),
        outcome.plan.summary()
    );
    let structured = json!({
        "filename": filename,
        "uri": uri,
        "source_format": outcome.source_format.id(),
        "target_format": outcome.target_format.id(),
        "input_bytes": bytes.len(),
        "output_bytes": outcome.output_bytes,
        "duration_ms": outcome.duration_ms,
        "mime_type": target.mime(),
        "route": outcome.plan.summary(),
        "plan": outcome.plan,
    });
    let resource = json!({
        "type": "resource",
        "resource": {
            "uri": uri,
            "mimeType": target.mime(),
            "blob": BASE64.encode(output)
        }
    });
    tool_success(summary, structured, vec![resource])
}

fn server_info(state: &McpState) -> Value {
    let info = json!({
        "name": "docforge",
        "version": env!("CARGO_PKG_VERSION"),
        "mcp_protocol": LATEST_PROTOCOL,
        "base_url": state.base_url,
        "rest_api": format!("{}/api/v1", state.base_url),
        "mcp_endpoint": format!("{}/mcp", state.base_url),
        "authorization_server": state.auth.config.authorization_server_issuer(),
        "protected_resource_metadata": format!("{}/.well-known/oauth-protected-resource/mcp", state.auth.config.public_base_url),
        "dynamic_client_registration": state
            .auth
            .config
            .external
            .as_ref()
            .and_then(|external| external.registration_endpoint.clone())
            .unwrap_or_else(|| format!("{}/oauth/register", state.auth.config.issuer)),
        "max_inline_bytes": state.max_inline_bytes,
        "catalog_size": state.service.catalog().entries().len(),
        "engines": state.service.registry().descriptors(),
    });
    tool_success("DocForge MCP server information.", info, vec![])
}

fn read_resource(state: &McpState, params: Option<Value>) -> Result<Value, (i64, String)> {
    let params = params.ok_or((code::INVALID_PARAMS, "params are required".into()))?;
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((code::INVALID_PARAMS, "uri is required".into()))?;
    let (mime, text) = match uri {
        "docforge://formats" => (
            "application/json",
            serde_json::to_string_pretty(&json!({
                "formats": Format::ALL.iter().map(|format| format.info()).collect::<Vec<_>>()
            }))
            .unwrap_or_default(),
        ),
        "docforge://conversions" => (
            "application/json",
            serde_json::to_string_pretty(&json!({
                "conversions": state.service.catalog().entries(),
                "availability": state.service.graph().catalog_availability(),
            }))
            .unwrap_or_default(),
        ),
        "docforge://engines" => (
            "application/json",
            serde_json::to_string_pretty(&json!({
                "engines": state.service.registry().descriptors()
            }))
            .unwrap_or_default(),
        ),
        "docforge://server" => (
            "application/json",
            serde_json::to_string_pretty(&server_info_value(state)).unwrap_or_default(),
        ),
        other if other.starts_with("docforge://conversions/") => {
            let rank = other
                .trim_start_matches("docforge://conversions/")
                .parse::<u16>()
                .map_err(|_| {
                    (
                        code::INVALID_PARAMS,
                        "conversion rank must be a number".into(),
                    )
                })?;
            let entry = state.service.catalog().get(rank).ok_or((
                code::RESOURCE_NOT_FOUND,
                format!("no conversion with rank {rank}"),
            ))?;
            (
                "application/json",
                serde_json::to_string_pretty(entry).unwrap_or_default(),
            )
        }
        other if other.starts_with("docforge://formats/") => {
            let id = other.trim_start_matches("docforge://formats/");
            let format = Format::parse(id)
                .map_err(|_| (code::INVALID_PARAMS, format!("unknown format '{id}'")))?;
            (
                "application/json",
                serde_json::to_string_pretty(&json!({
                    "format": format.info(),
                    "targets": state.service.catalog().targets_for(format).iter().map(|target| target.id()).collect::<Vec<_>>(),
                }))
                .unwrap_or_default(),
            )
        }
        _ => {
            return Err((
                code::RESOURCE_NOT_FOUND,
                format!("resource '{uri}' is not available"),
            ))
        }
    };
    Ok(json!({
        "contents": [{"uri": uri, "mimeType": mime, "text": text}]
    }))
}

fn server_info_value(state: &McpState) -> Value {
    json!({
        "name": "docforge",
        "version": env!("CARGO_PKG_VERSION"),
        "mcp_protocol": LATEST_PROTOCOL,
        "base_url": state.base_url,
        "catalog_size": state.service.catalog().entries().len(),
        "engines": state.service.registry().descriptors(),
    })
}

fn prompts_get(id: Option<Value>, params: Option<Value>) -> RpcResponse {
    let params = params.unwrap_or_else(|| json!({}));
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = match name {
        "convert_document" => {
            let filename = arguments
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("document.pdf");
            let target = arguments
                .get("target_format")
                .and_then(Value::as_str)
                .unwrap_or("docx");
            format!(
                "Convert {filename} using DocForge. Call plan_conversion first if you need to check the route, then call convert_document with the file bytes as content_base64 and target_format \"{target}\". If the result is a document, summarize its structure; if it is an image, describe it."
            )
        }
        "choose_format" => {
            let use_case = arguments
                .get("use_case")
                .and_then(Value::as_str)
                .unwrap_or("editing a document");
            format!(
                "For the use case \"{use_case}\", call list_conversions and pick the highest-ranked conversion whose use_case matches. Explain the tradeoff between fidelity and editability, then use convert_document."
            )
        }
        _ => {
            return RpcResponse::error(
                id,
                code::INVALID_PARAMS,
                format!("unknown prompt '{name}'"),
                None,
            )
        }
    };
    RpcResponse::success(
        id,
        json!({
            "description": "DocForge conversion prompt",
            "messages": [{"role": "user", "content": {"type": "text", "text": text}}]
        }),
    )
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
