//! PDF output built on `printpdf` with an embedded Unicode font.
//!
//! The native layout engine is intentionally simple: headings, paragraphs,
//! lists, quotes, code blocks, tables, and page breaks. For exact visual
//! fidelity use LibreOffice, Pandoc, or Chromium when installed.

use super::write::page_dimensions;
use super::{Block, Document};
use crate::error::{ConvertError, Result};
use crate::options::ConversionOptions;
use printpdf::{
    Color, Greyscale, Image, ImageTransform, IndirectFontRef, Line, Mm, PdfDocument,
    PdfDocumentReference, PdfLayerReference, Point,
};
use std::fs::File;
use std::io::{BufWriter, Cursor};
use std::path::Path;

const FONT: &[u8] = include_bytes!("../../assets/RobotoMedium.ttf");

pub fn write_pdf(path: &Path, document: &Document, options: &ConversionOptions) -> Result<()> {
    let (width, height) = page_dimensions(options);
    let title = document.title.as_deref().unwrap_or("Converted document");
    let (doc, page, layer) = PdfDocument::new(title, Mm(width), Mm(height), "Content");
    let font = doc
        .add_external_font(Cursor::new(FONT))
        .map_err(|error| ConvertError::engine("native-pdf", error))?;
    let layer = doc.get_page(page).get_layer(layer);
    let mut layout = Layout {
        doc,
        font,
        page_width: width,
        page_height: height,
        margin: options.pdf.margin_mm.max(8.0),
        y: height - options.pdf.margin_mm.max(8.0),
        page_number: 1,
        layer,
        footer_drawn: false,
    };

    layout.header(title);
    for block in &document.blocks {
        match block {
            Block::Heading { level, text } => layout.heading(*level, text),
            Block::Paragraph(text) => layout.paragraph(text, 10.5, 0.0),
            Block::List { ordered, items } => {
                for (index, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}.", index + 1)
                    } else {
                        "•".to_string()
                    };
                    layout.list_item(&marker, item);
                }
            }
            Block::Quote(text) => layout.quote(text),
            Block::Code { text, .. } => layout.code(text),
            Block::Table { headers, rows } => layout.table(headers, rows),
            Block::PageBreak => layout.new_page(),
        }
    }
    layout.footer();
    let file = File::create(path)?;
    layout
        .doc
        .save(&mut BufWriter::new(file))
        .map_err(|error| ConvertError::engine("native-pdf", error))?;
    Ok(())
}

struct Layout {
    doc: PdfDocumentReference,
    font: IndirectFontRef,
    page_width: f32,
    page_height: f32,
    margin: f32,
    y: f32,
    page_number: usize,
    layer: PdfLayerReference,
    footer_drawn: bool,
}

impl Layout {
    fn usable_width(&self) -> f32 {
        self.page_width - self.margin * 2.0
    }

    fn new_page(&mut self) {
        self.footer();
        let (page, layer) = self
            .doc
            .add_page(Mm(self.page_width), Mm(self.page_height), "Content");
        self.layer = self.doc.get_page(page).get_layer(layer);
        self.page_number += 1;
        self.y = self.page_height - self.margin;
        self.footer_drawn = false;
    }

    fn ensure(&mut self, needed: f32) {
        if self.y - needed < self.margin {
            self.new_page();
        }
    }

    fn header(&mut self, title: &str) {
        self.layer
            .set_fill_color(Color::Rgb(printpdf::Rgb::new(0.11, 0.13, 0.16, None)));
        let lines = wrap_text(title, 18.0, self.usable_width());
        for line in lines {
            self.ensure(10.0);
            self.layer
                .use_text(line, 18.0, Mm(self.margin), Mm(self.y), &self.font);
            self.y -= 8.0;
        }
        self.y -= 4.0;
    }

    fn heading(&mut self, level: u8, text: &str) {
        let size = match level {
            1 => 19.0,
            2 => 15.5,
            3 => 13.0,
            _ => 11.5,
        };
        let leading = size * 0.42;
        self.y -= size * 0.15;
        self.ensure(leading * 2.0);
        self.layer
            .set_fill_color(Color::Rgb(printpdf::Rgb::new(0.09, 0.12, 0.16, None)));
        for line in wrap_text(text, size, self.usable_width()) {
            self.ensure(leading);
            self.layer
                .use_text(line, size, Mm(self.margin), Mm(self.y), &self.font);
            self.y -= leading;
        }
        self.y -= leading * 0.3;
    }

