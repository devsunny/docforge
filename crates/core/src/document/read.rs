//! Readers for text-bearing document formats.

use super::{decode_text, document_from_html, normalize_whitespace, Block, Document};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use mail_parser::MimeHeaders;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Cursor, Read};
use std::path::Path;
use zip::ZipArchive;

pub fn read_document(path: &Path, from: Format, options: &ConversionOptions) -> Result<Document> {
    let bytes = std::fs::read(path)?;
    match from {
        Format::Txt => Ok(Document::from_plain_text(&decode_text(&bytes))),
        Format::Markdown => Ok(Document::from_markdown(&decode_text(&bytes))),
        Format::Html => Ok(document_from_html(&decode_text(&bytes))),
        Format::Docx => docx_document(&bytes),
        Format::Odt => odt_document(&bytes),
        Format::Rtf => rtf_document(&bytes),
        Format::Pdf => pdf_document(path, options),
        Format::Epub => epub_document(&bytes),
        Format::Pptx => pptx_document(&bytes),
        Format::Eml => eml_document(&bytes),
        Format::Msg => msg_document(&bytes),
        Format::Tex => Ok(tex_document(&decode_text(&bytes))),
        Format::Json => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            Ok(Document::from_plain_text(&pretty))
        }
        Format::Yaml => {
            let value: serde_json::Value = serde_yaml::from_slice(&bytes)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            Ok(Document::from_plain_text(&pretty))
        }
        Format::Toml => {
            let text = decode_text(&bytes);
            let value: toml::Value = toml::from_str(&text)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            let json = serde_json::to_value(value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            let pretty = serde_json::to_string_pretty(&json)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            Ok(Document::from_plain_text(&pretty))
        }
        Format::Xml => Ok(Document::from_plain_text(&decode_text(&bytes))),
        Format::Csv | Format::Tsv => crate::data::workbook_document(path, from, options),
        Format::Xlsx | Format::Xls | Format::Ods => {
            crate::data::workbook_document(path, from, options)
        }
        Format::Vcf => crate::data::vcf_document(&bytes),
        Format::Ics => crate::data::ics_document(&bytes),
        Format::Svg => Err(ConvertError::InvalidInput(
            "SVG is an image format; use an image target or an external renderer".into(),
        )),
        other => Err(ConvertError::Unsupported {
            from: other,
            to: Format::Txt,
        }),
    }
}

fn zip_entry(bytes: &[u8], name: &str) -> Result<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    let mut entry = archive
        .by_name(name)
        .map_err(|_| ConvertError::Archive(format!("missing ZIP entry '{name}'")))?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(text)
}

fn zip_names(bytes: &[u8]) -> Result<Vec<String>> {
    let archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    Ok(archive.file_names().map(str::to_string).collect())
}

