//! Canonical formats and the aliases accepted by the public API.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Every format DocForge knows how to name.
///
/// The catalog uses human labels such as `PDF (Searchable)`. Those labels are
/// normalized to one of these variants; flags such as OCR are carried in
/// [`crate::options::ConversionOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Format {
    Pdf,
    Docx,
    Doc,
    Txt,
    Markdown,
    Html,
    Odt,
    Rtf,
    Epub,
    Mobi,
    Pptx,
    Ppt,
    Key,
    Pages,
    Odp,
    Xlsx,
    Xls,
    Csv,
    Tsv,
    Ods,
    Numbers,
    Jpg,
    Png,
    Tiff,
    Heic,
    Webp,
    Bmp,
    Svg,
    Json,
    Xml,
    Yaml,
    Toml,
    Tex,
    Ps,
    Xps,
    Djvu,
    Vcf,
    Ics,
    Eml,
    Msg,
    Cbz,
    Cbr,
}

/// A coarse family used for routing, UI grouping, and engine selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatKind {
    Document,
    Presentation,
    Spreadsheet,
    Image,
    StructuredData,
    Email,
    Calendar,
    Contact,
    Archive,
    Typesetting,
    Print,
    Ebook,
}

impl Format {
    pub const ALL: &'static [Format] = &[
        Format::Pdf,
        Format::Docx,
        Format::Doc,
        Format::Txt,
        Format::Markdown,
        Format::Html,
        Format::Odt,
        Format::Rtf,
        Format::Epub,
        Format::Mobi,
        Format::Pptx,
        Format::Ppt,
        Format::Key,
        Format::Pages,
        Format::Odp,
        Format::Xlsx,
        Format::Xls,
        Format::Csv,
        Format::Tsv,
        Format::Ods,
        Format::Numbers,
        Format::Jpg,
        Format::Png,
        Format::Tiff,
        Format::Heic,
        Format::Webp,
        Format::Bmp,
        Format::Svg,
        Format::Json,
        Format::Xml,
        Format::Yaml,
        Format::Toml,
        Format::Tex,
        Format::Ps,
        Format::Xps,
        Format::Djvu,
        Format::Vcf,
        Format::Ics,
        Format::Eml,
        Format::Msg,
        Format::Cbz,
        Format::Cbr,
    ];

    /// Stable identifier used in JSON, URLs, and the CLI.
    pub fn id(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Doc => "doc",
            Format::Txt => "txt",
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Odt => "odt",
            Format::Rtf => "rtf",
            Format::Epub => "epub",
            Format::Mobi => "mobi",
            Format::Pptx => "pptx",
            Format::Ppt => "ppt",
            Format::Key => "key",
            Format::Pages => "pages",
            Format::Odp => "odp",
            Format::Xlsx => "xlsx",
            Format::Xls => "xls",
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Ods => "ods",
            Format::Numbers => "numbers",
            Format::Jpg => "jpg",
            Format::Png => "png",
            Format::Tiff => "tiff",
            Format::Heic => "heic",
            Format::Webp => "webp",
            Format::Bmp => "bmp",
            Format::Svg => "svg",
            Format::Json => "json",
            Format::Xml => "xml",
            Format::Yaml => "yaml",
            Format::Toml => "toml",
            Format::Tex => "tex",
            Format::Ps => "ps",
            Format::Xps => "xps",
            Format::Djvu => "djvu",
            Format::Vcf => "vcf",
            Format::Ics => "ics",
            Format::Eml => "eml",
            Format::Msg => "msg",
            Format::Cbz => "cbz",
            Format::Cbr => "cbr",
        }
    }

    /// Human-facing label; the catalog uses these spellings.
    pub fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::Docx => "DOCX",
            Format::Doc => "DOC",
            Format::Txt => "TXT",
            Format::Markdown => "MD",
            Format::Html => "HTML",
            Format::Odt => "ODT",
            Format::Rtf => "RTF",
            Format::Epub => "EPUB",
            Format::Mobi => "MOBI",
            Format::Pptx => "PPTX",
            Format::Ppt => "PPT",
            Format::Key => "KEY",
            Format::Pages => "Pages",
            Format::Odp => "ODP",
            Format::Xlsx => "XLSX",
            Format::Xls => "XLS",
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Ods => "ODS",
            Format::Numbers => "Numbers",
            Format::Jpg => "JPG",
            Format::Png => "PNG",
            Format::Tiff => "TIFF",
            Format::Heic => "HEIC",
            Format::Webp => "WEBP",
            Format::Bmp => "BMP",
            Format::Svg => "SVG",
            Format::Json => "JSON",
            Format::Xml => "XML",
            Format::Yaml => "YAML",
            Format::Toml => "TOML",
            Format::Tex => "TEX (LaTeX)",
            Format::Ps => "PS (PostScript)",
            Format::Xps => "XPS",
            Format::Djvu => "DJVU",
            Format::Vcf => "VCF (vCard)",
            Format::Ics => "ICS",
            Format::Eml => "EML",
            Format::Msg => "MSG",
            Format::Cbz => "CBZ",
            Format::Cbr => "CBR",
        }
    }

    /// Primary extension, without a dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Jpg => "jpg",
            Format::Tiff => "tiff",
            Format::Yaml => "yaml",
            Format::Tex => "tex",
            Format::Ps => "ps",
            Format::Djvu => "djvu",
            Format::Vcf => "vcf",
            Format::Eml => "eml",
            Format::Msg => "msg",
            Format::Cbz => "cbz",
            Format::Cbr => "cbr",
            Format::Numbers => "numbers",
            other => other.id(),
        }
    }

    /// All extensions commonly used for the format.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Pdf => &["pdf"],
            Format::Docx => &["docx"],
            Format::Doc => &["doc"],
            Format::Txt => &["txt", "text", "log"],
            Format::Markdown => &["md", "markdown", "mdown", "mkd"],
            Format::Html => &["html", "htm", "xhtml"],
            Format::Odt => &["odt"],
            Format::Rtf => &["rtf"],
            Format::Epub => &["epub"],
            Format::Mobi => &["mobi", "azw", "azw3"],
            Format::Pptx => &["pptx"],
            Format::Ppt => &["ppt"],
            Format::Key => &["key"],
            Format::Pages => &["pages"],
            Format::Odp => &["odp"],
            Format::Xlsx => &["xlsx"],
            Format::Xls => &["xls"],
            Format::Csv => &["csv"],
            Format::Tsv => &["tsv", "tab"],
            Format::Ods => &["ods"],
            Format::Numbers => &["numbers"],
            Format::Jpg => &["jpg", "jpeg", "jpe"],
            Format::Png => &["png"],
            Format::Tiff => &["tif", "tiff"],
            Format::Heic => &["heic", "heif"],
            Format::Webp => &["webp"],
            Format::Bmp => &["bmp"],
            Format::Svg => &["svg"],
            Format::Json => &["json"],
            Format::Xml => &["xml"],
            Format::Yaml => &["yaml", "yml"],
            Format::Toml => &["toml"],
            Format::Tex => &["tex", "latex"],
            Format::Ps => &["ps", "eps"],
            Format::Xps => &["xps", "oxps"],
            Format::Djvu => &["djvu", "djv"],
            Format::Vcf => &["vcf", "vcard"],
            Format::Ics => &["ics", "ical"],
            Format::Eml => &["eml"],
            Format::Msg => &["msg"],
            Format::Cbz => &["cbz"],
            Format::Cbr => &["cbr"],
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Format::Pdf => "application/pdf",
            Format::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            Format::Doc => "application/msword",
            Format::Txt => "text/plain",
            Format::Markdown => "text/markdown",
            Format::Html => "text/html",
            Format::Odt => "application/vnd.oasis.opendocument.text",
            Format::Rtf => "application/rtf",
            Format::Epub => "application/epub+zip",
            Format::Mobi => "application/x-mobipocket-ebook",
            Format::Pptx => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
            Format::Ppt => "application/vnd.ms-powerpoint",
            Format::Key => "application/vnd.apple.keynote",
            Format::Pages => "application/vnd.apple.pages",
            Format::Odp => "application/vnd.oasis.opendocument.presentation",
            Format::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Format::Xls => "application/vnd.ms-excel",
            Format::Csv => "text/csv",
            Format::Tsv => "text/tab-separated-values",
            Format::Ods => "application/vnd.oasis.opendocument.spreadsheet",
            Format::Numbers => "application/vnd.apple.numbers",
            Format::Jpg => "image/jpeg",
            Format::Png => "image/png",
            Format::Tiff => "image/tiff",
            Format::Heic => "image/heic",
            Format::Webp => "image/webp",
            Format::Bmp => "image/bmp",
            Format::Svg => "image/svg+xml",
            Format::Json => "application/json",
            Format::Xml => "application/xml",
            Format::Yaml => "application/yaml",
            Format::Toml => "application/toml",
            Format::Tex => "application/x-tex",
            Format::Ps => "application/postscript",
            Format::Xps => "application/vnd.ms-xpsdocument",
            Format::Djvu => "image/vnd.djvu",
            Format::Vcf => "text/vcard",
            Format::Ics => "text/calendar",
            Format::Eml => "message/rfc822",
            Format::Msg => "application/vnd.ms-outlook",
            Format::Cbz => "application/vnd.comicbook+zip",
            Format::Cbr => "application/vnd.comicbook-rar",
        }
    }

    pub fn kind(self) -> FormatKind {
        match self {
            Format::Pdf
            | Format::Docx
            | Format::Doc
            | Format::Txt
            | Format::Markdown
            | Format::Html
            | Format::Odt
            | Format::Rtf => FormatKind::Document,
            Format::Epub | Format::Mobi => FormatKind::Ebook,
            Format::Pptx | Format::Ppt | Format::Key | Format::Pages | Format::Odp => {
                FormatKind::Presentation
            }
            Format::Xlsx
            | Format::Xls
            | Format::Csv
            | Format::Tsv
            | Format::Ods
            | Format::Numbers => FormatKind::Spreadsheet,
            Format::Jpg
            | Format::Png
            | Format::Tiff
            | Format::Heic
            | Format::Webp
            | Format::Bmp
            | Format::Svg => FormatKind::Image,
            Format::Json | Format::Xml | Format::Yaml | Format::Toml => FormatKind::StructuredData,
            Format::Eml | Format::Msg => FormatKind::Email,
            Format::Vcf => FormatKind::Contact,
            Format::Ics => FormatKind::Calendar,
            Format::Cbz | Format::Cbr => FormatKind::Archive,
            Format::Tex => FormatKind::Typesetting,
            Format::Ps | Format::Xps | Format::Djvu => FormatKind::Print,
        }
    }

    pub fn is_image(self) -> bool {
        self.kind() == FormatKind::Image
    }

    pub fn is_spreadsheet(self) -> bool {
        self.kind() == FormatKind::Spreadsheet
    }

    pub fn is_presentation(self) -> bool {
        self.kind() == FormatKind::Presentation
    }

    pub fn is_textual(self) -> bool {
        matches!(
            self,
            Format::Txt
                | Format::Markdown
                | Format::Html
                | Format::Json
                | Format::Xml
                | Format::Yaml
                | Format::Toml
                | Format::Csv
                | Format::Tsv
                | Format::Tex
                | Format::Vcf
                | Format::Ics
                | Format::Svg
        )
    }

    pub fn is_document_like(self) -> bool {
        matches!(
            self,
            Format::Pdf
                | Format::Docx
                | Format::Doc
                | Format::Txt
                | Format::Markdown
                | Format::Html
                | Format::Odt
                | Format::Rtf
                | Format::Epub
        )
    }

    pub fn description(self) -> &'static str {
        match self {
            Format::Pdf => "Portable Document Format",
            Format::Docx => "Microsoft Word (Office Open XML)",
            Format::Doc => "Microsoft Word 97-2003",
            Format::Txt => "Plain UTF-8 text",
            Format::Markdown => "Markdown",
            Format::Html => "HyperText Markup Language",
            Format::Odt => "OpenDocument Text",
            Format::Rtf => "Rich Text Format",
            Format::Epub => "EPUB e-book",
            Format::Mobi => "Mobipocket / Kindle e-book",
            Format::Pptx => "Microsoft PowerPoint (Office Open XML)",
            Format::Ppt => "Microsoft PowerPoint 97-2003",
            Format::Key => "Apple Keynote",
            Format::Pages => "Apple Pages",
            Format::Odp => "OpenDocument Presentation",
            Format::Xlsx => "Microsoft Excel (Office Open XML)",
            Format::Xls => "Microsoft Excel 97-2003",
            Format::Csv => "Comma-separated values",
            Format::Tsv => "Tab-separated values",
            Format::Ods => "OpenDocument Spreadsheet",
            Format::Numbers => "Apple Numbers",
            Format::Jpg => "JPEG image",
            Format::Png => "Portable Network Graphics",
            Format::Tiff => "Tagged Image File Format",
            Format::Heic => "High Efficiency Image Container",
            Format::Webp => "WebP image",
            Format::Bmp => "Windows bitmap",
            Format::Svg => "Scalable Vector Graphics",
            Format::Json => "JavaScript Object Notation",
            Format::Xml => "Extensible Markup Language",
            Format::Yaml => "YAML Ain't Markup Language",
            Format::Toml => "Tom's Obvious Minimal Language",
            Format::Tex => "LaTeX source",
            Format::Ps => "PostScript",
            Format::Xps => "XML Paper Specification",
            Format::Djvu => "DjVu scanned document",
            Format::Vcf => "vCard contact",
            Format::Ics => "iCalendar",
            Format::Eml => "RFC 822 email",
            Format::Msg => "Microsoft Outlook message",
            Format::Cbz => "Comic book archive (ZIP)",
            Format::Cbr => "Comic book archive (RAR)",
        }
    }

    /// A serializable descriptor used by `/formats` and MCP.
    pub fn info(self) -> FormatInfo {
        FormatInfo {
            id: self.id().to_string(),
            label: self.label().to_string(),
            kind: self.kind(),
            extensions: self.extensions().iter().map(|s| s.to_string()).collect(),
            mime: self.mime().to_string(),
            description: self.description().to_string(),
        }
    }

    /// Parse a human label, extension, MIME type, or catalog spelling.
    pub fn parse(input: &str) -> Result<Self, FormatParseError> {
        input.parse()
    }

    /// Catalog labels can contain aliases (`EML / MSG`) and qualifiers
    /// (`JPG (OCR)`, `PDF (Searchable)`). Return every concrete format named.
    pub fn parse_catalog_label(input: &str) -> Result<Vec<Self>, FormatParseError> {
        let mut out = Vec::new();
        for part in input.split('/') {
            let token = part.split('(').next().unwrap_or(part).trim();
            if token.is_empty() {
                continue;
            }
            out.push(Format::parse(token)?);
        }
        out.sort();
        out.dedup();
        if out.is_empty() {
            return Err(FormatParseError(input.to_string()));
        }
        Ok(out)
    }

    /// Whether a catalog label requests OCR.
    pub fn label_requests_ocr(input: &str) -> bool {
        input.to_ascii_uppercase().contains("(OCR)")
    }

    /// Whether a catalog label requests a searchable-PDF output.
    pub fn label_is_searchable(input: &str) -> bool {
        input.to_ascii_uppercase().contains("SEARCHABLE")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatInfo {
    pub id: String,
    pub label: String,
    pub kind: FormatKind,
    pub extensions: Vec<String>,
    pub mime: String,
    pub description: String,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl From<Format> for String {
    fn from(value: Format) -> Self {
        value.id().to_string()
    }
}

impl TryFrom<String> for Format {
    type Error = FormatParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unsupported format: {0}")]
pub struct FormatParseError(pub String);

impl FromStr for Format {
    type Err = FormatParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(FormatParseError(input.to_string()));
        }
        let lower = trimmed.to_ascii_lowercase();
        let lower = lower.strip_prefix('.').unwrap_or(&lower);
        let lower = lower.split(';').next().unwrap_or(lower).trim();
        let base = lower.split('(').next().unwrap_or(lower).trim();
        let base = base.split(" / ").next().unwrap_or(base).trim();

        let format = match base {
            "pdf" | "application/pdf" => Format::Pdf,
            "docx" | "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Format::Docx
            }
            "doc" | "application/msword" => Format::Doc,
            "txt" | "text" | "text/plain" | "log" => Format::Txt,
            "md" | "markdown" | "text/markdown" | "text/x-markdown" => Format::Markdown,
            "html" | "htm" | "xhtml" | "text/html" | "application/xhtml+xml" => Format::Html,
            "odt" | "application/vnd.oasis.opendocument.text" => Format::Odt,
            "rtf" | "application/rtf" | "text/rtf" => Format::Rtf,
            "epub" | "application/epub+zip" => Format::Epub,
            "mobi" | "azw" | "azw3" | "application/x-mobipocket-ebook" => Format::Mobi,
            "pptx"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                Format::Pptx
            }
            "ppt" | "application/vnd.ms-powerpoint" => Format::Ppt,
            "key" | "keynote" | "application/vnd.apple.keynote" => Format::Key,
            "pages" | "application/vnd.apple.pages" => Format::Pages,
            "odp" | "application/vnd.oasis.opendocument.presentation" => Format::Odp,
            "xlsx" | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
                Format::Xlsx
            }
            "xls" | "application/vnd.ms-excel" => Format::Xls,
            "csv" | "text/csv" => Format::Csv,
            "tsv" | "tab" | "text/tab-separated-values" => Format::Tsv,
            "ods" | "application/vnd.oasis.opendocument.spreadsheet" => Format::Ods,
            "numbers" | "application/vnd.apple.numbers" => Format::Numbers,
            "jpg" | "jpeg" | "jpe" | "image/jpeg" => Format::Jpg,
            "png" | "image/png" => Format::Png,
            "tif" | "tiff" | "image/tiff" => Format::Tiff,
            "heic" | "heif" | "image/heic" | "image/heif" => Format::Heic,
            "webp" | "image/webp" => Format::Webp,
            "bmp" | "image/bmp" => Format::Bmp,
            "svg" | "image/svg+xml" => Format::Svg,
            "json" | "application/json" => Format::Json,
            "xml" | "application/xml" | "text/xml" => Format::Xml,
            "yaml" | "yml" | "application/yaml" | "text/yaml" => Format::Yaml,
            "toml" | "application/toml" | "text/toml" => Format::Toml,
            "tex" | "latex" | "application/x-tex" | "text/x-tex" => Format::Tex,
            "ps" | "eps" | "postscript" | "application/postscript" => Format::Ps,
            "xps" | "oxps" | "application/vnd.ms-xpsdocument" => Format::Xps,
            "djvu" | "djv" | "image/vnd.djvu" => Format::Djvu,
            "vcf" | "vcard" | "text/vcard" | "text/x-vcard" => Format::Vcf,
            "ics" | "ical" | "icalendar" | "text/calendar" => Format::Ics,
            "eml" | "message/rfc822" => Format::Eml,
            "msg" | "application/vnd.ms-outlook" => Format::Msg,
            "cbz" | "application/vnd.comicbook+zip" => Format::Cbz,
            "cbr" | "application/vnd.comicbook-rar" => Format::Cbr,
            _ => return Err(FormatParseError(input.to_string())),
        };
        Ok(format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_qualifiers() {
        assert_eq!(Format::parse("PDF (Searchable)").unwrap(), Format::Pdf);
        assert_eq!(Format::parse("JPG (OCR)").unwrap(), Format::Jpg);
        assert_eq!(Format::parse("TEX (LaTeX)").unwrap(), Format::Tex);
        assert_eq!(Format::parse("VCF (vCard)").unwrap(), Format::Vcf);
        assert_eq!(
            Format::parse_catalog_label("EML / MSG").unwrap(),
            vec![Format::Eml, Format::Msg]
        );
        assert_eq!(
            Format::parse_catalog_label("CBZ / CBR").unwrap(),
            vec![Format::Cbz, Format::Cbr]
        );
    }

    #[test]
    fn round_trips_json() {
        let json = serde_json::to_string(&Format::Markdown).unwrap();
        assert_eq!(json, "\"md\"");
        let parsed: Format = serde_json::from_str("\"markdown\"").unwrap();
        assert_eq!(parsed, Format::Markdown);
    }
}
