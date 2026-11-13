//! Native OCR using `ocrs` and embedded ONNX models.
//!
//! The models currently recognize the Latin alphabet (English). Tesseract
//! remains available for other languages and higher accuracy; the planner
//! prefers it when installed because this engine advertises a higher cost.

use crate::document::write::write_document;
use crate::document::{Block, Document};
use crate::engine::{Engine, EngineContext, Support};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use crate::pdf_raster;
use async_trait::async_trait;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use std::path::Path;
use std::sync::OnceLock;

static ENGINE: OnceLock<std::result::Result<OcrEngine, String>> = OnceLock::new();

fn engine() -> Result<&'static OcrEngine> {
    let result = ENGINE.get_or_init(|| {
        let detection =
            rten::Model::load_static_slice(include_bytes!("../assets/ocr/text-detection.onnx"))
                .map_err(|error| format!("cannot load OCR detection model: {error}"))?;
        let recognition =
            rten::Model::load_static_slice(include_bytes!("../assets/ocr/text-recognition.onnx"))
                .map_err(|error| format!("cannot load OCR recognition model: {error}"))?;
        OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            ..Default::default()
        })
        .map_err(|error| format!("cannot initialize OCR: {error}"))
    });
    result
        .as_ref()
        .map_err(|error| ConvertError::engine("ocrs", error.clone()))
}

fn image_text(image: &image::DynamicImage) -> Result<String> {
    let rgb = image.to_rgb8();
    let source = ImageSource::from_bytes(rgb.as_raw(), rgb.dimensions())
        .map_err(|error| ConvertError::engine("ocrs", error.to_string()))?;
    let engine = engine()?;
    let input = engine
        .prepare_input(source)
        .map_err(|error| ConvertError::engine("ocrs", error.to_string()))?;
    engine
        .get_text(&input)
        .map_err(|error| ConvertError::engine("ocrs", error.to_string()))
}

#[derive(Default)]
pub struct NativeOcrEngine;

#[async_trait]
impl Engine for NativeOcrEngine {
    fn id(&self) -> &'static str {
        "native-ocr"
    }

    fn name(&self) -> &'static str {
        "DocForge native OCR"
    }

    fn description(&self) -> &'static str {
        "Pure-Rust OCR for English/Latin images and scanned PDFs (ocrs models)"
    }

    fn priority(&self) -> u32 {
        1
    }

    fn available(&self) -> bool {
        true
    }

    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        let cost = match (from, to) {
            (
                Format::Jpg | Format::Png | Format::Tiff | Format::Webp | Format::Bmp,
                Format::Txt | Format::Pdf,
            ) => 14,
            (Format::Pdf, Format::Txt) => 20,
            _ => return None,
        };
        Some(Support::new(cost, "native OCR (ocrs, Latin/English models)").ocr())
    }

    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        ocr_convert(ctx.input, ctx.output, ctx.from, ctx.to, ctx.options)
    }
}

pub fn ocr_convert(
    input: &Path,
    output: &Path,
    from: Format,
    to: Format,
    options: &ConversionOptions,
) -> Result<()> {
    if !matches!(
        options.ocr_language.to_ascii_lowercase().as_str(),
        "eng" | "en"
    ) {
        return Err(ConvertError::engine(
            "ocrs",
            format!(
                "native OCR currently supports English/Latin only, not '{}'; install Tesseract for other languages",
                options.ocr_language
            ),
        ));
    }
    let text = match from {
        Format::Pdf => {
            let pages = pdf_raster::pdf_page_count(input)?;
            let last = options
                .max_pages
                .map(|max| max as usize)
                .unwrap_or(pages)
                .min(pages);
            let mut text = String::new();
            for page in 1..=last {
                let image = pdf_raster::pdf_page_image(input, page as u32, options.pdf.dpi)?;
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&image_text(&image)?);
            }
            text
        }
        _ => {
            let image = image::open(input).map_err(|error| {
                ConvertError::InvalidInput(format!("cannot decode image: {error}"))
            })?;
            image_text(&image)?
        }
    };
    if text.trim().is_empty() {
        return Err(ConvertError::InvalidInput(
            "OCR found no text in the input".into(),
        ));
    }
    match to {
        Format::Txt => {
            std::fs::write(output, text)?;
        }
        Format::Pdf => {
            let mut document = Document::new("OCR");
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                document.push(Block::Paragraph(line.trim().to_string()));
            }
            write_document(output, &document, Format::Pdf, options)?;
        }
        other => {
            return Err(ConvertError::Unsupported { from, to: other });
        }
    }
    Ok(())
}