pub fn docx_document(bytes: &[u8]) -> Result<Document> {
    let xml = zip_entry(bytes, "word/document.xml")?;
    let mut reader = Reader::from_str(&xml);
    let mut blocks = Vec::new();

    let mut in_paragraph = false;
    let mut in_text = false;
    let mut paragraph = String::new();
    let mut style = String::new();
    let mut outline_level: Option<u8> = None;
    let mut is_list = false;
    let mut table_depth = 0usize;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_cell = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                match event.local_name().as_ref() {
                    b"tbl" => {
                        table_depth += 1;
                        if table_depth == 1 {
                            table_rows.clear();
                        }
                    }
                    b"tr" if table_depth == 1 => row.clear(),
                    b"tc" if table_depth == 1 => {
                        cell.clear();
                        in_cell = true;
                    }
                    b"p" => {
                        in_paragraph = true;
                        paragraph.clear();
                        style.clear();
                        outline_level = None;
                        is_list = false;
                    }
                    b"pStyle" => {
                        if let Some(value) = attribute(&event, b"val") {
                            style = value;
                        }
                    }
                    b"outlineLvl" => {
                        if let Some(value) = attribute(&event, b"val") {
                            outline_level = value.parse().ok();
                        }
                    }
                    b"numPr" => is_list = true,
                    b"t" => in_text = true,
                    b"tab" => paragraph.push('\t'),
                    b"br" | b"cr" => paragraph.push('\n'),
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                if in_text && in_paragraph {
                    let decoded = text
                        .unescape()
                        .map(|value| value.into_owned())
                        .unwrap_or_else(|_| String::from_utf8_lossy(text.as_ref()).into_owned());
                    paragraph.push_str(&decoded);
                }
            }
            Ok(Event::CData(text)) => {
                if in_text && in_paragraph {
                    paragraph.push_str(&String::from_utf8_lossy(text.as_ref()));
                }
            }
            Ok(Event::End(event)) => match event.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" => {
                    in_paragraph = false;
                    let text = normalize_whitespace(&paragraph);
                    if table_depth > 0 && in_cell {
                        if !text.is_empty() {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(&text);
                        }
                    } else if !text.is_empty() {
                        let heading_level = outline_level
                            .map(|level| level.saturating_add(1))
                            .or_else(|| heading_level_from_style(&style));
                        if let Some(level) = heading_level {
                            blocks.push(Block::Heading {
                                level: level.clamp(1, 6),
                                text,
                            });
                        } else if is_list {
                            push_list_item(&mut blocks, text);
                        } else {
                            blocks.push(Block::Paragraph(text));
                        }
                    }
                }
                b"tc" if table_depth == 1 => {
                    row.push(cell.trim().to_string());
                    cell.clear();
                    in_cell = false;
                }
                b"tr" if table_depth == 1 => {
                    if !row.is_empty() {
                        table_rows.push(std::mem::take(&mut row));
                    }
                }
                b"tbl" => {
                    if table_depth == 1 {
                        if let Some((headers, rows)) = split_table(table_rows.clone()) {
                            blocks.push(Block::Table { headers, rows });
                        }
                        table_rows.clear();
                    }
                    table_depth = table_depth.saturating_sub(1);
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(ConvertError::InvalidFormat {
                    format: Format::Docx,
                    detail: format!("invalid word/document.xml: {error}"),
                })
            }
            _ => {}
        }
    }

    let title = zip_entry(bytes, "docProps/core.xml")
        .ok()
        .and_then(|core| simple_tag(&core, "title"))
        .filter(|title| !title.is_empty());

    Ok(Document { title, blocks })
}

fn heading_level_from_style(style: &str) -> Option<u8> {
    let lower = style.to_ascii_lowercase();
    if lower.contains("title") {
        return Some(1);
    }
    if !lower.contains("heading") && !lower.contains("titre") {
        return None;
    }
    let digits: String = lower.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u8>().ok().or(Some(1))
}

fn push_list_item(blocks: &mut Vec<Block>, text: String) {
    if let Some(Block::List { items, .. }) = blocks.last_mut() {
        items.push(text);
    } else {
        blocks.push(Block::List {
            ordered: false,
            items: vec![text],
        });
    }
}

fn split_table(rows: Vec<Vec<String>>) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let mut rows = rows;
    if rows.is_empty() {
        return None;
    }
    let headers = rows.remove(0);
    Some((headers, rows))
}

fn attribute(event: &quick_xml::events::BytesStart<'_>, local: &[u8]) -> Option<String> {
    for attribute in event.attributes().flatten() {
        if attribute.key.local_name().as_ref() == local {
            return attribute
                .unescape_value()
                .ok()
                .map(|value| value.into_owned());
        }
    }
    None
}

fn simple_tag(xml: &str, tag: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut capture = false;
    let mut value = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.local_name().as_ref() == tag.as_bytes() => {
                capture = true;
            }
            Ok(Event::Text(text)) if capture => {
                value.push_str(&text.unescape().ok()?);
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == tag.as_bytes() => {
                return Some(value.trim().to_string());
            }
            Ok(Event::Eof) => return None,
            _ => {}
        }
    }
}

