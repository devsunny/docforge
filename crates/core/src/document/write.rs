//! Writers for the engine-neutral document model.

use super::{Block, Document};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::{ConversionOptions, PageSize};
use std::io::{Cursor, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub fn write_document(
    path: &Path,
    document: &Document,
    to: Format,
    options: &ConversionOptions,
) -> Result<()> {
    match to {
        Format::Txt => write_text(path, &document.plain_text()),
        Format::Markdown => write_text(path, &document.to_markdown()),
        Format::Html => write_text(path, &document.to_html_document()),
        Format::Tex => write_text(path, &document.to_latex()),
        Format::Rtf => write_text(path, &to_rtf(document)),
        Format::Docx => write_docx(path, document),
        Format::Odt => write_odt(path, document),
        Format::Epub => write_epub(path, document),
        Format::Pdf => super::pdf::write_pdf(path, document, options),
        Format::Pptx => super::pptx::write_pptx(path, document),
        other => Err(ConvertError::Unsupported {
            from: Format::Txt,
            to: other,
        }),
    }
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text.as_bytes())?;
    Ok(())
}

pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            ch if ch.is_control() => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn zip_write(zip: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, data: &str) -> Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(name, options)
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    zip.write_all(data.as_bytes())?;
    Ok(())
}

fn finish_zip(zip: ZipWriter<Cursor<Vec<u8>>>, path: &Path) -> Result<()> {
    let cursor = zip
        .finish()
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    std::fs::write(path, cursor.into_inner())?;
    Ok(())
}

fn docx_runs(text: &str) -> String {
    let mut out = String::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push_str("<w:br/>");
        }
        out.push_str(&format!(
            "<w:t xml:space=\"preserve\">{}</w:t>",
            xml_escape(line)
        ));
    }
    out
}

fn docx_paragraph(text: &str, style: Option<&str>) -> String {
    let ppr = style
        .map(|style| format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>"))
        .unwrap_or_default();
    format!("<w:p>{ppr}<w:r>{}</w:r></w:p>", docx_runs(text))
}

fn docx_list_item(text: &str, num_id: u16) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr><w:r>{}</w:r></w:p>",
        docx_runs(text)
    )
}

fn docx_table(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::from(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr>",
    );
    if !headers.is_empty() {
        out.push_str("<w:tr>");
        for header in headers {
            out.push_str(&format!(
                "<w:tc><w:tcPr><w:shd w:val=\"clear\" w:fill=\"E8EDF2\"/></w:tcPr>{} </w:tc>",
                docx_paragraph(header, None)
            ));
        }
        out.push_str("</w:tr>");
    }
    for row in rows {
        out.push_str("<w:tr>");
        for index in 0..headers.len().max(row.len()) {
            out.push_str(&format!(
                "<w:tc>{}</w:tc>",
                docx_paragraph(row.get(index).map(String::as_str).unwrap_or(""), None)
            ));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    out
}

fn document_xml(document: &Document) -> String {
    let mut body = String::new();
    for block in &document.blocks {
        match block {
            Block::Heading { level, text } => {
                body.push_str(&docx_paragraph(text, Some(&format!("Heading{level}"))));
            }
            Block::Paragraph(text) => body.push_str(&docx_paragraph(text, None)),
            Block::List { ordered, items } => {
                let num_id = if *ordered { 2 } else { 1 };
                for item in items {
                    body.push_str(&docx_list_item(item, num_id));
                }
            }
            Block::Quote(text) => body.push_str(&docx_paragraph(text, Some("Quote"))),
            Block::Code { text, .. } => body.push_str(&docx_paragraph(text, Some("CodeBlock"))),
            Block::Table { headers, rows } => body.push_str(&docx_table(headers, rows)),
            Block::PageBreak => {
                body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
            }
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    )
}

fn styles_xml() -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\"/><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>",
    );
    for level in 1..=6 {
        let size = 36 - (level * 2);
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\"><w:name w:val=\"heading {level}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"280\" w:after=\"120\"/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"{size}\"/></w:rPr></w:style>",
            level - 1
        ));
    }
    out.push_str("<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\"><w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:ind w:left=\"720\"/></w:pPr></w:style><w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:ind w:left=\"720\"/></w:pPr><w:rPr><w:i/><w:color w:val=\"52606D\"/></w:rPr></w:style><w:style w:type=\"paragraph\" w:styleId=\"CodeBlock\"><w:name w:val=\"Code Block\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:shd w:val=\"clear\" w:fill=\"F3F5F7\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/><w:sz w:val=\"19\"/></w:rPr></w:style><w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D0D7DE\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D0D7DE\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D0D7DE\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D0D7DE\"/><w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D0D7DE\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"D0D7DE\"/></w:tblBorders></w:tblPr></w:style></w:styles>"
    );
    out
}

