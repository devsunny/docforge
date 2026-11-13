# Keycloak PKCE walkthrough

The Docker Compose stack imports `deploy/keycloak/realm.json` into a realm named
`docforge`. Keycloak is the local authorization server; DocForge is the
resource server.

| Item | Value |
| --- | --- |
| Issuer | `http://localhost:8081/realms/docforge` |
| REST client | `docforge-rest` (public) |
| REST redirect | `http://localhost:9876/callback` |
| MCP client | `docforge-mcp` (public) |
| MCP Inspector redirect | `http://localhost:6274/oauth/callback` |
| Demo user | `demo` / `demo` |
| Scopes | `openid convert convert:read mcp` (Keycloak also issues online refresh tokens) |

## REST API

Generate a PKCE verifier/challenge:

```bash
VERIFIER=$(openssl rand -base64 48 | tr -d '\n' | tr '+/' '-_' | tr -d '=')
CHALLENGE=$(printf '%s' "$VERIFIER" | openssl dgst -sha256 -binary \
  | openssl base64 -A | tr '+/' '-_' | tr -d '=')
```

Open this URL in a browser (or let your OAuth client do it):

```text
http://localhost:8081/realms/docforge/protocol/openid-connect/auth?
  client_id=docforge-rest&
  response_type=code&
  scope=openid%20convert&
  redirect_uri=http%3A%2F%2Flocalhost%3A9876%2Fcallback&
  state=demo&
  code_challenge=$CHALLENGE&
  code_challenge_method=S256
```

Sign in as `demo` / `demo`. The redirect contains `code=...`. Exchange it:

```bash
CODE=...
curl -s -X POST \
  http://localhost:8081/realms/docforge/protocol/openid-connect/token \
  -d grant_type=authorization_code \
  -d client_id=docforge-rest \
  -d redirect_uri=http://localhost:9876/callback \
  -d code="$CODE" \
  -d code_verifier="$VERIFIER"
```

Use `access_token`:

```bash
curl -s http://localhost:8080/api/v1/formats \
  -H "authorization: Bearer $ACCESS_TOKEN" | jq
```

The realm's `convert` client scope maps `aud` to `http://localhost:8080/api`.
The token also carries the scopes; DocForge rejects tokens without the right
`aud` or `convert`/`convert:read`.

## MCP Inspector

For a remote MCP client that supports OAuth discovery:

1. Connect to `http://localhost:8080/mcp` with Streamable HTTP.
2. The 401 challenge points to
   `http://localhost:8080/.well-known/oauth-protected-resource/mcp`.
3. That metadata names Keycloak as the authorization server.
4. Use `docforge-mcp` as the client ID (or let DCR register one), redirect
   `http://localhost:6274/oauth/callback`, scope `openid mcp`.
5. Complete the Keycloak login; the token audience is
   `http://localhost:8080/mcp`.

If your client asks for a client secret, use a public client (`none`); the
pre-registered MCP client has no secret.

## DCR

Keycloak's registration endpoint is:

```text
http://localhost:8081/realms/docforge/clients-registrations/openid-connect
```

The imported realm enables anonymous DCR for `localhost`, `127.0.0.1`, and
`host.docker.internal`, and allows the realm's default client scopes. For
internet-facing deployments, disable anonymous DCR and use an initial access
token or pre-register clients.

## Notes

- Keycloak's `sslRequired` is `none` for local HTTP only.
- Changing `PUBLIC_BASE_URL` or ports requires updating the realm audience
  mappers and Keycloak hostname.
- The `docforge-api` client and its `docforge-api-secret` are dev-only.