pub fn odt_document(bytes: &[u8]) -> Result<Document> {
    let xml = zip_entry(bytes, "content.xml")?;
    let mut reader = Reader::from_str(&xml);
    let mut blocks = Vec::new();

    let mut in_heading = false;
    let mut heading_level = 1u8;
    let mut in_paragraph = false;
    let mut paragraph = String::new();
    let mut in_list = false;
    let mut list_item = String::new();
    let mut in_item = false;
    let mut table_depth = 0usize;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_cell = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                match event.local_name().as_ref() {
                    b"h" => {
                        in_heading = true;
                        paragraph.clear();
                        heading_level = attribute(&event, b"outline-level")
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(1);
                    }
                    b"p" => {
                        in_paragraph = true;
                        paragraph.clear();
                    }
                    b"list" => in_list = true,
                    b"list-item" => {
                        in_item = true;
                        list_item.clear();
                    }
                    b"table" => table_depth += 1,
                    b"table-row" if table_depth == 1 => row.clear(),
                    b"table-cell" if table_depth == 1 => {
                        cell.clear();
                        in_cell = true;
                    }
                    b"s" => {
                        paragraph.push(' ');
                        if in_cell {
                            cell.push(' ');
                        }
                    }
                    b"tab" => paragraph.push('\t'),
                    b"line-break" => paragraph.push('\n'),
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                let value = text
                    .unescape()
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(text.as_ref()).into_owned());
                if in_heading || in_paragraph {
                    paragraph.push_str(&value);
                }
            }
            Ok(Event::End(event)) => match event.local_name().as_ref() {
                b"h" => {
                    in_heading = false;
                    let text = normalize_whitespace(&paragraph);
                    if !text.is_empty() {
                        blocks.push(Block::Heading {
                            level: heading_level.clamp(1, 6),
                            text,
                        });
                    }
                }
                b"p" => {
                    in_paragraph = false;
                    let text = normalize_whitespace(&paragraph);
                    if table_depth > 0 && in_cell {
                        if !text.is_empty() {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(&text);
                        }
                    } else if in_item {
                        if !list_item.is_empty() {
                            list_item.push(' ');
                        }
                        list_item.push_str(&text);
                    } else if !text.is_empty() {
                        blocks.push(Block::Paragraph(text));
                    }
                }
                b"list-item" => {
                    if in_item {
                        let text = normalize_whitespace(&list_item);
                        if !text.is_empty() {
                            push_list_item(&mut blocks, text);
                        }
                    }
                    in_item = false;
                }
                b"list" => in_list = false,
                b"table-cell" if table_depth == 1 => {
                    row.push(cell.trim().to_string());
                    cell.clear();
                    in_cell = false;
                }
                b"table-row" if table_depth == 1 => {
                    if !row.is_empty() {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                b"table" => {
                    if table_depth == 1 {
                        if let Some((headers, data)) = split_table(std::mem::take(&mut rows)) {
                            blocks.push(Block::Table {
                                headers,
                                rows: data,
                            });
                        }
                    }
                    table_depth = table_depth.saturating_sub(1);
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(ConvertError::InvalidFormat {
                    format: Format::Odt,
                    detail: format!("invalid content.xml: {error}"),
                })
            }
            _ => {}
        }
    }
    let _ = in_list;
    Ok(Document {
        title: None,
        blocks,
    })
}