    fn paragraph(&mut self, text: &str, size: f32, indent: f32) {
        let leading = size * 0.46;
        let width = self.usable_width() - indent;
        self.layer
            .set_fill_color(Color::Rgb(printpdf::Rgb::new(0.14, 0.16, 0.19, None)));
        for line in wrap_text(text, size, width) {
            self.ensure(leading);
            self.layer
                .use_text(line, size, Mm(self.margin + indent), Mm(self.y), &self.font);
            self.y -= leading;
        }
        self.y -= leading * 0.35;
    }

    fn list_item(&mut self, marker: &str, text: &str) {
        let size = 10.5;
        let leading = size * 0.46;
        let indent = 7.0;
        let lines = wrap_text(text, size, self.usable_width() - indent);
        self.ensure(leading);
        self.layer
            .set_fill_color(Color::Rgb(printpdf::Rgb::new(0.14, 0.16, 0.19, None)));
        self.layer
            .use_text(marker, size, Mm(self.margin + 1.5), Mm(self.y), &self.font);
        for (index, line) in lines.into_iter().enumerate() {
            if index > 0 {
                self.ensure(leading);
            }
            self.layer
                .use_text(line, size, Mm(self.margin + indent), Mm(self.y), &self.font);
            self.y -= leading;
        }
        self.y -= 1.0;
    }

    fn quote(&mut self, text: &str) {
        let size = 10.0;
        let leading = size * 0.46;
        let indent = 8.0;
        self.ensure(leading);
        self.layer
            .set_outline_color(Color::Greyscale(Greyscale::new(0.7, None)));
        self.layer.set_outline_thickness(1.2);
        self.layer.add_line(Line {
            points: vec![
                (Point::new(Mm(self.margin), Mm(self.y + 2.0)), false),
                (
                    Point::new(Mm(self.margin), Mm(self.y - leading * 2.0)),
                    false,
                ),
            ],
            is_closed: false,
        });
        self.layer
            .set_fill_color(Color::Greyscale(Greyscale::new(0.38, None)));
        for line in wrap_text(text, size, self.usable_width() - indent) {
            self.ensure(leading);
            self.layer
                .use_text(line, size, Mm(self.margin + indent), Mm(self.y), &self.font);
            self.y -= leading;
        }
        self.y -= leading * 0.3;
    }

    fn code(&mut self, text: &str) {
        let size = 8.5;
        let leading = size * 0.46;
        self.y -= 2.0;
        self.layer
            .set_fill_color(Color::Greyscale(Greyscale::new(0.18, None)));
        for line in wrap_text(text, size, self.usable_width() - 5.0) {
            self.ensure(leading);
            self.layer
                .use_text(line, size, Mm(self.margin + 4.0), Mm(self.y), &self.font);
            self.y -= leading;
        }
        self.y -= leading * 0.5;
    }

    fn table(&mut self, headers: &[String], rows: &[Vec<String>]) {
        let columns = headers
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if columns == 0 {
            return;
        }
        let size = 8.2;
        let leading = size * 0.46;
        let col_width = self.usable_width() / columns as f32;
        let row_gap = 2.0;

        let render_row = |layout: &mut Layout, cells: &[String], bold: bool| {
            let normalized: Vec<String> = (0..columns)
                .map(|index| cells.get(index).cloned().unwrap_or_default())
                .collect();
            let wrapped: Vec<Vec<String>> = normalized
                .iter()
                .map(|cell| wrap_text(cell, size, col_width - 3.0))
                .collect();
            let max_lines = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
            let row_height = max_lines as f32 * leading + row_gap;
            layout.ensure(row_height);
            let row_top = layout.y + leading;
            layout
                .layer
                .set_outline_color(Color::Greyscale(Greyscale::new(0.78, None)));
            layout.layer.set_outline_thickness(0.3);
            layout.layer.add_line(Line {
                points: vec![
                    (Point::new(Mm(layout.margin), Mm(row_top)), false),
                    (
                        Point::new(Mm(layout.margin + layout.usable_width()), Mm(row_top)),
                        false,
                    ),
                ],
                is_closed: false,
            });
            for (column, lines) in wrapped.iter().enumerate() {
                let x = layout.margin + column as f32 * col_width;
                if column > 0 {
                    layout.layer.add_line(Line {
                        points: vec![
                            (Point::new(Mm(x), Mm(row_top)), false),
                            (Point::new(Mm(x), Mm(layout.y - leading)), false),
                        ],
                        is_closed: false,
                    });
                }
                layout.layer.set_fill_color(Color::Rgb(printpdf::Rgb::new(
                    if bold { 0.05 } else { 0.16 },
                    if bold { 0.07 } else { 0.18 },
                    if bold { 0.09 } else { 0.21 },
                    None,
                )));
                let mut y = layout.y;
                for line in lines {
                    layout
                        .layer
                        .use_text(line, size, Mm(x + 1.5), Mm(y), &layout.font);
                    y -= leading;
                }
            }
            layout.y -= row_height;
        };

        if !headers.is_empty() {
            render_row(self, headers, true);
        }
        for row in rows {
            render_row(self, row, false);
        }
        self.y -= leading * 0.5;
    }

