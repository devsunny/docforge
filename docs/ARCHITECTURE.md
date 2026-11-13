# Architecture

## Goals

1. **One catalog, two interfaces.** The 100 conversions in `catalog/conversions.json` are the product contract. REST and MCP read the same catalog, the same planner, and the same engines.
2. **Arbitrary source/target pairs where possible.** Direct catalog rows are preferred, but the planner can chain engines across intermediate formats. This is how `PPTX → JPG` works via `PPTX → PDF → JPG`, or `DOC → DOCX` via LibreOffice or `DOC → TXT → DOCX`.
3. **Graceful degradation.** The native Rust engines run without external binaries, including DOCX/PPTX/XLSX→PDF (Typst), PDF→raster, and English/Latin OCR. Optional engines are probed at startup; missing tools are visible in `/api/v1/engines` and excluded from routing.
4. **OAuth is part of the product.** Docker Compose uses **Keycloak** (RS256 JWT, PKCE, DCR) as the local identity provider; DocForge serves RFC 9728 resource metadata and validates Keycloak tokens via the realm JWKS. A self-contained Ed25519 authorization server remains available with `AUTH_MODE=embedded` for zero-dependency runs and tests.

## Runtime topology

```
                       ┌──────────────────────────────────────────────┐
Browser ──PKCE──────▶  │  /oauth/authorize  /oauth/token  /oauth/register
                       │  /.well-known/oauth-authorization-server
REST client ─Bearer─▶  │  /api/v1/*                      (aud = /api)
MCP client ─Bearer──▶  │  /mcp                           (aud = /mcp)
                       │  └── same ConversionService ──▶ planner ──▶ engines
                       └──────────────────────────────────────────────┘
```

The server is one process and one `axum` router. In Keycloak mode, DocForge is a resource server plus metadata host; Keycloak performs login, consent, token issuance, and DCR. In embedded mode, DocForge also hosts the authorization server. `AuthState` abstracts both through `verify_access`.

## Conversion pipeline

1. **Detect input.** Extension + magic bytes + container inspection (ZIP/OLE) + text heuristics. Explicit `source_format` wins.
2. **Plan.** Build a weighted directed graph:
   - Nodes: canonical `Format` values.
   - Edges: every pair advertised by an available engine.
   - Weight: engine cost + hop penalty; `layout` edges get a small discount for PDF targets; OCR edges are only eligible when OCR is requested.
   - Algorithm: Dijkstra. Identity is a zero-cost copy.
3. **Execute.** Each step runs an `Engine` with an isolated working directory. Intermediates are deleted by `TempDir` cleanup. Every step is timeout-bounded.
4. **Verify.** Output must exist and be non-empty. The response includes the plan, route summary, duration, and byte count.

Catalog rows carry availability computed by the planner at startup, but the planner is re-run per request (so options like OCR are respected).

## Crates

### `docforge-core`

| Module | Purpose |
| --- | --- |
| `format.rs` | Canonical enum, aliases, labels, MIME types, extensions, families |
| `catalog.rs` | Embedded JSON, validation (exactly 100, ranks 1..100, unique pairs), lookup |
| `graph.rs` | Edge construction from engine `supports`, Dijkstra, catalog availability |
| `engine.rs` | `Engine` trait, registry, probing, engine execution |
| `document/` | `Document` block model; DOCX/ODT/RTF/PDF/EPUB/PPTX readers/writers |
| `data.rs` | CSV/TSV/XLSX/XLS/ODS, JSON/YAML/TOML/XML, VCF/ICS, images, CBZ |
| `native.rs` | Route matrix and dispatch for the pure-Rust engine |
| `native_extra.rs` | DjVu, CBR/RAR, HEIC, and legacy Word adapters |
| `native_extra2.rs` | MOBI/EPUB, Apple iWork, and PostScript adapters |
| `office.rs` | DOCX/PPTX/XLSX → PDF through vendored `office2pdf`/Typst |
| `pdf_raster.rs` | Pure-Rust PDF page rasterization through `pdf-render` |
| `ocr.rs` | Native OCR engine through embedded `ocrs` models |
| `external.rs` | LibreOffice, Pandoc, Poppler, Ghostscript, ImageMagick, Calibre, Tesseract, 7-Zip, DjVu, gxps, antiword, FFmpeg, iWork |
| `detect.rs` | Content-based detection |
| `service.rs` | Planner + engine execution + inline byte conversion |

### `docforge-auth`

| Module | Purpose |
| --- | --- |
| `config.rs` | Embedded/external mode, issuer/resources/scopes/TTLs, RFC 8414 + 9728 documents |
| `external.rs` | Keycloak/OIDC RS256 JWT verification + JWKS cache/refresh |
| `jwt.rs` | Ed25519 signing/verification, JWKS, claims |
| `pkce.rs` | S256 verification, RFC 7636 test vector |
| `store.rs` | Clients, codes, refresh tokens, revocation; SHA-256 hashes |
| `routes.rs` | `/oauth/*`, DCR, login/consent, metadata, JWKS |
| `middleware.rs` | REST/MCP Bearer validation and challenges |

### `docforge-mcp`

| Module | Purpose |
| --- | --- |
| `protocol.rs` | JSON-RPC 2.0 types and MCP protocol constants |
| `server.rs` | Method dispatch, tools, resources, prompts, sessions |
| `http.rs` | Streamable HTTP transport, Origin checks, auth challenge |
| `stdio.rs` | Line-delimited JSON-RPC for local clients |

### `docforge-server`

| Module | Purpose |
| --- | --- |
| `config.rs` | Environment configuration and directories |
| `jobs.rs` | In-memory job records + disk artifacts |
| `api.rs` | REST handlers, multipart parsing, downloads |
| `lib.rs` | `AppState`, router assembly, CORS, security headers, OpenAPI |
| `main.rs` | `docforge serve`, `mcp --stdio`, `formats`, `convert` |

## Data and lifetime

- Jobs live under `DATA_DIR/jobs/<uuid>/input.<ext>` and `output.<ext>`.
- Records are in memory; a restart loses history but leaves files for TTL cleanup.
- Download is authorized by the job owner (`sub` in the JWT). Jobs are never shared across subjects.
- Completed jobs are cleaned after `JOB_TTL_HOURS`; orphan directories are also removed.
- MCP converts inline with a temporary directory and returns a base64 resource; large files should use REST jobs.

## Concurrency and failure isolation

- A semaphore limits concurrent jobs (`CONVERSION_CONCURRENCY`).
- Every engine invocation has a timeout; external child processes are killed on drop.
- Engines run with `stdin` closed and captured stdout/stderr; stdout/stderr tails are included in errors.
- A missing external tool does not fail startup; it removes those edges from the graph.
- Panics in handlers are caught by `CatchPanicLayer`.

## Extension points

- **New native conversion:** add a route in `native.rs` and implement the reader/writer/model path.
- **New external tool:** implement `Engine`, probe it in `external.rs`, and declare `supports` pairs. The graph picks it up automatically.
- **New auth store:** replace `AuthStore` methods with a database/Redis implementation; keep the hash-at-rest behavior.
- **New transport:** use `docforge_mcp::server::handle_rpc` directly; stdio and HTTP are thin adapters.