pub fn rtf_document(bytes: &[u8]) -> Result<Document> {
    let mut out = String::new();
    let mut index = 0usize;
    let mut skip_depth: Option<usize> = None;
    let mut depth = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if skip_depth.is_some_and(|skip| depth < skip) {
                    skip_depth = None;
                }
                index += 1;
            }
            b'\\' => {
                index += 1;
                if index >= bytes.len() {
                    break;
                }
                if bytes[index] == b'\'' {
                    if index + 2 < bytes.len() {
                        if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                            if let Ok(value) = u8::from_str_radix(hex, 16) {
                                let raw = [value];
                                let (text, _, _) = encoding_rs::WINDOWS_1252.decode(&raw);
                                let decoded = text.into_owned();
                                if skip_depth.is_none() {
                                    out.push_str(&decoded);
                                }
                            }
                        }
                        index += 3;
                    } else {
                        index += 1;
                    }
                    continue;
                }
                if !bytes[index].is_ascii_alphabetic() {
                    let literal = bytes[index] as char;
                    if skip_depth.is_none() {
                        out.push(literal);
                    }
                    index += 1;
                    continue;
                }
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                    index += 1;
                }
                let word = std::str::from_utf8(&bytes[start..index]).unwrap_or("");
                let mut sign = 1i32;
                if index < bytes.len() && bytes[index] == b'-' {
                    sign = -1;
                    index += 1;
                }
                let number_start = index;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                let number = std::str::from_utf8(&bytes[number_start..index])
                    .ok()
                    .and_then(|value| value.parse::<i32>().ok())
                    .map(|value| value * sign);
                if index < bytes.len() && bytes[index] == b' ' {
                    index += 1;
                }

                if matches!(
                    word,
                    "fonttbl"
                        | "colortbl"
                        | "stylesheet"
                        | "info"
                        | "pict"
                        | "object"
                        | "header"
                        | "footer"
                        | "themedata"
                        | "colorschememapping"
                        | "datastore"
                        | "latentstyles"
                        | "listtable"
                        | "listoverridetable"
                        | "rsidtbl"
                        | "generator"
                        | "xmlnstbl"
                ) {
                    skip_depth = Some(depth + 1);
                    continue;
                }
                if skip_depth.is_some() {
                    continue;
                }
                match word {
                    "par" | "line" => out.push('\n'),
                    "tab" => out.push('\t'),
                    "u" => {
                        if let Some(number) = number {
                            let code = if number < 0 {
                                (number + 65536) as u32
                            } else {
                                number as u32
                            };
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                            // Skip the ANSI fallback character (`?` or `\'hh`).
                            if index < bytes.len() && bytes[index] == b'?' {
                                index += 1;
                            } else if index + 3 < bytes.len() && bytes[index] == b'\\' {
                                index += 4;
                            }
                        }
                    }
                    "emdash" => out.push('—'),
                    "endash" => out.push('–'),
                    "lquote" => out.push('‘'),
                    "rquote" => out.push('’'),
                    "ldblquote" => out.push('“'),
                    "rdblquote" => out.push('”'),
                    "bullet" => out.push('•'),
                    _ => {}
                }
            }
            _ => {
                if skip_depth.is_none() {
                    if byte < 0x80 {
                        out.push(byte as char);
                    } else {
                        let raw = [byte];
                        let (text, _, _) = encoding_rs::WINDOWS_1252.decode(&raw);
                        let decoded = text.into_owned();
                        out.push_str(&decoded);
                    }
                }
                index += 1;
            }
        }
    }

    Ok(Document::from_plain_text(&out))
}

pub fn pdf_document(path: &Path, options: &ConversionOptions) -> Result<Document> {
    let text = pdf_extract::extract_text(path).map_err(|error| ConvertError::InvalidFormat {
        format: Format::Pdf,
        detail: error.to_string(),
    })?;
    if text.trim().is_empty() {
        return Err(ConvertError::InvalidFormat {
            format: Format::Pdf,
            detail: if options.ocr {
                "OCR was requested but the PDF text layer is empty; the OCR engine may not be installed or configured".into()
            } else {
                "the PDF has no extractable text layer; retry with options.ocr=true".into()
            },
        });
    }
    Ok(pdf_text_to_document(&text))
}

