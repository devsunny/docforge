//! Phase 2 pure-Rust adapters: MOBI/EPUB, Apple iWork, and PostScript.

use crate::document::pdf::images_to_pdf;
use crate::document::read::read_document;
use crate::document::write::write_document;
use crate::document::{Block, Document};
use crate::engine::{Engine, EngineContext, Support};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use async_trait::async_trait;

#[derive(Default)]
pub struct EbookEngine;

#[async_trait]
impl Engine for EbookEngine {
    fn id(&self) -> &'static str {
        "native-ebook"
    }
    fn name(&self) -> &'static str {
        "DocForge e-book"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust MOBI to EPUB and EPUB to MOBI conversion"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        match (from, to) {
            (Format::Mobi, Format::Epub) => Some(Support::new(8, "native MOBI parser")),
            (Format::Epub, Format::Mobi) => Some(Support::new(10, "native Kindle writer")),
            _ => None,
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        match (ctx.from, ctx.to) {
            (Format::Mobi, Format::Epub) => {
                let book = mobi::Mobi::from_path(ctx.input)
                    .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
                let title = book.title();
                let html = book.content_as_string_lossy();
                if html.trim().is_empty() {
                    return Err(ConvertError::InvalidInput(
                        "MOBI book has no readable content".into(),
                    ));
                }
                let temp = ctx.workdir.join("mobi-content.html");
                std::fs::write(&temp, html)?;
                let mut document = read_document(&temp, Format::Html, ctx.options)?;
                if document.title.is_none() && !title.is_empty() {
                    document.title = Some(title);
                }
                write_document(ctx.output, &document, Format::Epub, ctx.options)?;
            }
            (Format::Epub, Format::Mobi) => {
                epub3_kindle::file::convert(
                    ctx.input,
                    ctx.output,
                    &epub3_kindle::ConvertOptions::default(),
                )
                .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
            }
            (from, to) => return Err(ConvertError::Unsupported { from, to }),
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct IworkEngine;

#[async_trait]
impl Engine for IworkEngine {
    fn id(&self) -> &'static str {
        "native-iwork"
    }
    fn name(&self) -> &'static str {
        "DocForge iWork"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust Pages, Numbers, and Keynote content extraction"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        match (from, to) {
            (Format::Pages, Format::Docx | Format::Pdf | Format::Txt | Format::Markdown) => {
                Some(Support::new(14, "native Pages text model"))
            }
            (Format::Numbers, Format::Xlsx | Format::Pdf | Format::Csv) => {
                Some(Support::new(14, "native Numbers table model"))
            }
            (Format::Key, Format::Pptx | Format::Pdf) => {
                Some(Support::new(16, "native Keynote slide text"))
            }
            _ => None,
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let document = iwork::Document::open(ctx.input)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        match ctx.from {
            Format::Pages => {
                let mut pages = Document::new(
                    ctx.input
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("Pages document"),
                );
                for storage in document.text_storages() {
                    let text = storage.text.trim();
                    if !text.is_empty() {
                        pages.push(Block::Paragraph(text.to_string()));
                    }
                }
                if pages.is_empty() {
                    return Err(ConvertError::InvalidInput(
                        "Pages document has no readable text".into(),
                    ));
                }
                write_document(ctx.output, &pages, ctx.to, ctx.options)?;
            }
            Format::Numbers => {
                let mut workbook = crate::data::Workbook::default();
                for table in document.tables() {
                    let rows = table.to_rows();
                    if rows.is_empty() {
                        continue;
                    }
                    let mut sheet = crate::data::Sheet {
                        name: if table.name.is_empty() {
                            format!("Sheet{}", workbook.sheets.len() + 1)
                        } else {
                            table.name.clone()
                        },
                        headers: rows.first().cloned().unwrap_or_default(),
                        rows: rows.into_iter().skip(1).collect(),
                    };
                    if table.header_rows == 0 {
                        sheet.rows.insert(0, sheet.headers.clone());
                        sheet.headers = Vec::new();
                    }
                    workbook.sheets.push(sheet);
                }
                if workbook.sheets.is_empty() {
                    return Err(ConvertError::InvalidInput(
                        "Numbers document has no readable tables".into(),
                    ));
                }
                crate::data::write_workbook(ctx.output, &workbook, ctx.to, ctx.options)?;
            }
            Format::Key => {
                let mut deck = Document::new(
                    ctx.input
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("Keynote deck"),
                );
                for (index, slide) in document.slides().into_iter().enumerate() {
                    if index > 0 {
                        deck.push(Block::PageBreak);
                    }
                    let title = slide.title_text().trim();
                    let body = slide.body_text().trim();
                    if !title.is_empty() {
                        deck.push(Block::Heading {
                            level: 1,
                            text: title.to_string(),
                        });
                    }
                    if !body.is_empty() {
                        for line in body.lines().filter(|line| !line.trim().is_empty()) {
                            deck.push(Block::Paragraph(line.trim().to_string()));
                        }
                    }
                }
                if deck.is_empty() {
                    return Err(ConvertError::InvalidInput(
                        "Keynote deck has no readable slide text".into(),
                    ));
                }
                write_document(ctx.output, &deck, ctx.to, ctx.options)?;
            }
            other => {
                return Err(ConvertError::Unsupported {
                    from: other,
                    to: ctx.to,
                })
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct PostScriptEngine;

#[async_trait]
impl Engine for PostScriptEngine {
    fn id(&self) -> &'static str {
        "native-postscript"
    }
    fn name(&self) -> &'static str {
        "DocForge PostScript"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust PostScript Level 3 interpreter"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        (from == Format::Ps && matches!(to, Format::Pdf | Format::Png))
            .then(|| Support::new(10, "native PostScript interpreter"))
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let bytes = std::fs::read(ctx.input)?;
        let dpi = f64::from(ctx.options.pdf.dpi.max(72));
        let pages = stet::Interpreter::new()
            .render(&bytes, dpi)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        if pages.is_empty() {
            return Err(ConvertError::InvalidInput(
                "PostScript produced no pages".into(),
            ));
        }
        let mut images = Vec::new();
        for (index, page) in pages.iter().enumerate() {
            let image = image::RgbaImage::from_raw(page.width, page.height, page.rgba.clone())
                .ok_or_else(|| {
                    ConvertError::engine(self.id(), "PostScript page buffer has invalid size")
                })?;
            let path = ctx.workdir.join(format!("ps-page-{index:04}.png"));
            image
                .save_with_format(&path, image::ImageFormat::Png)
                .map_err(|error| ConvertError::engine(self.id(), error.to_string()))?;
            images.push(path);
        }
        match ctx.to {
            Format::Pdf => images_to_pdf(ctx.output, &images, ctx.options)?,
            Format::Png => {
                std::fs::copy(&images[0], ctx.output)?;
            }
            other => {
                return Err(ConvertError::Unsupported {
                    from: Format::Ps,
                    to: other,
                })
            }
        }
        Ok(())
    }
}
