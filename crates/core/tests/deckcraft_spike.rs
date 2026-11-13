#![cfg(feature = "deckcraft")]

use docforge_core::document::write::write_document;
use docforge_core::document::{Block, Document};
use docforge_core::format::Format;
use docforge_core::options::ConversionOptions;

#[test]
fn deckcraft_imports_and_exports_pptx() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("deck.pptx");
    let mut document = Document::new("DeckCraft spike");
    document.push(Block::Heading {
        level: 1,
        text: "DeckCraft PPTX".to_string(),
    });
    document.push(Block::Paragraph(
        "A spike for the DeckCraft PPTX-to-PDF engine.".to_string(),
    ));
    write_document(
        &input,
        &document,
        Format::Pptx,
        &ConversionOptions::default(),
    )
    .unwrap();

    let bytes = std::fs::read(&input).unwrap();
    let presentation = deckcraft_pptx::import(&bytes).unwrap();
    let pdf = deckcraft_pdf::export(
        &presentation,
        &deckcraft_pdf::PdfOptions {
            text_layer: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    assert!(pdf.len() > 1000);
    assert!(!presentation.slides.is_empty());
}

#[tokio::test]
async fn deckcraft_engine_routes_pptx_to_pdf() {
    use docforge_core::catalog::Catalog;
    use docforge_core::engine::EngineRegistry;
    use docforge_core::graph::ConversionGraph;
    use docforge_core::service::ConversionService;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("deck.pptx");
    let output = dir.path().join("deck.pdf");
    write_document(
        &input,
        &document(),
        Format::Pptx,
        &ConversionOptions::default(),
    )
    .unwrap();

    let catalog = Arc::new(Catalog::load().unwrap());
    let registry = Arc::new(EngineRegistry::native_only());
    let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
    let service = ConversionService::new(catalog, graph, registry, dir.path().join("work"), 60);
    let plan = service
        .plan(Format::Pptx, Format::Pdf, &ConversionOptions::default())
        .unwrap();
    assert_eq!(plan.steps[0].engine, "deckcraft");
    service
        .convert_file(
            &input,
            &output,
            Format::Pptx,
            Format::Pdf,
            &ConversionOptions::default(),
        )
        .await
        .unwrap();
    assert!(std::fs::read(output).unwrap().starts_with(b"%PDF"));
}

fn document() -> Document {
    let mut document = Document::new("DeckCraft spike");
    document.push(Block::Heading {
        level: 1,
        text: "DeckCraft PPTX".to_string(),
    });
    document.push(Block::Paragraph(
        "A spike for the DeckCraft PPTX-to-PDF engine.".to_string(),
    ));
    document
}