    fn footer(&mut self) {
        if self.footer_drawn {
            return;
        }
        self.layer
            .set_fill_color(Color::Greyscale(Greyscale::new(0.55, None)));
        self.layer.use_text(
            format!("DocForge · page {}", self.page_number),
            7.5,
            Mm(self.margin),
            Mm(self.margin * 0.45),
            &self.font,
        );
        self.footer_drawn = true;
    }
}

fn wrap_text(text: &str, size: f32, width_mm: f32) -> Vec<String> {
    let char_width_mm = (size * 0.5 * 0.352_778).max(0.8);
    let max_chars = ((width_mm / char_width_mm).floor() as usize).max(8);
    let mut lines = Vec::new();
    for logical in text.replace('\r', "").split('\n') {
        if logical.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in logical.split_whitespace() {
            if current.is_empty() {
                if word.chars().count() > max_chars {
                    let mut chunk = String::new();
                    for ch in word.chars() {
                        chunk.push(ch);
                        if chunk.chars().count() >= max_chars {
                            lines.push(std::mem::take(&mut chunk));
                        }
                    }
                    current = chunk;
                } else {
                    current.push_str(word);
                }
                continue;
            }
            if current.chars().count() + 1 + word.chars().count() <= max_chars {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Create a PDF with one decoded image per page, fitted to the page.
pub fn images_to_pdf(
    path: &Path,
    images: &[std::path::PathBuf],
    options: &ConversionOptions,
) -> Result<()> {
    if images.is_empty() {
        return Err(ConvertError::InvalidInput(
            "no images were found for PDF output".into(),
        ));
    }
    let (width, height) = page_dimensions(options);
    let (doc, first_page, first_layer) =
        PdfDocument::new("Converted images", Mm(width), Mm(height), "Images");
    let first_layer = doc.get_page(first_page).get_layer(first_layer);
    for (index, image_path) in images.iter().enumerate() {
        let layer = if index == 0 {
            first_layer.clone()
        } else {
            let (page, layer) = doc.add_page(Mm(width), Mm(height), "Images");
            doc.get_page(page).get_layer(layer)
        };
        let dynamic = image::open(image_path)
            .map_err(|error| ConvertError::InvalidInput(format!("cannot decode image: {error}")))?;
        let pixel_width = dynamic.width() as f32;
        let pixel_height = dynamic.height() as f32;
        let natural_w = pixel_width / 300.0 * 25.4;
        let natural_h = pixel_height / 300.0 * 25.4;
        let usable_w = width - options.pdf.margin_mm * 2.0;
        let usable_h = height - options.pdf.margin_mm * 2.0;
        let scale = (usable_w / natural_w).min(usable_h / natural_h).min(1.0);
        let draw_w = natural_w * scale;
        let draw_h = natural_h * scale;
        let x = (width - draw_w) / 2.0;
        let y = (height - draw_h) / 2.0;
        let image = Image::from_dynamic_image(&dynamic);
        image.add_to_layer(
            layer,
            ImageTransform {
                translate_x: Some(Mm(x)),
                translate_y: Some(Mm(y)),
                scale_x: Some(scale),
                scale_y: Some(scale),
                dpi: Some(300.0),
                ..Default::default()
            },
        );
    }
    let file = File::create(path)?;
    doc.save(&mut BufWriter::new(file))
        .map_err(|error| ConvertError::engine("native-pdf-images", error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn writes_a_pdf_header() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("out.pdf");
        let document = Document::from_plain_text("Hello PDF");
        write_pdf(&path, &document, &ConversionOptions::default()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > 500);
    }
}
