# Local Keycloak

`docker-compose.yml` starts Keycloak 26.4 and imports `realm.json`.

## URLs and credentials

| Item | Value |
| --- | --- |
| Keycloak | `http://localhost:8081` |
| Admin console | `http://localhost:8081/admin` |
| Admin | `admin` / `admin` (bootstrap, dev only) |
| Realm | `docforge` |
| Issuer | `http://localhost:8081/realms/docforge` |
| Discovery | `http://localhost:8081/realms/docforge/.well-known/openid-configuration` |
| JWKS | `http://localhost:8081/realms/docforge/protocol/openid-connect/certs` |
| DCR | `http://localhost:8081/realms/docforge/clients-registrations/openid-connect` |

## Clients

| Client | Type | Redirects | Scopes |
| --- | --- | --- | --- |
| `docforge-rest` | public, PKCE | `http://localhost:9876/callback`, `http://127.0.0.1:9876/callback` | `convert`, `convert:read` |
| `docforge-mcp` | public, PKCE | `http://localhost:6274/oauth/callback`, `http://127.0.0.1:6274/oauth/callback`, loopback 9876 | `mcp` |
| `docforge-api` | confidential | — | `convert`, `convert:read` |

`docforge-api` has the dev secret `docforge-api-secret` and service
accounts enabled. Rotate/remove it outside local development.

## Scopes and audiences

| Scope | Audience added to the token |
| --- | --- |
| `convert` | `http://localhost:8080/api` |
| `convert:read` | `http://localhost:8080/api` |
| `mcp` | `http://localhost:8080/mcp` |
| `basic` | internal OpenID scope used by Keycloak DCR |

The audience values are hardcoded in `realm.json` and must match
`AUTH_API_AUDIENCE` / `AUTH_MCP_AUDIENCE` in Compose.

## Dynamic client registration

Anonymous DCR is enabled for trusted hosts:

- `localhost`
- `127.0.0.1`
- `host.docker.internal`

The client URI check is on; the request-source check is off (Docker's gateway
address differs from the host). Allowed scopes are `basic`, `convert`,
`convert:read`, and `mcp`. This is a local-development policy. For production,
disable anonymous DCR or use an initial access token, and restrict trusted hosts.

## Reset the realm

The realm import uses `IGNORE_EXISTING`, and Keycloak data is stored in the
`keycloak-data` volume. To re-import after editing `realm.json`:

```bash
docker compose down -v
docker compose up --build
```

Removing the volume also invalidates previously issued tokens and dynamically
registered clients.

## Changing the public URL

The import hardcodes `localhost:8080` and `localhost:8081`. If you change ports
or hostnames, update:

- `realm.json` audience mapper values;
- `KC_HOSTNAME` in `docker-compose.yml`;
- `KEYCLOAK_*` endpoints;
- `AUTH_API_AUDIENCE` / `AUTH_MCP_AUDIENCE`;
- `PUBLIC_BASE_URL`;
- registered redirect URIs and `webOrigins`.

Then recreate the volume.
