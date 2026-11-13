# Conversions

The product contract is `catalog/conversions.json`: 100 rows, ranks 1–100, six categories. It is validated at startup (exactly 100, ranks 1..100, unique normalized pairs).

## How a pair becomes routable

An engine advertises directed pairs with a cost:

```rust
Support::new(12, "native document model")
    .layout()   // preferred when preserve_layout is requested
    .ocr()      // only eligible when OCR is requested
```

The registry probes tools at startup. The graph contains an edge for every pair an **available** engine advertises. `ConversionGraph::plan` runs Dijkstra with a hop penalty, so a direct native conversion generally beats a two-hop external route.

If no route exists, the API lists the direct catalog hint and the optional tools that would unlock it.

### OCR

OCR is opt-in (`options.ocr=true`) and catalog rows 67–69 set it automatically. When OCR is requested, non-OCR routes out of raster/PDF sources are skipped, forcing an OCR-capable engine (Tesseract). `PDF (OCR) → PDF (Searchable)` requires `pdftoppm`, `tesseract`, and `pdfunite`.

### Multi-hop examples

```
PPTX -> JPG      PPTX -> PDF (LibreOffice) -> JPG (Poppler/Ghostscript)
DOC  -> DOCX     DOC  -> TXT (antiword) -> DOCX (native)
PDF  -> CSV      PDF  -> TXT (Poppler) -> CSV (native table heuristic)
EPUB -> DOCX     EPUB -> HTML (native/Pandoc) -> DOCX
```

## Catalog categories

| Category | Ranks | Focus |
| --- | --- | --- |
| Document & Word Processing | 1–30 | PDF/Office/Markdown/HTML/e-book text |
| Presentation & Slide Formats | 31–43 | PPTX/PPT/KEY/ODP/PDF |
| Spreadsheets & Tabular Data | 44–57 | XLSX/XLS/ODS/CSV/TSV/PDF |
| Document-to-Image & OCR | 58–74 | PDF/image/HEIC/SVG + OCR |
| Structured Data, Serialization & Web Formats | 75–88 | JSON/XML/YAML/TOML/CSV/HTML |
| Specialized, Legacy & Technical Formats | 89–100 | LaTeX/PS/XPS/DjVu/VCF/ICS/EML/CBZ |

## Native coverage

The pure-Rust engine handles:

- **Documents:** PDF → text/Markdown/HTML/DOCX/RTF/ODT/EPUB/PPTX/TeX (text layer only); DOCX/ODT/RTF/HTML/Markdown/TXT/EPUB/PPTX/EML/TeX → PDF/DOCX/ODT/RTF/HTML/Markdown/TXT/EPUB/PPTX/TeX.
- **Structured data:** JSON ↔ YAML/TOML/XML/CSV/XLSX, XML ↔ JSON/CSV, CSV ↔ TSV/JSON/XML/VCF/ICS, JSON/CSV → XLSX/PDF/DOCX.
- **Spreadsheets:** CSV/TSV/XLSX/XLS/ODS → CSV/TSV/XLSX/ODS/JSON/XML/HTML/Markdown/PDF/DOCX.
- **Contacts/calendar:** VCF/ICS ↔ CSV, and as document tables.
- **Images:** JPG/PNG/TIFF/WEBP/BMP → each other and PDF; SVG → PNG/JPG/PDF; CBZ → PDF.
- **Email:** EML and Outlook MSG → PDF/DOCX/HTML/Markdown/TXT. MSG parsing reads the Outlook MAPI body (HTML, plain text, or RTF fallback), sender/recipients/date, and attachment names; `MSG → Markdown` is available even though it is not one of the 100 ranked catalog rows.

Native PDF output embeds a Unicode font (Roboto Medium, OFL) and produces a structured text layout, not a pixel clone. Native DOCX/ODT/EPUB/PPTX writers produce standards-compliant ZIP packages.

## External engines and tool matrix

| Tool | Unlocks |
| --- | --- |
| `soffice` | High-fidelity Office/ODF/RTF/PDF/HTML, `PPTX→JPG` via PDF |
| `pandoc` | Markdown/HTML/DOCX/ODT/EPUB/LaTeX/RTF/PPTX |
| `pdftotext`, `pdftohtml`, `pdftocairo`, `pdftoppm` | PDF → TXT/HTML/SVG/PNG/JPG/TIFF |
| `gs` | PS → PDF, PDF → PS/PNG/JPG/TIFF, XPS (with gxps) |
| `magick` | HEIC/SVG/image conversions, PDF raster |
| `ebook-convert` | EPUB ↔ MOBI, EPUB → PDF/DOCX |
| `tesseract` | Image OCR, PDF searchable output |
| `7z` | CBR extraction → PDF |
| `ddjvu`, `cjb2`, `djvm` | DjVu → PDF/TIFF, PDF → DjVu |
| `gxps` | XPS → PDF |
| `antiword` | Legacy DOC → TXT |
| `ffmpeg` | HEIC → JPG/PNG |
| `osascript` (macOS) | Pages/Key/Numbers → PDF/DOCX/PPTX/XLSX |

The full Docker image installs the Linux-capable subset. Apple iWork formats are only usable on macOS.

## Known limitations

- **Layout fidelity:** native routes preserve block structure, not typography. Use LibreOffice/Pandoc for layout-sensitive work and set `options.preserve_layout=true` to bias the planner toward layout engines.
- **Scanned PDFs:** use OCR. Native PDF text extraction returns an error when the text layer is empty.
- **PDF tables:** native `PDF→CSV/XLSX` uses whitespace/run heuristics; complex merged tables should use a dedicated table extractor.
- **Multi-page image output:** PDF→JPG/PNG/TIFF currently returns the selected (or first) page. Use a job pipeline or a dedicated raster tool for page archives.
- **Apple iWork:** only on macOS with Pages/Keynote/Numbers installed.
- **DCR/auth storage:** in-memory; restart invalidates clients, codes, refresh tokens, and the signing key unless replaced.
- **Unicode shaping:** the native PDF writer embeds a Latin/Cyrillic/Greek-capable font; complex scripts (Arabic/Indic/CJK) need an external PDF engine.

## Availability API

`GET /api/v1/conversions?available_only=true` returns rows the current deployment can route. `GET /api/v1/engines` shows the reason a row may be unavailable. The startup log prints the available engine count.
