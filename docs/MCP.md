# MCP server

DocForge implements the [Model Context Protocol](https://modelcontextprotocol.io) **Streamable HTTP** transport at `POST /mcp`, plus a local stdio transport. The same tools run in both modes.

## Transports

### Streamable HTTP

- `POST /mcp` — JSON-RPC 2.0 request; response is `application/json`.
- `GET /mcp` — `405`; there is no standalone SSE stream (the spec permits this).
- `DELETE /mcp` — terminate an `Mcp-Session-Id`.
- Session header: `Mcp-Session-Id` (issued by `initialize`).
- Protocol header: `MCP-Protocol-Version` (`2025-06-18`, `2025-03-26`, `2024-11-05`; latest is the default).
- Notifications return `202 Accepted` with no body.
- Batches are rejected with `-32600`; send one request per HTTP POST.
- Body limit: 24 MiB. Inline conversion is additionally limited by `MCP_MAX_INLINE_MB` (16 MiB default).
- `Origin` is checked when present. Same-origin, configured origins, and localhost are allowed; other origins get `403`.

### Stdio

```bash
docforge mcp --stdio
```

Reads one JSON-RPC request per line from stdin and writes one response per line. Trusted-local; OAuth is not applied.

## Authorization

HTTP MCP requires a Bearer JWT with `aud = AUTH_MCP_RESOURCE` and scope `mcp`. On missing/invalid tokens, the server returns `401` with:

```http
WWW-Authenticate: Bearer resource_metadata="…/.well-known/oauth-protected-resource/mcp", error="invalid_token", …
```

Discovery and DCR follow the authorization-server mode:

**Keycloak mode (Docker Compose default)**

1. `GET http://localhost:8080/.well-known/oauth-protected-resource/mcp`
2. Read `authorization_servers[0]` = `http://localhost:8081/realms/docforge`.
3. Read Keycloak discovery (`/.well-known/openid-configuration`) for
   `authorization_endpoint`, `token_endpoint`, and `registration_endpoint`.
4. DCR at `.../clients-registrations/openid-connect` (anonymous local policy)
   or use the pre-registered `docforge-mcp` client.
5. Keycloak PKCE flow with `scope=openid mcp`; the realm's audience mapper adds
   `http://localhost:8080/mcp` to `aud` (Keycloak still issues online refresh
   tokens).
6. `POST /mcp` with `Authorization: Bearer …`.

**Embedded mode**

1. `GET /.well-known/oauth-protected-resource/mcp`
2. `GET /.well-known/oauth-authorization-server`
3. `POST /oauth/register` (DCR)
4. PKCE authorization code flow with `resource=<mcp URL>` and `scope=mcp offline_access`
5. `POST /mcp` with `Authorization: Bearer …`

MCP clients should handle the 401 → metadata → discovery → DCR → browser → token
sequence automatically. In Keycloak mode the browser login and consent pages are
served by Keycloak, not DocForge.

## Initialize

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-06-18",
    "capabilities": {},
    "clientInfo": {"name": "my-client", "version": "1.0"}
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2025-06-18",
    "capabilities": {
      "tools": {"listChanged": false},
      "resources": {"subscribe": false, "listChanged": false},
      "prompts": {"listChanged": false},
      "logging": {}
    },
    "serverInfo": {
      "name": "docforge",
      "title": "DocForge Document Converter",
      "version": "0.1.0"
    },
    "instructions": "…"
  }
}
```

The `Mcp-Session-Id` response header (and `_meta.sessionId` in the result) identifies the session. Send `notifications/initialized` next.

## Tools

### `list_formats`

No arguments. Returns every canonical format with `id`, `label`, `kind`, `mime`, `extensions`, `description`, and catalog `targets`.

### `list_conversions`

Arguments: `category?`, `available_only?` (default `false`), `limit?` (1–100).

Returns the 100 rows with rank, source/target labels, category, use case, OCR/searchable flags, live `available`, and a route summary.

### `plan_conversion`

Arguments: `source_format`, `target_format`, optional `options`. Returns the same plan as `POST /api/v1/conversions/plan`: steps, engines, costs, notes, and catalog rank.

### `convert_document`

Arguments:

| Name | Required | Notes |
| --- | --- | --- |
| `filename` | yes | Includes an extension; used for detection and output naming |
| `target_format` | yes | Format ID (`pdf`, `docx`, `md`, `png`, …) |
| `source_format` | no | Detected when omitted |
| `content_base64` | one of | File bytes, base64-encoded |
| `text` | one of | Plain text bytes |
| `options` | no | Same `ConversionOptions` as REST |

Exactly one of `content_base64`/`text` must be supplied. The result contains:

- a text summary with the route;
- a `resource` content block whose `resource.blob` is the base64 output and whose `uri` is `docforge://conversions/<uuid>/<filename>`;
- `structuredContent` with formats, byte counts, duration, MIME type, route, and the full plan.

