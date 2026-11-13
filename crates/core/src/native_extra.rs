//! Additional pure-Rust adapters: DjVu, CBR/RAR, HEIC, and legacy Word.

use crate::document::pdf::images_to_pdf;
use crate::engine::{Engine, EngineContext, Support};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default)]
pub struct DjvuEngine;

#[async_trait]
impl Engine for DjvuEngine {
    fn id(&self) -> &'static str {
        "native-djvu"
    }
    fn name(&self) -> &'static str {
        "DocForge DjVu"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust DjVu rendering and PDF/text export"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        match (from, to) {
            (Format::Djvu, Format::Pdf) => Some(Support::new(8, "native DjVu renderer")),
            (Format::Djvu, Format::Txt | Format::Markdown) => {
                Some(Support::new(12, "native DjVu text layer"))
            }
            _ => None,
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let bytes = std::fs::read(ctx.input)?;
        let document = djvu_rs::DjVuDocument::parse(&bytes)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        match ctx.to {
            Format::Pdf => {
                let pdf = djvu_rs::pdf::djvu_to_pdf(&document)
                    .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
                std::fs::write(ctx.output, pdf)?;
            }
            Format::Txt | Format::Markdown => {
                let mut text = String::new();
                for index in 0..document.page_count() {
                    let page = document
                        .page(index)
                        .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
                    let Some(page_text) = page
                        .text()
                        .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?
                    else {
                        continue;
                    };
                    if !text.is_empty() {
                        text.push_str("\n\n");
                    }
                    text.push_str(&page_text);
                }
                if text.trim().is_empty() {
                    return Err(ConvertError::InvalidInput(
                        "DjVu document has no text layer".into(),
                    ));
                }
                if ctx.to == Format::Markdown {
                    let document = crate::document::Document {
                        title: None,
                        blocks: text
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .map(|line| crate::document::Block::Paragraph(line.trim().to_string()))
                            .collect(),
                    };
                    crate::document::write::write_document(
                        ctx.output,
                        &document,
                        Format::Markdown,
                        ctx.options,
                    )?;
                } else {
                    std::fs::write(ctx.output, text)?;
                }
            }
            other => {
                return Err(ConvertError::Unsupported {
                    from: Format::Djvu,
                    to: other,
                })
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct CbrEngine;

#[async_trait]
impl Engine for CbrEngine {
    fn id(&self) -> &'static str {
        "native-cbr"
    }
    fn name(&self) -> &'static str {
        "DocForge CBR/RAR"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust RAR/CBR extraction to PDF"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        (from == Format::Cbr && to == Format::Pdf)
            .then(|| Support::new(20, "native RAR extraction"))
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let archive = rars::ArchiveReader::read_path(ctx.input)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        let extracted = Mutex::new(Vec::<PathBuf>::new());
        let mut sequence = 0usize;
        archive
            .extract_to(None, |meta| {
                if meta.is_directory {
                    return Ok(Box::new(std::io::sink()));
                }
                let name = meta
                    .name_lossy()
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("page")
                    .to_string();
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("bin");
                sequence += 1;
                let path = ctx.workdir.join(format!(
                    "rar-{sequence:05}.{}",
                    extension.to_ascii_lowercase()
                ));
                let file = std::fs::File::create(&path)?;
                extracted.lock().unwrap().push(path);
                Ok(Box::new(file))
            })
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        let mut images = extracted.into_inner().unwrap();
        images.retain(|path| is_image(path));
        if images.is_empty() {
            return Err(ConvertError::InvalidInput(
                "CBR archive contains no supported images".into(),
            ));
        }
        images.sort();
        images_to_pdf(ctx.output, &images, ctx.options)?;
        Ok(())
    }
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "bmp" | "tif" | "tiff")
    )
}

#[derive(Default)]
pub struct HeicEngine;

