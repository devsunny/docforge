# OAuth 2.1

DocForge supports two authorization-server modes:

| `AUTH_MODE` | Authorization server | Default |
| --- | --- | --- |
| `keycloak` | Keycloak realm `docforge` in Docker Compose | Compose |
| `embedded` | DocForge's own `/oauth/*` server | `cargo run` / tests |

In both modes the **resource** side is identical: DocForge serves RFC 9728
protected-resource metadata and validates the Bearer token's signature, issuer,
audience, expiry, and scopes.

## Keycloak mode (local IdP)

`docker compose up --build` starts Keycloak (`quay.io/keycloak/keycloak:26.4`)
and imports [`deploy/keycloak/realm.json`](../deploy/keycloak/realm.json).

| Item | Value |
| --- | --- |
| Realm issuer | `http://localhost:8081/realms/docforge` |
| Admin console | `http://localhost:8081` (`admin` / `admin`) |
| Demo user | `demo` / `demo` |
| REST client | `docforge-rest` (public, PKCE) |
| MCP client | `docforge-mcp` (public, PKCE) |
| Automation client | `docforge-api` (confidential, secret `docforge-api-secret`, dev only) |
| Scopes | `convert`, `convert:read`, `mcp`, `offline_access` |
| Audiences | `http://localhost:8080/api`, `http://localhost:8080/mcp` |

The realm's client scopes contain audience mappers, so a token requested with
`scope=openid convert` has `aud` including `http://localhost:8080/api`. DocForge
validates against `AUTH_API_AUDIENCE` and `AUTH_MCP_AUDIENCE`.

Keycloak serves the authorization server metadata at:

```
http://localhost:8081/realms/docforge/.well-known/openid-configuration
```

DocForge serves protected-resource metadata at:

```
http://localhost:8080/.well-known/oauth-protected-resource/api
http://localhost:8080/.well-known/oauth-protected-resource/mcp
```

### Keycloak: browser PKCE

```text
GET http://localhost:8081/realms/docforge/protocol/openid-connect/auth
  ?client_id=docforge-rest
  &response_type=code
  &scope=openid%20convert
  &redirect_uri=http%3A%2F%2Flocalhost%3A9876%2Fcallback
  &code_challenge=<S256>
  &code_challenge_method=S256
  &state=<opaque>
```

Exchange at `.../protocol/openid-connect/token` with
`grant_type=authorization_code`, `code`, `redirect_uri`, `client_id`, and
`code_verifier`. No client secret is needed for the public clients.

### Keycloak: DCR for MCP

Keycloak exposes OpenID Connect Dynamic Client Registration at:

```
http://localhost:8081/realms/docforge/clients-registrations/openid-connect
```

The imported realm enables anonymous registration for the local hosts
(`localhost`, `127.0.0.1`, `host.docker.internal`) and allows the realm's
default client scopes. MCP clients can register with
`token_endpoint_auth_method: "none"`, get a client ID, then run the normal
Keycloak PKCE flow. For internet-facing deployments, replace anonymous DCR
with an initial access token or pre-register clients.

### Automation

The confidential `docforge-api` client can obtain a service token:

```bash
curl -s -X POST http://localhost:8081/realms/docforge/protocol/openid-connect/token \
  -d grant_type=client_credentials \
  -d client_id=docforge-api \
  -d client_secret=docforge-api-secret \
  -d scope=convert
```

Use that token as `Authorization: Bearer ...` against `/api/v1`.

### Changing the public URL

The realm import hardcodes `http://localhost:8080` in the audience mappers and
the Keycloak hostname. If you change ports/hosts, update
`deploy/keycloak/realm.json`, `KC_HOSTNAME`, `AUTH_API_AUDIENCE`,
`AUTH_MCP_AUDIENCE`, and the Keycloak endpoints in `.env`/Compose, then recreate
the realm/container.

## Embedded mode

DocForge embeds a small authorization server for two resource audiences:

| Resource | URL | JWT `aud` | Required scope |
| --- | --- | --- | --- |
| REST API | `${PUBLIC_BASE_URL}/api` | same | `convert` or `convert:read` |
| MCP | `${PUBLIC_BASE_URL}/mcp` | same | `mcp` |

The issuer is `AUTH_ISSUER` (defaults to `PUBLIC_BASE_URL`). It must be the same URL in discovery, JWTs, and client configuration.

## Discovery

```http
GET /.well-known/oauth-authorization-server
```

Returns RFC 8414 metadata:

```json
{
  "issuer": "https://bridge.example.com",
  "authorization_endpoint": "https://bridge.example.com/oauth/authorize",
  "token_endpoint": "https://bridge.example.com/oauth/token",
  "registration_endpoint": "https://bridge.example.com/oauth/register",
  "jwks_uri": "https://bridge.example.com/oauth/jwks",
  "revocation_endpoint": "https://bridge.example.com/oauth/revoke",
  "scopes_supported": ["convert", "convert:read", "mcp", "offline_access"],
  "response_types_supported": ["code"],
  "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials"],
  "token_endpoint_auth_methods_supported": ["none", "client_secret_basic", "client_secret_post"],
  "code_challenge_methods_supported": ["S256"],
  "authorization_response_iss_parameter_supported": true
}
```

Protected-resource metadata (RFC 9728):

```http
GET /.well-known/oauth-protected-resource/mcp
GET /.well-known/oauth-protected-resource/api
```