pub fn pdf_text_to_document(text: &str) -> Document {
    let mut blocks = Vec::new();
    let pages: Vec<&str> = text.split('\u{000c}').collect();
    for (page_index, page) in pages.iter().enumerate() {
        if page_index > 0 && !blocks.is_empty() {
            blocks.push(Block::PageBreak);
        }
        let lines: Vec<&str> = page.lines().collect();
        let mut index = 0usize;
        while index < lines.len() {
            let line = lines[index].trim();
            if line.is_empty() {
                index += 1;
                continue;
            }
            if looks_like_pdf_heading(line) {
                blocks.push(Block::Heading {
                    level: 2,
                    text: line.to_string(),
                });
                index += 1;
                continue;
            }
            let mut paragraph = vec![line.to_string()];
            index += 1;
            while index < lines.len() {
                let next = lines[index].trim();
                if next.is_empty() || looks_like_pdf_heading(next) {
                    break;
                }
                paragraph.push(next.to_string());
                index += 1;
            }
            blocks.push(Block::Paragraph(join_pdf_lines(&paragraph)));
        }
    }
    Document {
        title: None,
        blocks,
    }
}

fn looks_like_pdf_heading(line: &str) -> bool {
    if line.len() > 90 || line.ends_with('.') || line.ends_with(',') || line.ends_with(';') {
        return false;
    }
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return false;
    }
    let uppercase = letters.iter().filter(|c| c.is_uppercase()).count();
    let all_caps = uppercase == letters.len() && line.len() > 2;
    let numbered = line
        .split_once(' ')
        .map(|(head, _)| {
            head.trim_end_matches('.')
                .chars()
                .all(|c| c.is_ascii_digit())
        })
        .unwrap_or(false);
    all_caps || numbered
}

fn join_pdf_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        if out.ends_with('-') {
            out.pop();
            out.push_str(line);
        } else {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(line);
        }
    }
    out
}

pub fn epub_document(bytes: &[u8]) -> Result<Document> {
    let names = zip_names(bytes)?;
    let container = zip_entry(bytes, "META-INF/container.xml").ok();
    let opf_path = container
        .as_deref()
        .and_then(rootfile_path)
        .or_else(|| names.iter().find(|name| name.ends_with(".opf")).cloned())
        .ok_or_else(|| ConvertError::Archive("EPUB has no OPF package document".into()))?;
    let opf = zip_entry(bytes, &opf_path)?;
    let base = opf_path
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default();
    let title = opf_title(&opf);
    let chapters = opf_spine(&opf);
    let mut blocks = Vec::new();
    for chapter in chapters {
        let path = join_zip_path(&base, &chapter);
        let Ok(content) = zip_entry(bytes, &path) else {
            continue;
        };
        let chapter_document = document_from_html(&content);
        if chapter_document.is_empty() {
            continue;
        }
        if !blocks.is_empty() {
            blocks.push(Block::PageBreak);
        }
        blocks.extend(chapter_document.blocks);
    }
    if blocks.is_empty() {
        return Err(ConvertError::InvalidFormat {
            format: Format::Epub,
            detail: "EPUB spine contained no readable chapters".into(),
        });
    }
    Ok(Document { title, blocks })
}

fn rootfile_path(container_xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(container_xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() == b"rootfile" {
                    if let Some(path) = attribute(&event, b"full-path") {
                        return Some(path);
                    }
                }
            }
            Ok(Event::Eof) => return None,
            _ => {}
        }
    }
}

fn opf_title(opf: &str) -> Option<String> {
    simple_tag(opf, "title").filter(|title| !title.is_empty())
}