#[async_trait]
impl Engine for HeicEngine {
    fn id(&self) -> &'static str {
        "native-heic"
    }
    fn name(&self) -> &'static str {
        "DocForge HEIC"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust HEIC/HEIF image decoding"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        (from == Format::Heic && matches!(to, Format::Pdf | Format::Jpg | Format::Png))
            .then(|| Support::new(10, "native HEIC decoder"))
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let bytes = std::fs::read(ctx.input)?;
        let decoded = heic_rs::decode(&bytes, &heic_rs::DecodeOptions::default())
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        let image = heic_image(&decoded)?;
        let temporary = ctx.workdir.join("heic-decoded.png");
        image
            .save_with_format(&temporary, image::ImageFormat::Png)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        if ctx.to == Format::Pdf {
            images_to_pdf(ctx.output, &[temporary], ctx.options)
        } else {
            crate::data::convert_image(&temporary, ctx.output, Format::Png, ctx.to, ctx.options)
        }
    }
}

fn heic_image(decoded: &heic_rs::Image) -> Result<image::DynamicImage> {
    use heic_rs::PixelLayout;
    let width = decoded.width;
    let height = decoded.height;
    let data = &decoded.data;
    let rgb = match decoded.layout {
        PixelLayout::Rgb8 => image::RgbImage::from_raw(width, height, data.clone()),
        PixelLayout::Rgba8 => image::RgbaImage::from_raw(width, height, data.clone())
            .map(|image| image::DynamicImage::ImageRgba8(image).to_rgb8()),
        PixelLayout::Bgr8 => image::RgbImage::from_raw(
            width,
            height,
            data.chunks(3)
                .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
                .collect(),
        ),
        PixelLayout::Bgra8 => image::RgbImage::from_raw(
            width,
            height,
            data.chunks(4)
                .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
                .collect(),
        ),
        PixelLayout::Gray8 => image::GrayImage::from_raw(width, height, data.clone())
            .map(|image| image::DynamicImage::ImageLuma8(image).to_rgb8()),
        _ => {
            return Err(ConvertError::engine(
                "native-heic",
                "unsupported 16-bit HEIC pixel layout",
            ))
        }
    }
    .ok_or_else(|| ConvertError::engine("native-heic", "decoded HEIC buffer has invalid size"))?;
    Ok(image::DynamicImage::ImageRgb8(rgb))
}

#[derive(Default)]
pub struct LegacyDocEngine;

#[async_trait]
impl Engine for LegacyDocEngine {
    fn id(&self) -> &'static str {
        "native-doc"
    }
    fn name(&self) -> &'static str {
        "DocForge legacy Word"
    }
    fn description(&self) -> &'static str {
        "Pure-Rust Word 97-2003 parser and DOCX serializer"
    }
    fn priority(&self) -> u32 {
        2
    }
    fn available(&self) -> bool {
        true
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        match (from, to) {
            (Format::Doc, Format::Docx) => Some(Support::new(8, "native legacy Word parser")),
            (Format::Doc, Format::Pdf) => {
                Some(Support::new(4, "native legacy Word parser + Typst PDF").layout())
            }
            _ => None,
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let bytes = std::fs::read(ctx.input)?;
        let model = rust_docx::parse_document_model(&bytes)
            .map_err(|error| ConvertError::engine(self.id(), error))?;
        let output = rust_docx::serialize::serialize_docx(&model, None)
            .map_err(|error| ConvertError::engine(self.id(), error))?;
        if ctx.to == Format::Docx {
            std::fs::write(ctx.output, output)?;
            return Ok(());
        }
        let temporary = ctx.workdir.join("legacy-doc.docx");
        std::fs::write(&temporary, output)?;
        if let Err(error) =
            crate::office::office_to_pdf(&temporary, ctx.output, Format::Docx, ctx.options)
        {
            tracing::warn!(
                engine = "native-doc",
                error = %error,
                "Typst renderer rejected legacy DOC; using structural PDF fallback"
            );
            let document =
                crate::document::read::read_document(&temporary, Format::Docx, ctx.options)?;
            crate::document::write::write_document(
                ctx.output,
                &document,
                Format::Pdf,
                ctx.options,
            )?;
        }
        Ok(())
    }
}