fn numbering_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"singleLevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"•\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/></w:rPr></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"singleLevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num></w:numbering>".to_string()
}

pub fn write_docx(path: &Path, document: &Document) -> Result<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let title = document.title.as_deref().unwrap_or("Converted document");
    zip_write(
        &mut zip,
        "[Content_Types].xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/><Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/><Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/><Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/></Types>",
    )?;
    zip_write(
        &mut zip,
        "_rels/.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/></Relationships>",
    )?;
    zip_write(
        &mut zip,
        "word/_rels/document.xml.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/></Relationships>",
    )?;
    zip_write(&mut zip, "word/document.xml", &document_xml(document))?;
    zip_write(&mut zip, "word/styles.xml", &styles_xml())?;
    zip_write(&mut zip, "word/numbering.xml", &numbering_xml())?;
    zip_write(
        &mut zip,
        "word/settings.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:zoom w:percent=\"100\"/><w:defaultTabStop w:val=\"720\"/><w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>",
    )?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    zip_write(
        &mut zip,
        "docProps/core.xml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>{}</dc:title><dc:creator>DocForge</dc:creator><cp:lastModifiedBy>DocForge</cp:lastModifiedBy><dcterms:created xsi:type=\"dcterms:W3CDTF\">{now}</dcterms:created><dcterms:modified xsi:type=\"dcterms:W3CDTF\">{now}</dcterms:modified></cp:coreProperties>",
            xml_escape(title)
        ),
    )?;
    zip_write(
        &mut zip,
        "docProps/app.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>DocForge</Application><AppVersion>0.1</AppVersion></Properties>",
    )?;
    finish_zip(zip, path)
}

