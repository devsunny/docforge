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

## Rust crates

All other Rust dependencies are declared in `Cargo.toml` / `Cargo.lock` and
retain their upstream licenses.
