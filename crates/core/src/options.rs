//! Conversion options shared by REST, MCP, CLI, and engines.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversionOptions {
    /// Run OCR before text extraction.
    pub ocr: bool,
    /// Tesseract language(s), e.g. `eng`, `deu`, or `eng+deu`.
    pub ocr_language: String,
    /// Prefer a layout-preserving external engine when one exists.
    pub preserve_layout: bool,
    /// Render only this one-based output page, where an engine supports it.
    pub page: Option<u32>,
    /// Stop after this many pages for engines that support page ranges.
    pub max_pages: Option<u32>,
    /// Hard per-step timeout. The service also enforces its own ceiling.
    pub timeout_secs: Option<u64>,
    pub pdf: PdfOptions,
    pub image: ImageOptions,
    pub csv: CsvOptions,
    pub spreadsheet: SpreadsheetOptions,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            ocr: false,
            ocr_language: "eng".to_string(),
            preserve_layout: false,
            page: None,
            max_pages: None,
            timeout_secs: None,
            pdf: PdfOptions::default(),
            image: ImageOptions::default(),
            csv: CsvOptions::default(),
            spreadsheet: SpreadsheetOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfOptions {
    pub page_size: PageSize,
    pub landscape: bool,
    pub margin_mm: f32,
    pub dpi: u16,
    pub title: Option<String>,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            page_size: PageSize::A4,
            landscape: false,
            margin_mm: 18.0,
            dpi: 200,
            title: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageSize {
    #[default]
    A4,
    Letter,
    Legal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageOptions {
    /// JPEG quality, 1..=100.
    pub quality: u8,
    pub max_width: Option<u32>,
    /// Background used when flattening transparency. `#RRGGBB`.
    pub background: String,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            max_width: None,
            background: "#ffffff".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CsvOptions {
    /// A single-byte delimiter. TSV defaults to a tab at the call site.
    pub delimiter: String,
    pub has_headers: bool,
    /// Number of records to inspect when inferring JSON scalar types.
    pub infer_types: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: ",".to_string(),
            has_headers: true,
            infer_types: true,
        }
    }
}

impl CsvOptions {
    pub fn delimiter_byte(&self) -> u8 {
        let mut chars = self.delimiter.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii() => c as u8,
            _ => b',',
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpreadsheetOptions {
    pub sheet_name: Option<String>,
    /// Treat the first row as headers when reading or writing.
    pub header_row: bool,
}

impl Default for SpreadsheetOptions {
    fn default() -> Self {
        Self {
            sheet_name: None,
            header_row: true,
        }
    }
}

impl ConversionOptions {
    pub fn with_ocr(mut self, required: bool) -> Self {
        if required {
            self.ocr = true;
        }
        self
    }

    pub fn timeout(&self, default_secs: u64, maximum_secs: u64) -> std::time::Duration {
        let secs = self
            .timeout_secs
            .unwrap_or(default_secs)
            .clamp(1, maximum_secs);
        std::time::Duration::from_secs(secs)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.ocr_language.trim().is_empty() {
            return Err("ocr_language must not be empty".into());
        }
        if self.ocr_language.len() > 64
            || !self
                .ocr_language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '_')
        {
            return Err("ocr_language contains invalid characters".into());
        }
        if let Some(0) = self.page {
            return Err("page is one-based and must be >= 1".into());
        }
        if let Some(0) = self.max_pages {
            return Err("max_pages must be >= 1".into());
        }
        if let Some(timeout) = self.timeout_secs {
            if !(1..=3600).contains(&timeout) {
                return Err("timeout_secs must be between 1 and 3600".into());
            }
        }
        if !(36..=1200).contains(&self.pdf.dpi) {
            return Err("pdf.dpi must be between 36 and 1200".into());
        }
        if !(0.0..=100.0).contains(&self.pdf.margin_mm) {
            return Err("pdf.margin_mm must be between 0 and 100".into());
        }
        if !(1..=100).contains(&self.image.quality) {
            return Err("image.quality must be between 1 and 100".into());
        }
        if self.csv.delimiter.len() != 1 {
            return Err("csv.delimiter must be exactly one ASCII byte".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        ConversionOptions::default().validate().unwrap();
    }

    #[test]
    fn rejects_bad_options() {
        let mut options = ConversionOptions::default();
        options.image.quality = 0;
        assert!(options.validate().is_err());
        options = ConversionOptions::default();
        options.csv.delimiter = "||".into();
        assert!(options.validate().is_err());
    }
}
