//! Pure-Rust PDF rasterization backed by `pdf-render`/Vello CPU.

use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use std::io::Write;
use std::path::Path;

pub fn pdf_page_count(input: &Path) -> Result<usize> {
    let bytes = std::fs::read(input)?;
    let pdf = pdf_render::pdf_syntax::Pdf::new(bytes)
        .map_err(|error| ConvertError::engine("pdf-render", format!("{error:?}")))?;
    Ok(pdf.pages().len())
}

pub fn pdf_page_image(input: &Path, page_number: u32, dpi: u16) -> Result<image::DynamicImage> {
    let bytes = std::fs::read(input)?;
    let pdf = pdf_render::pdf_syntax::Pdf::new(bytes)
        .map_err(|error| ConvertError::engine("pdf-render", format!("{error:?}")))?;
    let page = pdf
        .pages()
        .get(page_number.saturating_sub(1) as usize)
        .ok_or_else(|| {
            ConvertError::InvalidInput(format!(
                "PDF has no page {page_number} (requested page {page_number})"
            ))
        })?;
    let scale = f32::from(dpi) / 72.0;
    let settings = pdf_render::RenderSettings {
        x_scale: scale,
        y_scale: scale,
        ..Default::default()
    };
    let pixmap = pdf_render::render(
        page,
        &pdf_render::pdf_interpret::InterpreterSettings::default(),
        &settings,
    );
    let png = pixmap
        .into_png()
        .map_err(|error| ConvertError::engine("pdf-render", error.to_string()))?;
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|error| ConvertError::engine("pdf-render", error.to_string()))?;
    Ok(flatten_on_white(image))
}

/// PDF pages commonly render with a transparent background. Flattening onto
/// white before encoding prevents black backgrounds in JPEG/TIFF and makes the
/// result usable by OCR.
fn flatten_on_white(image: image::DynamicImage) -> image::DynamicImage {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(width, height);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u16::from(pixel[3]);
        let inverse = 255 - alpha;
        let blend =
            |channel: u8| -> u8 { ((u16::from(channel) * alpha + 255 * inverse) / 255) as u8 };
        rgb.put_pixel(
            x,
            y,
            image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]),
        );
    }
    image::DynamicImage::ImageRgb8(rgb)
}

pub fn pdf_to_image(
    input: &Path,
    output: &Path,
    to: Format,
    options: &ConversionOptions,
) -> Result<()> {
    let image = pdf_page_image(input, options.page.unwrap_or(1), options.pdf.dpi)?;
    write_image(output, to, image, options)
}

fn write_image(
    output: &Path,
    to: Format,
    image: image::DynamicImage,
    options: &ConversionOptions,
) -> Result<()> {
    match to {
        Format::Jpg => {
            let file = std::fs::File::create(output)?;
            let mut writer = std::io::BufWriter::new(file);
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut writer,
                options.image.quality,
            );
            image
                .to_rgb8()
                .write_with_encoder(encoder)
                .map_err(|error| ConvertError::engine("pdf-render", error.to_string()))?;
            writer.flush()?;
        }
        Format::Png => image
            .save_with_format(output, image::ImageFormat::Png)
            .map_err(|error| ConvertError::engine("pdf-render", error.to_string()))?,
        Format::Tiff => image
            .save_with_format(output, image::ImageFormat::Tiff)
            .map_err(|error| ConvertError::engine("pdf-render", error.to_string()))?,
        other => {
            return Err(ConvertError::Unsupported {
                from: Format::Pdf,
                to: other,
            })
        }
    }
    Ok(())
}
