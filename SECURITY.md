# Security

## Reporting

Report vulnerabilities privately to the maintainers rather than opening a public issue. Include a reproducer, affected version/config, and impact. Do not include real user documents or tokens.

## Threat model

DocForge is a document-conversion service. It accepts untrusted files and runs third-party converters on them. The default deployment is a **single trusted-tenant service** behind your own authentication/network boundary. The embedded OAuth server is intended for first-party clients and MCP; it is not a hardened multi-tenant identity provider.

### Assets

- Uploaded documents and converted outputs (potentially confidential).
- OAuth client secrets, refresh tokens, signing keys, and access tokens.
- The host running external converters.
- The `DATA_DIR` volume.

### Trust boundaries

- Browser ↔ OAuth login/consent: credentials, CSRF, redirect URI matching.
- OAuth client ↔ token endpoint: PKCE, client authentication, code replay.
- API/MCP client ↔ resource server: Bearer JWT validation, audience, scopes, revocation.
- Upload ↔ converter: malicious files, zip bombs, path traversal, converter vulnerabilities.
- Converter ↔ host: external binaries, subprocess environment, resource limits.

## Local Keycloak (development)

`docker-compose.yml` starts Keycloak with:

- admin bootstrap credentials `admin` / `admin`;
- a demo user `demo` / `demo`;
- a dev client secret `docforge-api-secret`;
- anonymous DCR with trusted hosts `localhost`, `127.0.0.1`, and `host.docker.internal`;
- `sslRequired: none` and plain HTTP.

These are **development-only** settings. Before exposing the stack: change the
Keycloak admin password, delete or change the demo user, rotate the automation
client secret, remove/limit DCR trusted hosts, require HTTPS, and restrict who
can reach `:8081`.

## Implemented controls

- **PKCE is mandatory** for authorization-code clients; only `S256` is accepted.
- **Exact redirect URI matching**; HTTPS, HTTP loopback, or native custom schemes only; no fragments or userinfo.
- **Single-use authorization codes**, bound to client, redirect URI, resource, and PKCE challenge; short TTL.
- **Rotating refresh tokens**; old tokens are removed on use.
- **Ed25519 JWT** (embedded) or **Keycloak RS256 JWT** with `iss`/`aud`/`exp`/`iat` validation and JWKS caching/refresh.
- **Hashed secrets at rest** (SHA-256) for client secrets, codes, refresh tokens, and registration tokens; constant-time comparison.
- **Owner-scoped jobs**: the JWT `sub` must match the job owner; unauthorized lookups return 404.
- **Filename sanitization**, per-job private directories, UUID names.
- **Upload limits** (`MAX_UPLOAD_MB`, `MAX_SYNC_MB`), body limits, conversion timeouts, concurrency semaphore.
- **No shell invocation**: external commands use `Command` args, `stdin=null`, captured stdout/stderr, `kill_on_drop`.
- **MCP Origin checks** to reduce DNS rebinding; sessions expire.
- **Security headers** (`nosniff`, `DENY` framing, no-referrer, permissions policy).
- **CORS** restricted by default; `CORS_ALLOW_ANY` is opt-in.
- **Panic catching** on handlers.

## Production checklist

1. In Keycloak mode, change the admin password, demo user password, and
   `docforge-api` secret; remove anonymous DCR or restrict trusted hosts. In
   embedded mode, set `AUTH_PASSWORD` and `AUTH_DEMO_MODE=false`.
2. Serve over TLS. Set `AUTH_ISSUER` and `PUBLIC_BASE_URL` to the public HTTPS origin.
3. Put the service behind a reverse proxy with request-size/time limits and rate limiting, especially for `/oauth/authorize`, `/oauth/token`, and `/oauth/register`.
4. Restrict `AUTH_ALLOWED_ORIGINS` / `CORS_ALLOWED_ORIGINS`.
5. If DCR is internet-facing, require `AUTH_DCR_INITIAL_ACCESS_TOKEN`, or disable DCR and pre-register clients.
6. Replace the in-memory `AuthStore` and `JobStore` for multi-node deployments; persist a stable signing key (otherwise restart invalidates all tokens and clients).
7. Run converters in a sandboxed worker/container with CPU/memory/disk quotas and no network. Third-party parsers process attacker-controlled input and are outside DocForge's memory-safety guarantees.
8. Mount `DATA_DIR` on encrypted storage; apply retention/TTL; do not expose it via a static file server.
9. Monitor conversion failures, timeouts, and 401/403 rates; log request IDs but not tokens or document contents.
10. Keep LibreOffice, Poppler, Ghostscript, ImageMagick, Tesseract, Pandoc, and Calibre patched.

## Known limitations

- The embedded login is a single demo user; there is no user database, MFA, consent revocation UI, or login rate limiting.
- Auth and job state are in memory. Restart loses clients, refresh tokens, revocation entries, and job history.
- Access tokens are bearer tokens; protect them in transit and at rest. There is no DPoP or mTLS.
- No antivirus/content disarm; uploads are passed to converters as-is.
- Native PDF/Office writers focus on structure, not layout fidelity; malformed inputs may produce partial output or engine errors.
- `preserve_layout` biases routing but cannot guarantee layout preservation for native fallbacks.
- The full Docker image enables ImageMagick PDF read/write, which increases attack surface. Use `Dockerfile.slim` or a worker sandbox if you do not need those routes.

## Dependency and supply chain

- `Cargo.lock` is committed; build images use `--locked`.
- External binaries are installed from the Debian repositories at image build time.
- The bundled Roboto Medium font is OFL-licensed; see `crates/core/assets/OFL.txt`.