Errors are tool results with `isError: true`, not JSON-RPC errors. Inputs/outputs over the inline limit are rejected with a suggestion to use REST jobs.

Outlook messages can be converted inline, for example:

```json
{
  "name": "convert_document",
  "arguments": {
    "filename": "mail.msg",
    "source_format": "msg",
    "target_format": "md",
    "content_base64": "<base64 .msg bytes>"
  }
}
```

The native `msg_parser` path extracts the HTML/plain/RTF body, sender,
recipients, date, and attachment names, then emits Markdown. `MSG -> MD` is an
extended route outside the 100 ranked catalog rows; it is listed by
`list_formats` and the REST `/api/v1/formats` target list.

### `server_info`

Version, base URL, REST/MCP/discovery URLs, limits, catalog size, and engine descriptors.

## Resources

| URI | Contents |
| --- | --- |
| `docforge://formats` | Canonical formats |
| `docforge://conversions` | Catalog + availability |
| `docforge://engines` | Engine descriptors |
| `docforge://server` | Server information |
| `docforge://conversions/{rank}` | One catalog row |
| `docforge://formats/{id}` | Format + targets |

Templates are advertised by `resources/templates/list`. `resources/read` returns `contents[0].text` with `application/json`.

## Prompts

- `convert_document` — arguments `filename`, `target_format`, optional `source_format`.
- `choose_format` — argument `use_case`.

## Error model

- Protocol errors use JSON-RPC codes: `-32700` parse, `-32600` invalid request, `-32601` method not found, `-32602` invalid params, `-32603` internal.
- Tool failures return `{"content":[{"type":"text","text":"…"}],"isError":true}`.
- Auth failures use HTTP 401/403 with `WWW-Authenticate` and a JSON body.
- Unknown sessions return HTTP 404.

## Client configuration

Remote MCP with OAuth discovery:

```json
{
  "mcpServers": {
    "docforge": {
      "type": "http",
      "url": "http://127.0.0.1:8080/mcp"
    }
  }
}
```

If your client does not implement DCR/discovery, register a client manually and provide the client ID; then complete the browser PKCE flow. For desktop clients that do not support remote OAuth, run:

```bash
docforge mcp --stdio
```

and configure the client to launch that command.

## Testing with curl

```bash
# 1) Get a demo machine token (confidential client; dev only)
curl -s -X POST http://127.0.0.1:8080/oauth/register \
  -H 'content-type: application/json' \
  -d '{"client_name":"curl","redirect_uris":["http://127.0.0.1:9876/cb"],"grant_types":["authorization_code","client_credentials"],"token_endpoint_auth_method":"client_secret_basic","scope":"mcp"}'

# 2) Exchange client credentials
curl -s -X POST http://127.0.0.1:8080/oauth/token \
  -d 'grant_type=client_credentials&client_id=...&client_secret=...&scope=mcp&resource=http://127.0.0.1:8080/mcp'

# 3) Initialize
curl -s -X POST http://127.0.0.1:8080/mcp \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"1"}}}'
```

Then send `tools/list` and `tools/call` with the returned `Mcp-Session-Id`.
