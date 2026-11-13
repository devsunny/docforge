//! DocForge MCP server.
//!
//! Implements the Streamable HTTP transport (`POST /mcp`) plus a stdio mode and
//! exposes document conversion as MCP tools. OAuth 2.1 + PKCE + DCR is enforced
//! on the HTTP transport by the shared DocForge authorization server.

pub mod http;
pub mod protocol;
pub mod server;
pub mod stdio;

pub use http::router;
pub use server::McpState;