fn odt_content(document: &Document) -> String {
    let mut body = String::new();
    for block in &document.blocks {
        match block {
            Block::Heading { level, text } => body.push_str(&format!(
                "<text:h text:outline-level=\"{}\" text:style-name=\"Heading_20_{}\">{}</text:h>",
                (*level).clamp(1, 6),
                (*level).clamp(1, 6),
                xml_escape(text)
            )),
            Block::Paragraph(text) => body.push_str(&format!(
                "<text:p text:style-name=\"Text_20_body\">{}</text:p>",
                xml_escape(text)
            )),
            Block::List { ordered, items } => {
                body.push_str(&format!(
                    "<text:list text:style-name=\"{}\">",
                    if *ordered {
                        "Numbering_20_123"
                    } else {
                        "List_20_1"
                    }
                ));
                for item in items {
                    body.push_str(&format!(
                        "<text:list-item><text:p text:style-name=\"Text_20_body\">{}</text:p></text:list-item>",
                        xml_escape(item)
                    ));
                }
                body.push_str("</text:list>");
            }
            Block::Quote(text) => body.push_str(&format!(
                "<text:p text:style-name=\"Quotations\">{}</text:p>",
                xml_escape(text)
            )),
            Block::Code { text, .. } => body.push_str(&format!(
                "<text:p text:style-name=\"Preformatted_20_Text\">{}</text:p>",
                xml_escape(text)
            )),
            Block::Table { headers, rows } => {
                body.push_str("<table:table table:name=\"Table1\"><table:table-column table:number-columns-repeated=\"");
                body.push_str(&headers.len().max(1).to_string());
                body.push_str("\"/>");
                if !headers.is_empty() {
                    body.push_str("<table:table-row>");
                    for header in headers {
                        body.push_str(&format!(
                            "<table:table-cell office:value-type=\"string\"><text:p text:style-name=\"Table_20_Heading\">{}</text:p></table:table-cell>",
                            xml_escape(header)
                        ));
                    }
                    body.push_str("</table:table-row>");
                }
                for row in rows {
                    body.push_str("<table:table-row>");
                    for index in 0..headers.len().max(row.len()) {
                        body.push_str(&format!(
                            "<table:table-cell office:value-type=\"string\"><text:p text:style-name=\"Table_20_Contents\">{}</text:p></table:table-cell>",
                            xml_escape(row.get(index).map(String::as_str).unwrap_or(""))
                        ));
                    }
                    body.push_str("</table:table-row>");
                }
                body.push_str("</table:table>");
            }
            Block::PageBreak => body
                .push_str("<text:p text:style-name=\"PageBreak\"><text:soft-page-break/></text:p>"),
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" office:version=\"1.2\"><office:body><office:text>{body}</office:text></office:body></office:document-content>"
    )
}

fn odt_styles() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" office:version=\"1.2\"><office:styles><style:style style:name=\"Standard\" style:family=\"paragraph\" style:class=\"text\"/><style:style style:name=\"Text_20_body\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-bottom=\"0.2in\"/></style:style><style:style style:name=\"Heading_20_1\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-top=\"0.25in\" fo:margin-bottom=\"0.12in\"/><style:text-properties fo:font-size=\"18pt\" fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Heading_20_2\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-top=\"0.2in\" fo:margin-bottom=\"0.1in\"/><style:text-properties fo:font-size=\"15pt\" fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Heading_20_3\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:text-properties fo:font-size=\"13pt\" fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Heading_20_4\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:text-properties fo:font-size=\"12pt\" fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Heading_20_5\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:text-properties fo:font-size=\"11pt\" fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Heading_20_6\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:text-properties fo:font-size=\"11pt\" fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Quotations\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:paragraph-properties fo:margin-left=\"0.4in\"/><style:text-properties fo:font-style=\"italic\"/></style:style><style:style style:name=\"Preformatted_20_Text\" style:family=\"paragraph\" style:parent-style-name=\"Standard\"><style:text-properties style:font-name=\"Liberation Mono\" fo:font-family=\"Liberation Mono\" fo:font-size=\"9pt\"/></style:style><style:style style:name=\"Table_20_Heading\" style:family=\"paragraph\"><style:text-properties fo:font-weight=\"bold\"/></style:style><style:style style:name=\"Table_20_Contents\" style:family=\"paragraph\"/><style:style style:name=\"PageBreak\" style:family=\"paragraph\"><style:paragraph-properties fo:break-before=\"page\"/></style:style></office:styles></office:document-styles>".to_string()
}

pub fn write_odt(path: &Path, document: &Document) -> Result<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored)
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    zip.write_all(b"application/vnd.oasis.opendocument.text")?;

    zip_write(&mut zip, "content.xml", &odt_content(document))?;
    zip_write(&mut zip, "styles.xml", &odt_styles())?;
    zip_write(
        &mut zip,
        "meta.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-meta xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" office:version=\"1.2\"><office:meta><meta:generator>DocForge</meta:generator></office:meta></office:document-meta>",
    )?;
    zip_write(
        &mut zip,
        "META-INF/manifest.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.2\"><manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/><manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/><manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/><manifest:file-entry manifest:full-path=\"meta.xml\" manifest:media-type=\"text/xml\"/></manifest:manifest>",
    )?;
    finish_zip(zip, path)
}

fn rtf_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\par "),
            '\t' => out.push_str("\\tab "),
            ch if ch.is_ascii() => out.push(ch),
            ch => {
                let code = ch as u32;
                if code <= 0xFFFF {
                    out.push_str(&format!("\\u{}?", code as i32));
                } else {
                    out.push('?');
                }
            }
        }
    }
    out
}

