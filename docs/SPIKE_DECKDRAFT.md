# DeckCraft PPTX spike

Status: **experimental, feature-gated, not default**.

## Why

DeckCraft provides published pure-Rust PPTX import/export and PDF export crates:

- `deckcraft-pptx` — PresentationML/DrawingML import/export
- `deckcraft-pdf` — slide renderer + selectable text layer, links, bookmarks

The spike asks whether DeckCraft can replace the current PPTX path:

- native `document/pptx.rs` writer
- `office2pdf` PPTX → PDF
- LibreOffice PPTX → PDF

## Enable

```bash
cargo build --release -p docforge-server --features deckcraft
```

or run the CLI:

```bash
cargo run -p docforge-server --features deckcraft -- \
  convert deck.pptx --to pdf --output deck.pdf
```

Feature graph:

```text
docforge-server/deckcraft
  -> docforge-core/deckcraft
      -> deckcraft-pptx
      -> deckcraft-pdf
```

The default build does not compile or link DeckCraft.

## Planner behavior

When the feature is enabled, engine `deckcraft` advertises:

```text
PPTX -> PDF   cost 4   layout
```

That outranks `office2pdf` (cost 6) and LibreOffice. Disable the feature to
return to the default PPTX path.

## Spike result

Test deck: one slide with a title, paragraph, and bullet list, generated from
Markdown.

| Path | Time | PDF size | Text layer | Notes |
| --- | ---: | ---: | --- | --- |
| DeckCraft | ~0.9 s | 150,669 bytes | Selectable, clean spacing | Rendered slide background + invisible text layer |
| `office2pdf` | ~0.22 s | 13,098 bytes | Selectable | Pure text/layout, serif fallback |

Visual comparison:

- DeckCraft preserved the slide theme typography (sans-serif), spacing, and
  bullet layout.
- `office2pdf` produced a simpler structural page with a serif font.
- DeckCraft’s PDF is larger because each slide is rendered at print resolution
  with a text layer over it; it is not a purely vector PDF.

## Current limitations

- DeckCraft is **alpha** and its crate APIs may change.
- PDF export is raster background + text layer, not fully vector.
- Larger PDFs and slower conversion than `office2pdf`.
- The spike supports PPTX → PDF only.
- Legacy `.ppt` still needs LibreOffice.
- Notes pages/handouts and slide ranges are not exposed yet, although
  `deckcraft-pdf` supports them.
- Font availability and theme fidelity need a wider corpus test.

## Recommendation

Keep DeckCraft behind the `deckcraft` feature while evaluating:

1. a larger corpus with themes, images, charts, tables, and master layouts;
2. PDF text/search quality;
3. file size and conversion time;
4. whether `deckcraft-pptx` is stable enough to replace the native PPTX
   writer.

Do not make it the default until those checks pass.
