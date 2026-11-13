//! Pure-Rust Office-to-PDF conversion backed by `office2pdf`/Typst.

use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use std::path::Path;

pub fn office_to_pdf(
    input: &Path,
    output: &Path,
    from: Format,
    _options: &ConversionOptions,
) -> Result<()> {
    let format = match from {
        Format::Docx => office2pdf::config::Format::Docx,
        Format::Pptx => office2pdf::config::Format::Pptx,
        Format::Xlsx => office2pdf::config::Format::Xlsx,
        other => {
            return Err(ConvertError::Unsupported {
                from: other,
                to: Format::Pdf,
            })
        }
    };
    let bytes = std::fs::read(input)?;
    let result = office2pdf::convert_bytes(&bytes, format, &Default::default())
        .map_err(|error| ConvertError::engine("office2pdf", error.to_string()))?;
    std::fs::write(output, result.pdf)?;
    Ok(())
}
