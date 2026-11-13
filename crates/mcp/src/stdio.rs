//! Line-delimited JSON-RPC transport for local MCP clients.
//!
//! Stdio mode is trusted-local: OAuth is not applied. The same tool handlers are
//! used, so behavior matches the HTTP transport.

use crate::protocol::{code, RpcResponse};
use crate::server::{handle_rpc, McpState};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn run(state: Arc<McpState>) -> std::io::Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                let response = RpcResponse::error(
                    None,
                    code::PARSE_ERROR,
                    format!("parse error: {error}"),
                    None,
                );
                write_json(&mut stdout, serde_json::to_value(response).unwrap()).await?;
                continue;
            }
        };
        let request = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(error) => {
                let response =
                    RpcResponse::error(None, code::INVALID_REQUEST, error.to_string(), None);
                write_json(&mut stdout, serde_json::to_value(response).unwrap()).await?;
                continue;
            }
        };
        let outcome = handle_rpc(&state, request, None).await;
        if let Some(body) = outcome.body {
            write_json(&mut stdout, body).await?;
        }
    }
    Ok(())
}

async fn write_json(
    stdout: &mut tokio::io::Stdout,
    value: serde_json::Value,
) -> std::io::Result<()> {
    let line = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await
}
