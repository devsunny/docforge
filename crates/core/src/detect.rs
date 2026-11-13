//! Content-based format detection with extension and MIME hints.

use crate::error::{ConvertError, Result};
use crate::format::Format;
use std::io::{Cursor, Read};
use std::path::Path;
use zip::ZipArchive;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedFormat {
    pub format: Format,
    pub confidence: f32,
    pub reason: String,
}

pub fn detect_file(
    path: &Path,
    filename: Option<&str>,
    mime: Option<&str>,
) -> Result<DetectedFormat> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Err(ConvertError::InvalidInput("uploaded file is empty".into()));
    }
    let name = filename.map(str::to_string).or_else(|| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
    });
    detect_bytes(&bytes, name.as_deref(), mime)
}

pub fn detect_bytes(
    bytes: &[u8],
    filename: Option<&str>,
    mime: Option<&str>,
) -> Result<DetectedFormat> {
    if bytes.is_empty() {
        return Err(ConvertError::InvalidInput("file is empty".into()));
    }
    let extension = filename
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase());
    let extension_format = extension
        .as_deref()
        .and_then(|extension| extension.parse::<Format>().ok());
    let mime = mime.map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or(value)
            .trim()
            .to_ascii_lowercase()
    });

    let magic = |format: Format, reason: &str| DetectedFormat {
        format,
        confidence: 0.99,
        reason: reason.to_string(),
    };

    if bytes.starts_with(b"%PDF-") {
        return Ok(magic(Format::Pdf, "PDF header"));
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(magic(Format::Png, "PNG signature"));
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(magic(Format::Jpg, "JPEG signature"));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        // There is no first-class GIF target, but JPEG is the closest public
        // image representation. Keep the extension hint when present.
        if let Some(format) = extension_format {
            return Ok(DetectedFormat {
                format,
                confidence: 0.8,
                reason: "GIF signature with extension hint".into(),
            });
        }
        return Err(ConvertError::InvalidInput(
            "GIF images are not in the supported catalog; convert to PNG or JPEG first".into(),
        ));
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Ok(magic(Format::Tiff, "TIFF signature"));
    }
    if bytes.starts_with(b"BM") {
        return Ok(magic(Format::Bmp, "BMP signature"));
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Ok(magic(Format::Webp, "WebP RIFF signature"));
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"heic" || brand == b"heix" || brand == b"hevc" || brand == b"mif1" {
            return Ok(magic(Format::Heic, "HEIF/HEIC signature"));
        }
    }
    if bytes.starts_with(b"%!PS") {
        return Ok(magic(Format::Ps, "PostScript header"));
    }
    if bytes.starts_with(b"AT&TFORM") {
        return Ok(magic(Format::Djvu, "DjVu FORM signature"));
    }
    if bytes.len() > 68 && &bytes[60..68] == b"BOOKMOBI" {
        return Ok(magic(Format::Mobi, "Mobipocket signature"));
    }
    if bytes.starts_with(b"{\\rtf") {
        return Ok(magic(Format::Rtf, "RTF header"));
    }
    if bytes.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        let format = match extension.as_deref() {
            Some("doc") => Format::Doc,
            Some("xls") => Format::Xls,
            Some("ppt") => Format::Ppt,
            Some("msg") => Format::Msg,
            _ => {
                return Err(ConvertError::InvalidInput(
                    "legacy OLE file detected; declare source_format (doc, xls, ppt, or msg)"
                        .into(),
                ))
            }
        };
        return Ok(DetectedFormat {
            format,
            confidence: 0.9,
            reason: "OLE compound document signature with extension hint".into(),
        });
    }

    if bytes.starts_with(b"PK\x03\x04") {
        if let Some(zip_format) = detect_zip(bytes) {
            return Ok(magic(zip_format, "ZIP container contents"));
        }
        if let Some(format) = extension_format {
            return Ok(DetectedFormat {
                format,
                confidence: 0.7,
                reason: "ZIP signature with extension hint".into(),
            });
        }
    }

    // Text formats: inspect the first 256 KiB.
    let sample = &bytes[..bytes.len().min(256 * 1024)];
    let text = crate::document::decode_text(sample);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("begin:vcard") {
        return Ok(magic(Format::Vcf, "vCard marker"));
    }
    if lower.starts_with("begin:vcalendar") {
        return Ok(magic(Format::Ics, "iCalendar marker"));
    }
    if lower.starts_with("mime-version:")
        || (lower.starts_with("from:") && lower.contains("subject:"))
    {
        return Ok(magic(Format::Eml, "RFC 822 headers"));
    }
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return Ok(magic(Format::Html, "HTML doctype"));
    }
    if lower.starts_with("<svg") || (lower.contains("<svg") && lower.contains("</svg>")) {
        return Ok(magic(Format::Svg, "SVG root element"));
    }
    if trimmed.starts_with('<') {
        return Ok(DetectedFormat {
            format: Format::Xml,
            confidence: 0.8,
            reason: "XML-like text".into(),
        });
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return Ok(magic(Format::Json, "valid JSON document"));
    }
    if lower.starts_with("{\\rtf") {
        return Ok(magic(Format::Rtf, "RTF header"));
    }
    if looks_like_ics(&text) {
        return Ok(magic(Format::Ics, "iCalendar text"));
    }

    if let Some(format) = extension_format {
        // Trust an explicit extension for ambiguous text formats, but still
        // perform a cheap sanity check where it helps.
        let confidence = match format {
            Format::Csv | Format::Tsv => 0.8,
            Format::Markdown => 0.75,
            _ => 0.6,
        };
        return Ok(DetectedFormat {
            format,
            confidence,
            reason: "text file with extension hint".into(),
        });
    }

    if looks_like_markdown(trimmed) {
        return Ok(DetectedFormat {
            format: Format::Markdown,
            confidence: 0.65,
            reason: "Markdown headings or fences".into(),
        });
    }
    if looks_like_delimited(trimmed, ',') {
        return Ok(DetectedFormat {
            format: Format::Csv,
            confidence: 0.65,
            reason: "consistent comma-separated rows".into(),
        });
    }
    if looks_like_delimited(trimmed, '\t') {
        return Ok(DetectedFormat {
            format: Format::Tsv,
            confidence: 0.65,
            reason: "consistent tab-separated rows".into(),
        });
    }
    if looks_like_yaml(trimmed) {
        return Ok(DetectedFormat {
            format: Format::Yaml,
            confidence: 0.55,
            reason: "YAML-like key/value text".into(),
        });
    }
    if looks_like_toml(trimmed) {
        return Ok(DetectedFormat {
            format: Format::Toml,
            confidence: 0.55,
            reason: "TOML-like sections or assignments".into(),
        });
    }
    if let Some(mime) = mime {
        if let Ok(format) = mime.parse::<Format>() {
            return Ok(DetectedFormat {
                format,
                confidence: 0.5,
                reason: "MIME type hint".into(),
            });
        }
    }

    Ok(DetectedFormat {
        format: Format::Txt,
        confidence: 0.5,
        reason: "fallback: UTF-8/Windows-1252 text".into(),
    })
}

