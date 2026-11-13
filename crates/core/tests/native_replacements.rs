use docforge_core::document::write::write_document;
use docforge_core::document::{Block, Document};
use docforge_core::format::Format;
use docforge_core::ocr::ocr_convert;
use docforge_core::office::office_to_pdf;
use docforge_core::options::ConversionOptions;
use docforge_core::pdf_raster::pdf_to_image;

fn document() -> Document {
    let mut document = Document::new("Native replacement test");
    document.push(Block::Heading {
        level: 1,
        text: "DocForge".to_string(),
    });
    document.push(Block::Paragraph(
        "Pure-Rust Office and PDF conversion.".to_string(),
    ));
    document
}

#[test]
fn office2pdf_renders_docx() {
    let dir = tempfile::tempdir().unwrap();
    let docx = dir.path().join("input.docx");
    let pdf = dir.path().join("output.pdf");
    write_document(
        &docx,
        &document(),
        Format::Docx,
        &ConversionOptions::default(),
    )
    .unwrap();
    office_to_pdf(&docx, &pdf, Format::Docx, &ConversionOptions::default()).unwrap();
    let bytes = std::fs::read(&pdf).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "not a PDF");
    assert!(bytes.len() > 500, "PDF is unexpectedly small");
}

#[test]
fn native_ocr_reads_rendered_text() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("input.pdf");
    let png = dir.path().join("page.png");
    let txt = dir.path().join("page.txt");
    write_document(
        &pdf,
        &document(),
        Format::Pdf,
        &ConversionOptions::default(),
    )
    .unwrap();
    pdf_to_image(&pdf, &png, Format::Png, &ConversionOptions::default()).unwrap();
    ocr_convert(
        &png,
        &txt,
        Format::Png,
        Format::Txt,
        &ConversionOptions::default(),
    )
    .unwrap();
    let text = std::fs::read_to_string(&txt).unwrap();
    assert!(
        text.to_ascii_uppercase().contains("DOCFORGE"),
        "OCR text: {text:?}"
    );
}

#[test]
fn pdf_render_rasterizes_png() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("input.pdf");
    let png = dir.path().join("output.png");
    write_document(
        &pdf,
        &document(),
        Format::Pdf,
        &ConversionOptions::default(),
    )
    .unwrap();
    pdf_to_image(&pdf, &png, Format::Png, &ConversionOptions::default()).unwrap();
    let bytes = std::fs::read(&png).unwrap();
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    assert!(bytes.len() > 500, "PNG is unexpectedly small");
}
