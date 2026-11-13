use docforge_core::catalog::Catalog;
use docforge_core::engine::EngineRegistry;
use docforge_core::format::Format;
use docforge_core::graph::ConversionGraph;
use docforge_core::options::ConversionOptions;
use docforge_core::service::ConversionService;
use std::path::Path;
use std::sync::Arc;

fn service() -> (ConversionService, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Catalog::load().unwrap());
    let registry = Arc::new(EngineRegistry::native_only());
    let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
    let service = ConversionService::new(catalog, graph, registry, dir.path().join("work"), 120);
    (service, dir)
}

async fn convert(
    service: &ConversionService,
    input: &Path,
    output: &Path,
    from: Format,
    to: Format,
) -> docforge_core::error::Result<()> {
    service
        .convert_file(input, output, from, to, &ConversionOptions::default())
        .await
        .map(|_| ())
}

#[tokio::test]
async fn djvu_to_pdf_native() {
    let (service, dir) = service();
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.djvu");
    let output = dir.path().join("djvu.pdf");
    convert(&service, &input, &output, Format::Djvu, Format::Pdf)
        .await
        .unwrap();
    let bytes = std::fs::read(output).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
}

#[tokio::test]
async fn heic_to_png_native() {
    let (service, dir) = service();
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.heic");
    let output = dir.path().join("heic.png");
    convert(&service, &input, &output, Format::Heic, Format::Png)
        .await
        .unwrap();
    let image = image::open(output).unwrap();
    assert_eq!((image.width(), image.height()), (64, 64));
}

#[tokio::test]
async fn legacy_doc_to_docx_native() {
    let (service, dir) = service();
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy.doc");
    let output = dir.path().join("legacy.docx");
    convert(&service, &input, &output, Format::Doc, Format::Docx)
        .await
        .unwrap();
    let bytes = std::fs::read(output).unwrap();
    assert!(bytes.starts_with(b"PK"));

    let pdf = dir.path().join("legacy.pdf");
    convert(&service, &input, &pdf, Format::Doc, Format::Pdf)
        .await
        .unwrap();
    assert!(std::fs::read(pdf).unwrap().starts_with(b"%PDF"));
}

#[tokio::test]
async fn cbr_to_pdf_native() {
    let (service, dir) = service();
    let input = dir.path().join("comic.cbr");
    let image = image::RgbImage::from_fn(32, 32, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 8) as u8, 128])
    });
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut builder = rars::Builder::new(rars::ArchiveVersion::Rar50);
    builder
        .add_bytes(b"page01.png".to_vec(), png, None, None)
        .unwrap();
    std::fs::write(&input, builder.to_bytes().unwrap()).unwrap();
    let output = dir.path().join("comic.pdf");
    convert(&service, &input, &output, Format::Cbr, Format::Pdf)
        .await
        .unwrap();
    assert!(std::fs::read(output).unwrap().starts_with(b"%PDF"));
}

#[tokio::test]
async fn epub_mobi_roundtrip_native() {
    let (service, dir) = service();
    let mut document = docforge_core::document::Document::new("E-book test");
    document.push(docforge_core::document::Block::Paragraph(
        "A pure-Rust e-book conversion.".to_string(),
    ));
    let epub = dir.path().join("book.epub");
    docforge_core::document::write::write_document(
        &epub,
        &document,
        Format::Epub,
        &ConversionOptions::default(),
    )
    .unwrap();
    let mobi = dir.path().join("book.mobi");
    convert(&service, &epub, &mobi, Format::Epub, Format::Mobi)
        .await
        .unwrap();
    assert!(mobi::Mobi::from_path(&mobi).is_ok());

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alice.mobi");
    let epub_back = dir.path().join("alice.epub");
    convert(&service, &source, &epub_back, Format::Mobi, Format::Epub)
        .await
        .unwrap();
    assert!(std::fs::read(epub_back).unwrap().starts_with(b"PK"));
}

#[tokio::test]
async fn pages_to_docx_native() {
    let (service, dir) = service();
    let input = dir.path().join("report.pages");
    {
        let mut pages = iwork::Document::new(iwork::Kind::Pages).unwrap();
        pages
            .body_mut()
            .unwrap()
            .append("DocForge Pages test")
            .unwrap();
        pages.save(&input).unwrap();
    }
    let output = dir.path().join("report.docx");
    convert(&service, &input, &output, Format::Pages, Format::Docx)
        .await
        .unwrap();
    assert!(std::fs::read(output).unwrap().starts_with(b"PK"));
}

#[tokio::test]
async fn numbers_to_xlsx_native() {
    let (service, dir) = service();
    let input = dir.path().join("sales.numbers");
    {
        let mut numbers = iwork::Document::new_spreadsheet("Sales", "Q1", 3, 2).unwrap();
        let mut table = numbers.table_mut("Q1").unwrap();
        table.set("A1", "Region").unwrap();
        table.set("B1", "Units").unwrap();
        table.set("A2", "North").unwrap();
        table.set("B2", 10).unwrap();
        numbers.save(&input).unwrap();
    }
    let output = dir.path().join("sales.xlsx");
    convert(&service, &input, &output, Format::Numbers, Format::Xlsx)
        .await
        .unwrap();
    assert!(std::fs::read(output).unwrap().starts_with(b"PK"));
}

#[tokio::test]
async fn postscript_to_pdf_native() {
    let (service, dir) = service();
    let input = dir.path().join("page.ps");
    std::fs::write(
        &input,
        "%!PS-Adobe-3.0\n/Helvetica findfont 24 scalefont setfont\n72 720 moveto (DocForge) show\nshowpage\n",
    )
    .unwrap();
    let output = dir.path().join("page.pdf");
    convert(&service, &input, &output, Format::Ps, Format::Pdf)
        .await
        .unwrap();
    assert!(std::fs::read(output).unwrap().starts_with(b"%PDF"));
}