fn detect_zip(bytes: &[u8]) -> Option<Format> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    if let Ok(mut mimetype) = archive.by_name("mimetype") {
        let mut value = String::new();
        if mimetype.read_to_string(&mut value).is_ok() {
            let value = value.trim();
            return match value {
                "application/epub+zip" => Some(Format::Epub),
                "application/vnd.oasis.opendocument.text" => Some(Format::Odt),
                "application/vnd.oasis.opendocument.spreadsheet" => Some(Format::Ods),
                "application/vnd.oasis.opendocument.presentation" => Some(Format::Odp),
                _ => None,
            };
        }
    }
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    if names.iter().any(|name| name == "word/document.xml") {
        return Some(Format::Docx);
    }
    if names.iter().any(|name| name == "xl/workbook.xml") {
        return Some(Format::Xlsx);
    }
    if names.iter().any(|name| name == "ppt/presentation.xml") {
        return Some(Format::Pptx);
    }
    if names
        .iter()
        .any(|name| name == "FixedDocumentSequence.fdseq" || name.ends_with(".fdseq"))
    {
        return Some(Format::Xps);
    }
    if names.iter().all(|name| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".png")
            || lower.ends_with(".gif")
            || lower.ends_with(".webp")
            || lower.ends_with(".bmp")
            || lower.ends_with(".tif")
            || lower.ends_with(".tiff")
            || lower.ends_with(".xml")
            || lower.ends_with('/')
    }) && names.iter().any(|name| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png")
    }) {
        return Some(Format::Cbz);
    }
    None
}

fn looks_like_markdown(text: &str) -> bool {
    text.lines().take(40).any(|line| {
        let line = line.trim_start();
        line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("```")
            || line.starts_with("- ")
            || line.starts_with("* ")
            || line.starts_with("> ")
    })
}

fn looks_like_delimited(text: &str, delimiter: char) -> bool {
    let mut counts = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()).take(6) {
        counts.push(line.matches(delimiter).count());
    }
    counts.len() >= 2 && counts[0] > 0 && counts.iter().all(|count| *count == counts[0])
}

fn looks_like_yaml(text: &str) -> bool {
    let mut key_lines = 0;
    for line in text.lines().take(20) {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() || line == "---" {
            continue;
        }
        if let Some((key, _)) = line.split_once(':') {
            if !key.trim().is_empty()
                && !key.contains(' ')
                && !line.starts_with('{')
                && !line.starts_with('[')
            {
                key_lines += 1;
            }
        }
    }
    key_lines >= 2
}

fn looks_like_toml(text: &str) -> bool {
    text.lines().take(20).any(|line| {
        let line = line.trim();
        (line.starts_with('[') && line.ends_with(']'))
            || (line.contains(" = ") && !line.starts_with('{'))
    })
}

fn looks_like_ics(text: &str) -> bool {
    text.to_ascii_uppercase().contains("BEGIN:VCALENDAR")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_pdf_and_json() {
        assert_eq!(
            detect_bytes(b"%PDF-1.7\n...", Some("x.pdf"), None)
                .unwrap()
                .format,
            Format::Pdf
        );
        assert_eq!(
            detect_bytes(br#"{"hello":"world"}"#, Some("x.txt"), None)
                .unwrap()
                .format,
            Format::Json
        );
    }

    #[test]
    fn detects_html_svg_and_csv() {
        assert_eq!(
            detect_bytes(b"<!doctype html><html></html>", None, None)
                .unwrap()
                .format,
            Format::Html
        );
        assert_eq!(
            detect_bytes(
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
                None,
                None
            )
            .unwrap()
            .format,
            Format::Svg
        );
        assert_eq!(
            detect_bytes(b"a,b,c\n1,2,3\n4,5,6\n", None, None)
                .unwrap()
                .format,
            Format::Csv
        );
    }

    #[test]
    fn detects_docx_zip() {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            zip.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            use std::io::Write;
            zip.write_all(b"<w:document/>").unwrap();
            zip.finish().unwrap();
        }
        let bytes = buffer.into_inner();
        assert_eq!(
            detect_bytes(&bytes, Some("file.bin"), None).unwrap().format,
            Format::Docx
        );
    }
}