pub fn to_rtf(document: &Document) -> String {
    let mut out = String::from(
        "{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0\\fswiss Helvetica;}{\\f1\\fmodern Consolas;}}\\viewkind4\\uc1\n",
    );
    if let Some(title) = &document.title {
        out.push_str(&format!(
            "\\pard\\qc\\b\\fs36 {}\\b0\\fs24\\par\n",
            rtf_escape(title)
        ));
    }
    for block in &document.blocks {
        match block {
            Block::Heading { level, text } => {
                let size = 30 - (level.saturating_sub(1) * 2);
                out.push_str(&format!(
                    "\\pard\\sa180\\b\\fs{size} {}\\b0\\fs24\\par\n",
                    rtf_escape(text)
                ));
            }
            Block::Paragraph(text) => {
                out.push_str(&format!("\\pard\\sa120 {}\\par\n", rtf_escape(text)));
            }
            Block::List { ordered, items } => {
                for (index, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}. ", index + 1)
                    } else {
                        "\\bullet  ".to_string()
                    };
                    out.push_str(&format!(
                        "\\pard\\li360 {}{}\\par\n",
                        marker,
                        rtf_escape(item)
                    ));
                }
            }
            Block::Quote(text) => {
                out.push_str(&format!("\\pard\\li720\\i {}\\i0\\par\n", rtf_escape(text)))
            }
            Block::Code { text, .. } => out.push_str(&format!(
                "\\pard\\f1\\fs18 {}\\f0\\fs24\\par\n",
                rtf_escape(text)
            )),
            Block::Table { headers, rows } => {
                if !headers.is_empty() {
                    out.push_str(&format!(
                        "\\pard\\b {}\\b0\\par\n",
                        headers
                            .iter()
                            .map(|cell| rtf_escape(cell))
                            .collect::<Vec<_>>()
                            .join("\\tab ")
                    ));
                }
                for row in rows {
                    out.push_str(&format!(
                        "\\pard {}\\par\n",
                        row.iter()
                            .map(|cell| rtf_escape(cell))
                            .collect::<Vec<_>>()
                            .join("\\tab ")
                    ));
                }
            }
            Block::PageBreak => out.push_str("\\page\n"),
        }
    }
    out.push('}');
    out
}

fn epub_chapters(document: &Document) -> Vec<(String, Vec<Block>)> {
    let mut chapters: Vec<(String, Vec<Block>)> = Vec::new();
    let mut current: Vec<Block> = Vec::new();
    let mut title = document
        .title
        .clone()
        .unwrap_or_else(|| "Converted document".to_string());

    for block in &document.blocks {
        let start_new = match block {
            Block::Heading { level: 1, text } => !current.is_empty() && !text.is_empty(),
            Block::PageBreak => true,
            _ => false,
        };
        if start_new && !current.is_empty() {
            chapters.push((title, std::mem::take(&mut current)));
            title = match block {
                Block::Heading { text, .. } => text.clone(),
                _ => format!("Chapter {}", chapters.len() + 1),
            };
        }
        match block {
            Block::PageBreak => continue,
            Block::Heading { level: 1, text } => {
                title = text.clone();
                current.push(block.clone());
            }
            _ => current.push(block.clone()),
        }
    }
    if !current.is_empty() {
        chapters.push((title, current));
    }
    if chapters.is_empty() {
        chapters.push((
            document
                .title
                .clone()
                .unwrap_or_else(|| "Converted document".to_string()),
            Vec::new(),
        ));
    }
    chapters
}

fn xhtml_page(title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"en\"><head><meta charset=\"utf-8\"/><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head><body><h1>{}</h1>{}</body></html>",
        xml_escape(title),
        xml_escape(title),
        body
    )
}