fn opf_spine(opf: &str) -> Vec<String> {
    let mut reader = Reader::from_str(opf);
    let mut manifest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut spine: Vec<String> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                match event.local_name().as_ref() {
                    b"item" => {
                        if let (Some(id), Some(href)) =
                            (attribute(&event, b"id"), attribute(&event, b"href"))
                        {
                            manifest.insert(id, href);
                        }
                    }
                    b"itemref" => {
                        if let Some(idref) = attribute(&event, b"idref") {
                            spine.push(idref);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }
    spine
        .into_iter()
        .filter_map(|id| manifest.get(&id).cloned())
        .collect()
}

fn join_zip_path(base: &str, path: &str) -> String {
    if base.is_empty() {
        return path.trim_start_matches('/').to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

pub fn pptx_document(bytes: &[u8]) -> Result<Document> {
    let names = zip_names(bytes)?;
    let mut slides: Vec<(usize, String)> = names
        .iter()
        .filter_map(|name| {
            let rest = name.strip_prefix("ppt/slides/slide")?;
            let number = rest.strip_suffix(".xml")?;
            Some((number.parse::<usize>().ok()?, name.clone()))
        })
        .collect();
    slides.sort_by_key(|(number, _)| *number);

    let mut blocks = Vec::new();
    for (index, (number, name)) in slides.iter().enumerate() {
        let xml = zip_entry(bytes, name)?;
        let mut paragraphs = Vec::new();
        let mut reader = Reader::from_str(&xml);
        let mut paragraph = String::new();
        let mut in_paragraph = false;
        let mut in_text = false;
        loop {
            match reader.read_event() {
                Ok(Event::Start(event)) => match event.local_name().as_ref() {
                    b"p" => {
                        in_paragraph = true;
                        paragraph.clear();
                    }
                    b"t" => in_text = true,
                    b"br" => paragraph.push('\n'),
                    _ => {}
                },
                Ok(Event::Text(text)) => {
                    if in_text && in_paragraph {
                        if let Ok(value) = text.unescape() {
                            paragraph.push_str(&value);
                        }
                    }
                }
                Ok(Event::End(event)) => match event.local_name().as_ref() {
                    b"t" => in_text = false,
                    b"p" => {
                        in_paragraph = false;
                        let text = normalize_whitespace(&paragraph);
                        if !text.is_empty() {
                            paragraphs.push(text);
                        }
                    }
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(error) => {
                    return Err(ConvertError::InvalidFormat {
                        format: Format::Pptx,
                        detail: format!("invalid {name}: {error}"),
                    })
                }
                _ => {}
            }
        }

        let notes_name = format!("ppt/notesSlides/notesSlide{number}.xml");
        let notes = zip_entry(bytes, &notes_name)
            .ok()
            .map(|xml| slide_text(&xml));
        let notes = notes
            .unwrap_or_default()
            .into_iter()
            .filter(|text| !paragraphs.contains(text))
            .collect::<Vec<_>>();

        if index > 0 {
            blocks.push(Block::PageBreak);
        }
        if let Some(first) = paragraphs.first() {
            blocks.push(Block::Heading {
                level: 1,
                text: first.clone(),
            });
            for paragraph in paragraphs.iter().skip(1) {
                blocks.push(Block::Paragraph(paragraph.clone()));
            }
        } else {
            blocks.push(Block::Heading {
                level: 1,
                text: format!("Slide {number}"),
            });
        }
        if !notes.is_empty() {
            blocks.push(Block::Quote(notes.join("\n")));
        }
    }

    if blocks.is_empty() {
        return Err(ConvertError::InvalidFormat {
            format: Format::Pptx,
            detail: "presentation contained no slides".into(),
        });
    }
    Ok(Document {
        title: None,
        blocks,
    })
}

fn slide_text(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                if event.local_name().as_ref() == b"t" {
                    in_text = true;
                }
            }
            Ok(Event::Text(text)) if in_text => {
                if let Ok(value) = text.unescape() {
                    current.push_str(&value);
                }
            }
            Ok(Event::End(event)) => {
                if event.local_name().as_ref() == b"t" {
                    in_text = false;
                    let value = normalize_whitespace(&current);
                    if !value.is_empty() {
                        out.push(value);
                    }
                    current.clear();
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }
    out
}

pub fn eml_document(bytes: &[u8]) -> Result<Document> {
    let message = mail_parser::MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| ConvertError::InvalidFormat {
            format: Format::Eml,
            detail: "could not parse RFC 822 message".into(),
        })?;

    let subject = message.subject().unwrap_or("(no subject)").to_string();
    let from = message
        .from()
        .and_then(|address| address.first())
        .map(|address| {
            address
                .name()
                .map(|name| format!("{name} <{}>", address.address().unwrap_or("")))
                .unwrap_or_else(|| address.address().unwrap_or("").to_string())
        })
        .unwrap_or_default();
    let to = message
        .to()
        .and_then(|address| address.first())
        .and_then(|address| address.address())
        .unwrap_or("")
        .to_string();
    let date = message
        .date()
        .map(|date| date.to_rfc3339())
        .unwrap_or_default();

    let mut blocks = vec![
        Block::Heading {
            level: 1,
            text: subject.clone(),
        },
        Block::Paragraph(format!("From: {from}\nTo: {to}\nDate: {date}")),
    ];

    let body_added = if let Some(html) = message.body_html(0) {
        let document = document_from_html(&html);
        if !document.is_empty() {
            blocks.extend(document.blocks);
            true
        } else {
            false
        }
    } else {
        false
    };
    if !body_added {
        if let Some(text) = message.body_text(0) {
            let document = Document::from_plain_text(&text);
            blocks.extend(document.blocks);
        }
    }

    let attachments: Vec<String> = message
        .attachments()
        .filter_map(|attachment| attachment.attachment_name().map(str::to_string))
        .collect();
    if !attachments.is_empty() {
        blocks.push(Block::Heading {
            level: 2,
            text: "Attachments".into(),
        });
        blocks.push(Block::List {
            ordered: false,
            items: attachments,
        });
    }

    Ok(Document {
        title: Some(subject),
        blocks,
    })
}

pub fn msg_document(bytes: &[u8]) -> Result<Document> {
    let outlook =
        msg_parser::Outlook::from_slice(bytes).map_err(|error| ConvertError::InvalidFormat {
            format: Format::Msg,
            detail: format!("cannot parse Outlook MSG: {error}"),
        })?;

    let subject = if outlook.subject.trim().is_empty() {
        "(no subject)".to_string()
    } else {
        outlook.subject.clone()
    };
    let person = |person: &msg_parser::Person| {
        if person.name.is_empty() {
            person.email.clone()
        } else if person.email.is_empty() {
            person.name.clone()
        } else {
            format!("{} <{}>", person.name, person.email)
        }
    };
    let from = person(&outlook.sender);
    let to = outlook.to.iter().map(person).collect::<Vec<_>>().join(", ");
    let cc = outlook.cc.iter().map(person).collect::<Vec<_>>().join(", ");
    let date = if outlook.message_delivery_time.trim().is_empty() {
        outlook.creation_time.clone()
    } else {
        outlook.message_delivery_time.clone()
    };

    let mut blocks = vec![
        Block::Heading {
            level: 1,
            text: subject.clone(),
        },
        Block::Paragraph(format!("From: {from}\nTo: {to}\nCc: {cc}\nDate: {date}")),
    ];

    let mut body_added = false;
    if !outlook.html.trim().is_empty() {
        let document = document_from_html(&outlook.html);
        if !document.is_empty() {
            blocks.extend(document.blocks);
            body_added = true;
        }
    }
    if !body_added && !outlook.body.trim().is_empty() {
        blocks.extend(Document::from_plain_text(&outlook.body).blocks);
        body_added = true;
    }
    if !body_added {
        if let Some(html) = outlook.html_from_rtf() {
            let document = document_from_html(&html);
            blocks.extend(document.blocks);
        }
    }

    let attachments: Vec<String> = outlook
        .attachments
        .iter()
        .filter_map(|attachment| {
            let name = if !attachment.long_file_name.is_empty() {
                &attachment.long_file_name
            } else if !attachment.display_name.is_empty() {
                &attachment.display_name
            } else if !attachment.file_name.is_empty() {
                &attachment.file_name
            } else {
                return None;
            };
            let kind = if attachment.is_embedded_message() {
                " (embedded message)"
            } else {
                ""
            };
            Some(format!("{name}{kind}"))
        })
        .collect();
    if !attachments.is_empty() {
        blocks.push(Block::Heading {
            level: 2,
            text: "Attachments".into(),
        });
        blocks.push(Block::List {
            ordered: false,
            items: attachments,
        });
    }

    Ok(Document {
        title: Some(subject),
        blocks,
    })
}

pub fn tex_document(text: &str) -> Document {
    let mut blocks = Vec::new();
    let mut body = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('%') {
            continue;
        }
        if trimmed.starts_with("\\documentclass")
            || trimmed.starts_with("\\usepackage")
            || trimmed.starts_with("\\begin{document}")
            || trimmed.starts_with("\\end{document}")
            || trimmed.starts_with("\\title")
            || trimmed.starts_with("\\author")
            || trimmed.starts_with("\\date")
        {
            continue;
        }
        if let Some((level, heading)) = tex_heading(trimmed) {
            if !body.trim().is_empty() {
                blocks.push(Block::Paragraph(clean_tex(&body)));
                body.clear();
            }
            blocks.push(Block::Heading {
                level,
                text: heading,
            });
            continue;
        }
        body.push_str(trimmed);
        body.push('\n');
        if trimmed.is_empty() {
            let paragraph = clean_tex(&body);
            if !paragraph.trim().is_empty() {
                blocks.push(Block::Paragraph(paragraph));
            }
            body.clear();
        }
    }
    if !body.trim().is_empty() {
        blocks.push(Block::Paragraph(clean_tex(&body)));
    }
    Document {
        title: None,
        blocks,
    }
}

fn tex_heading(line: &str) -> Option<(u8, String)> {
    for (command, level) in [
        ("\\section", 1),
        ("\\subsection", 2),
        ("\\subsubsection", 3),
        ("\\paragraph", 4),
    ] {
        if let Some(rest) = line.strip_prefix(command) {
            let rest = rest.trim_start_matches('*').trim();
            let start = rest.find('{')?;
            let end = rest.rfind('}')?;
            if end > start {
                return Some((level, clean_tex(&rest[start + 1..end])));
            }
        }
    }
    None
}

fn clean_tex(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let mut command = String::new();
            while let Some(next) = chars.peek() {
                if next.is_ascii_alphabetic() {
                    command.push(*next);
                    chars.next();
                } else {
                    break;
                }
            }
            match command.as_str() {
                "" => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                "par" | "newline" | "linebreak" => out.push('\n'),
                "textbf" | "textit" | "emph" | "underline" | "texttt" | "textrm" | "textsf" => {}
                "item" => out.push_str("- "),
                "begin" | "end" if chars.peek() == Some(&'{') => {
                    // Skip the environment name in braces.
                    let mut depth = 0usize;
                    for next in chars.by_ref() {
                        if next == '{' {
                            depth += 1;
                        } else if next == '}' {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                }
                "begin" | "end" => {}
                _ => {}
            }
        } else if ch == '{' || ch == '}' {
            // Keep group content, drop delimiters.
        } else if ch == '~' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    normalize_whitespace(&out)
}

/// Read a slide/notes XML fragment into plain paragraphs; exported for tests.
pub fn xml_text_fragments(xml: &str) -> Vec<String> {
    slide_text(xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_heading_heuristics() {
        let document = pdf_text_to_document("1. INTRODUCTION\nThis is a sentence that wraps\nacross lines.\n\nRESULTS\nWe found a thing.");
        assert!(document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Heading { .. })));
        assert!(document.plain_text().contains("wraps across lines"));
    }

    #[test]
    fn tex_reader_extracts_sections() {
        let document = tex_document(
            "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\nHello \\textbf{world}.\n\\end{document}",
        );
        assert!(document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Heading { text, .. } if text == "Intro")));
        assert!(document.plain_text().contains("world"));
    }

    #[test]
    fn rtf_reader_handles_unicode_and_paragraphs() {
        let document = rtf_document(br"{\rtf1\ansi Hello\par World \u1046?}").unwrap();
        assert!(document.plain_text().contains("Hello"));
        assert!(document.plain_text().contains('Ж'));
    }
}