```json
{
  "resource": "https://bridge.example.com/mcp",
  "authorization_servers": ["https://bridge.example.com"],
  "scopes_supported": ["mcp"],
  "bearer_methods_supported": ["header"],
  "resource_name": "DocForge MCP server"
}
```

A `401` includes:

```http
WWW-Authenticate: Bearer resource_metadata="https://bridge.example.com/.well-known/oauth-protected-resource/mcp", error="invalid_token", error_description="â€¦"
```

A `403` insufficient-scope response adds `scope="convert"` (or the missing scope).

## Authorization code + PKCE

### 1. Register a client

```http
POST /oauth/register
Content-Type: application/json

{
  "client_name": "My converter",
  "redirect_uris": ["http://127.0.0.1:9876/callback"],
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "token_endpoint_auth_method": "none",
  "scope": "convert mcp offline_access"
}
```

Response `201` contains `client_id`, `registration_access_token`, and `registration_client_uri`. Public clients (`none`) receive no secret and must use PKCE. Confidential clients receive a one-time `client_secret`.

Redirect URI rules:

- `https://` with a host;
- `http://` only for loopback hosts (`localhost`, `127.0.0.1`, `::1`);
- native custom schemes with a host/path;
- no fragments, no userinfo, no `javascript`/`data`/`file`.

If `AUTH_DCR_INITIAL_ACCESS_TOKEN` is configured, DCR requires `Authorization: Bearer <token>`.

### 2. Authorize

Generate a 43â€“128 character verifier and its S256 challenge:

```text
verifier  = 43..128 chars from [A-Za-z0-9-._~]
challenge = BASE64URL(SHA256(ASCII(verifier)))
```

Open:

```text
/oauth/authorize?
  response_type=code&
  client_id=...&
  redirect_uri=...&
  scope=convert%20offline_access&
  state=...&
  code_challenge=...&
  code_challenge_method=S256&
  resource=https%3A%2F%2Fbridge.example.com%2Fapi
```

The login/consent page posts back to `/oauth/authorize` with the same fields plus `username` and `password`. On success the browser is redirected with:

```text
?code=...&iss=https%3A%2F%2Fbridge.example.com&state=...
```

Authorization codes are single-use, bound to `client_id`, `redirect_uri`, `resource`, and the PKCE challenge, and expire after `AUTH_AUTHORIZATION_CODE_TTL` (300s default).

### 3. Exchange

```http
POST /oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=authorization_code&
code=...&
redirect_uri=...&
client_id=...&
code_verifier=...&
resource=https%3A%2F%2Fbridge.example.com%2Fapi
```

Public clients send `client_id`; confidential clients additionally authenticate with `client_secret_basic` or `client_secret_post`.

Response:

```json
{
  "access_token": "eyJ...",
  "token_type": "Bearer",
  "expires_in": 3600,
  "refresh_token": "â€¦",
  "scope": "convert offline_access"
}
```

Access tokens are Ed25519 JWTs. Claims include `iss`, `sub`, `aud`, `client_id`, `scope`, `exp`, `iat`, `jti`, `resource`, and `username`. Refresh tokens rotate on every use; reuse of an old token fails.

### 4. Refresh

```http
POST /oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token&
refresh_token=...&
client_id=...
```

Optional `scope` may narrow the grant; it cannot widen it.

## Client credentials

Confidential clients may request a machine token:

```http
POST /oauth/token
Authorization: Basic BASE64(client_id:client_secret)
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&scope=mcp&resource=https%3A%2F%2Fbridge.example.com%2Fmcp
```

`client_credentials` requires a confidential client (`client_secret_basic`/`post`) and a registered `client_credentials` grant.

## Revocation

```http
POST /oauth/revoke
Content-Type: application/x-www-form-urlencoded

token=...&client_id=...
```

Refresh tokens are removed immediately; access-token `jti` values are added to the in-memory revocation set until they expire.

## JWT verification

`/oauth/jwks` returns an Ed25519 `OKP` key:

```json
{"keys":[{"kty":"OKP","crv":"Ed25519","x":"â€¦","use":"sig","alg":"EdDSA","kid":"â€¦"}]}
```

Resource servers verify signature, `alg=EdDSA`, `kid`, `exp`, `iat`, `iss`, and `aud`. DocForge's middleware does this internally; external services can fetch the JWKS.

## Production notes

- Change `AUTH_PASSWORD` and disable `AUTH_DEMO_MODE`.
- Use TLS; `http` is intended for loopback development.
- Set `AUTH_ISSUER` to the externally visible HTTPS URL. If behind a reverse proxy, set `PUBLIC_BASE_URL` to the public URL as well.
- Restrict `AUTH_ALLOWED_ORIGINS` and CORS.
- For multi-node deployments, replace the in-memory `AuthStore` and use a stable signing key so tokens survive restarts.
- Consider requiring `AUTH_DCR_INITIAL_ACCESS_TOKEN` if DCR is exposed to the internet.
- Login attempts are not rate-limited in the embedded demo UI; put the service behind a rate-limiting proxy or integrate an identity provider for real user auth.

## Standards

- RFC 6749 / RFC 6750 (OAuth 2.0 / Bearer)
- RFC 7636 (PKCE, S256 mandatory here)
- RFC 7591 / RFC 7592 (Dynamic Client Registration)
- RFC 8414 (Authorization Server Metadata)
- RFC 8707 (Resource Indicators)
- RFC 9728 (Protected Resource Metadata)
- OAuth 2.1 draft semantics: no implicit/password grants, PKCE required, exact redirect matching, rotating refresh tokens.