pub fn write_epub(path: &Path, document: &Document) -> Result<()> {
    let chapters = epub_chapters(document);
    let title = document
        .title
        .clone()
        .unwrap_or_else(|| "Converted document".to_string());
    let identifier = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored)
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    zip.write_all(b"application/epub+zip")?;

    zip_write(
        &mut zip,
        "META-INF/container.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>",
    )?;

    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav_items = String::new();
    let mut ncx_items = String::new();
    for (index, (chapter_title, blocks)) in chapters.iter().enumerate() {
        let number = index + 1;
        let filename = format!("chapter{number}.xhtml");
        let body = blocks.iter().map(block_to_html).collect::<String>();
        zip_write(
            &mut zip,
            &format!("OEBPS/{filename}"),
            &xhtml_page(chapter_title, &body),
        )?;
        manifest.push_str(&format!(
            "<item id=\"chapter{number}\" href=\"{filename}\" media-type=\"application/xhtml+xml\"/>"
        ));
        spine.push_str(&format!("<itemref idref=\"chapter{number}\"/>"));
        nav_items.push_str(&format!(
            "<li><a href=\"{filename}\">{}</a></li>",
            xml_escape(chapter_title)
        ));
        ncx_items.push_str(&format!(
            "<navPoint id=\"nav{number}\" playOrder=\"{number}\"><navLabel><text>{}</text></navLabel><content src=\"{filename}\"/></navPoint>",
            xml_escape(chapter_title)
        ));
    }

    zip_write(
        &mut zip,
        "OEBPS/style.css",
        "body{font-family:Georgia,serif;line-height:1.6;margin:1.5em;}h1{font-size:1.6em;}pre{background:#f4f6f8;padding:.8em;white-space:pre-wrap;}table{border-collapse:collapse;}td,th{border:1px solid #ccc;padding:.3em .5em;}blockquote{border-left:3px solid #ccc;margin-left:0;padding-left:1em;color:#555;}",
    )?;

    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"bookid\">{identifier}</dc:identifier><dc:title>{}</dc:title><dc:language>en</dc:language><meta property=\"dcterms:modified\">{}</meta></metadata><manifest><item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/><item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/><item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>{manifest}</manifest><spine toc=\"ncx\">{spine}</spine></package>",
        xml_escape(&title),
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
    zip_write(&mut zip, "OEBPS/content.opf", &opf)?;
    zip_write(
        &mut zip,
        "OEBPS/nav.xhtml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\"><head><title>Contents</title></head><body><nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1><ol>{nav_items}</ol></nav></body></html>"
        ),
    )?;
    zip_write(
        &mut zip,
        "OEBPS/toc.ncx",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><head><meta name=\"dtb:uid\" content=\"{identifier}\"/></head><docTitle><text>{}</text></docTitle><navMap>{ncx_items}</navMap></ncx>",
            xml_escape(&title)
        ),
    )?;
    finish_zip(zip, path)
}

fn block_to_html(block: &Block) -> String {
    let document = Document {
        title: None,
        blocks: vec![block.clone()],
    };
    document.to_html_fragment()
}

/// PDF page size in millimetres.
pub fn page_dimensions(options: &ConversionOptions) -> (f32, f32) {
    let (width, height) = match options.pdf.page_size {
        PageSize::A4 => (210.0, 297.0),
        PageSize::Letter => (215.9, 279.4),
        PageSize::Legal => (215.9, 355.6),
    };
    if options.pdf.landscape {
        (height, width)
    } else {
        (width, height)
    }
}

/// A small helper used by the image-to-PDF path.
pub fn flatten_background(hex: &str) -> [u8; 3] {
    let trimmed = hex.trim_start_matches('#');
    if trimmed.len() == 6 {
        if let Ok(value) = u32::from_str_radix(trimmed, 16) {
            return [
                ((value >> 16) & 0xFF) as u8,
                ((value >> 8) & 0xFF) as u8,
                (value & 0xFF) as u8,
            ];
        }
    }
    [255, 255, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_zip_contains_required_parts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("out.docx");
        let mut document = Document::new("Test");
        document.push(Block::Heading {
            level: 1,
            text: "Hello".into(),
        });
        document.push(Block::Paragraph("World".into()));
        write_docx(&path, &document).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let names = {
            let archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            archive.file_names().map(str::to_string).collect::<Vec<_>>()
        };
        for required in [
            "[Content_Types].xml",
            "word/document.xml",
            "word/styles.xml",
            "_rels/.rels",
        ] {
            assert!(names.contains(&required.to_string()), "missing {required}");
        }
    }

    #[test]
    fn odt_mimetype_is_first_and_stored() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("out.odt");
        write_odt(&path, &Document::from_plain_text("hello")).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.file_names().next(), Some("mimetype"));
    }

    #[test]
    fn rtf_escapes_braces() {
        let rtf = to_rtf(&Document::from_plain_text("a { b } c"));
        assert!(rtf.contains("\\{") && rtf.contains("\\}"));
    }
}
