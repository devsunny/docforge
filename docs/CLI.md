# DocForge CLI

The `docforge` binary can run the server, a local MCP server, or a local
conversion CLI. Local conversions use the same catalog, planner, native engine,
and optional external tools as the REST API and MCP server, but do **not**
require OAuth, Keycloak, or a network service.

## Install

```bash
cargo build --release -p docforge-server
# target/release/docforge

# or install into ~/.cargo/bin
cargo install --path crates/server
```

## Convert

```bash
docforge convert <INPUT> --to <FORMAT> [--output <PATH>]
docforge convert <INPUT> <FORMAT> [--output <PATH>]
```

Examples:

```bash
docforge convert report.pdf --to docx -o report.docx
docforge convert notes.md pdf
docforge convert scan.jpg --to txt --ocr
docforge convert mail.msg --to md -o mail.md
docforge convert deck.pptx --to pdf --page-size a4 --landscape
```

The target can be an ID (`pdf`, `docx`, `md`, `xlsx`, …), a file extension
(`.pdf`), or a MIME type. The source is detected from content and extension; use
`--from <format>` to override it.

### Options

| Option | Meaning |
| --- | --- |
| `-o, --output <PATH>` | Output path; defaults to `<input-stem>.<target-ext>` |
| `--from <FORMAT>` | Override source-format detection |
| `--stdout` | Write converted bytes to stdout (summary to stderr) |
| `--force` | Overwrite an existing output |
| `--json` | Machine-readable result |
| `--quiet` | Suppress the human summary |
| `--ocr` | Enable OCR (images/scanned PDFs) |
| `--ocr-language <LANG>` | Tesseract language(s), default `eng` |
| `--preserve-layout` | Prefer layout-preserving engines |
| `--pdf-dpi <N>` | PDF rasterization/output DPI (36–1200) |
| `--page-size <a4\|letter\|legal>` | PDF page size |
| `--landscape` | Landscape PDF pages |
| `--margin-mm <N>` | PDF margin in millimetres |
| `--image-quality <1-100>` | JPEG quality |
| `--csv-delimiter <CHAR>` | CSV/TSV delimiter override |
| `--timeout-secs <N>` | Hard per-conversion timeout |

### stdin/stdout

```bash
cat notes.md | docforge convert - --to pdf --stdout > notes.pdf
docforge convert scan.png --to txt --ocr --stdout
```

When reading stdin, write the input to a temporary file first. The CLI chooses
a writable scratch directory automatically; set `DOCFORGE_TMP=/path` if needed.

**Windows PowerShell:** `>` is a text redirect and will corrupt binary output
(including PDFs). Use `--output` instead:

```powershell
.\\docforge.exe convert .\\input.docx --to pdf --output .\\output.pdf
```

Or use cmd.exe's byte-preserving redirect:

```bat
cmd /c "docforge.exe convert input.docx --to pdf --stdout > output.pdf"
```

## Batch

```bash
docforge batch ./inbox --to pdf --output-dir ./out
docforge batch ./inbox --to docx --output-dir ./out --recursive
docforge batch ./inbox --to pdf --output-dir ./out --force --continue-on-error
```

Batch:
- lists files in deterministic order;
- detects each source format;
- writes `<output-dir>/<input-stem>.<target-ext>`;
- skips existing outputs unless `--force`;
- stops on the first error unless `--continue-on-error`;
- prints a summary, or JSON with `--json`.

It converts every file it can detect, including text, structured data, images,
and documents. Unsupported pairs fail with the planner’s error.

## Plan, formats, engines

```bash
docforge plan msg md
docforge plan pdf docx --json

docforge formats
docforge formats --available-only --json
docforge formats --category "Document-to-Image & OCR"

docforge engines
docforge engines --json
```

`plan` is useful before converting an unfamiliar format: it shows the direct or
multi-hop route and the engine for each step.

## MCP over stdio

```bash
docforge mcp --stdio
```

Reads line-delimited JSON-RPC from stdin and writes responses to stdout. This is
the local, trusted transport; OAuth is not applied.

## Server

```bash
docforge serve
docforge serve --bind 0.0.0.0:8080 --public-url https://docs.example.com
```

This starts the REST API, OAuth endpoints, MCP HTTP endpoint, and discovery
documents. See the main [README](../README.md) and [OAuth](OAUTH.md) /
[MCP](MCP.md) docs.

## Exit behavior

- `0`: success
- non-zero: invalid arguments, unsupported conversion, missing input, existing
  output without `--force`, engine failure, or timeout
- errors are printed to stderr; `--json` still prints structured results for
  successful conversions
