# Third-party notices

DocForge is MIT-licensed. The following bundled components have their own
licenses.

## office2pdf

- Source: <https://github.com/developer0hye/office2pdf>
- Version: 0.10.0
- License: Apache-2.0
- Location: [`vendor/office2pdf`](vendor/office2pdf)
- Patch: removed the unnecessary `cdylib` crate type so Windows GNU builds do
  not exceed MinGW's export-ordinal limit. The library API is unchanged.

## ocrs models

- Source: <https://github.com/robertknight/ocrs-models>
- Files: `crates/core/assets/ocr/text-detection.onnx`,
  `crates/core/assets/ocr/text-recognition.onnx`
- License: **CC BY-SA 4.0**
- Attribution: *Ocrs models (https://github.com/robertknight/ocrs-models),
  trained on the HierText dataset by Google Research. Licensed under
  CC BY-SA 4.0.*
- The `ocrs` Rust code is MIT OR Apache-2.0; the model files are data assets
  under CC BY-SA 4.0.

## Native format engines

| Crate | License | Use |
| --- | --- | --- |
| `djvu-rs` | MIT | DjVu parsing, rendering, PDF/text export |
| `rars` | Apache-2.0 | RAR/CBR extraction |
| `heic-rs` | MIT OR Apache-2.0 | HEIC/HEIF decoding |
| `rust-docx` | MIT | Legacy Word parsing and DOCX serialization |
| `epub3-kindle` | MIT | EPUB → MOBI |
| `mobi` | MIT | MOBI parsing |
| `iwork` | MIT | Pages/Numbers/Keynote content extraction |
| `stet` | Apache-2.0 OR MIT | PostScript interpretation/rendering; embeds URW base-35 fonts under SIL OFL 1.1 |

## Optional DeckCraft spike

When built with `--features deckcraft`, DocForge links:

- `deckcraft-pptx` — MIT OR Apache-2.0
- `deckcraft-pdf` — MIT OR Apache-2.0

## Rust crates

All other Rust dependencies are declared in `Cargo.toml` / `Cargo.lock` and
retain their upstream licenses.
