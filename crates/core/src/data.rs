//! Spreadsheets, structured data, contacts, calendars, and image helpers.

use crate::document::read::pdf_text_to_document;
use crate::document::{decode_text, Block, Document};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use calamine::{open_workbook_auto, Data, Reader};
use rust_xlsxwriter::{Color, Format as XlsxFormat, Workbook as XlsxWorkbook};
use std::io::{Cursor, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

impl Workbook {
    pub fn first(&self) -> Option<&Sheet> {
        self.sheets.first()
    }

    pub fn ensure_headers(&mut self) {
        for sheet in &mut self.sheets {
            let width = sheet
                .rows
                .iter()
                .map(Vec::len)
                .max()
                .unwrap_or(sheet.headers.len());
            if sheet.headers.is_empty() {
                sheet.headers = (1..=width).map(|index| format!("Column {index}")).collect();
            }
            for row in &mut sheet.rows {
                row.resize(width, String::new());
            }
        }
    }
}

pub fn read_workbook(path: &Path, from: Format, options: &ConversionOptions) -> Result<Workbook> {
    match from {
        Format::Csv => read_delimited(path, b',', options),
        Format::Tsv => read_delimited(path, b'\t', options),
        Format::Xlsx | Format::Xls | Format::Ods => read_calamine(path),
        Format::Json => {
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            Ok(json_to_workbook(&value, "Sheet1"))
        }
        Format::Xml => {
            let xml = decode_text(&std::fs::read(path)?);
            let value = xml_to_json(&xml)?;
            Ok(json_to_workbook(&value, "Sheet1"))
        }
        Format::Pdf => {
            let document =
                pdf_text_to_document(&pdf_extract::extract_text(path).map_err(|error| {
                    ConvertError::InvalidFormat {
                        format: Format::Pdf,
                        detail: error.to_string(),
                    }
                })?);
            Ok(text_to_workbook(&document, options))
        }
        other => Err(ConvertError::Unsupported {
            from: other,
            to: Format::Csv,
        }),
    }
}

fn read_delimited(path: &Path, delimiter: u8, options: &ConversionOptions) -> Result<Workbook> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(options.csv.has_headers)
        .flexible(true)
        .from_path(path)
        .map_err(|error| ConvertError::InvalidInput(format!("invalid delimited file: {error}")))?;
    let headers = if options.csv.has_headers {
        reader
            .headers()
            .map_err(|error| ConvertError::InvalidInput(error.to_string()))?
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
        rows.push(record.iter().map(|value| value.to_string()).collect());
    }
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Sheet1")
        .to_string();
    let mut workbook = Workbook {
        sheets: vec![Sheet {
            name,
            headers,
            rows,
        }],
    };
    workbook.ensure_headers();
    Ok(workbook)
}

fn read_calamine(path: &Path) -> Result<Workbook> {
    let mut archive = open_workbook_auto(path)
        .map_err(|error| ConvertError::InvalidInput(format!("cannot open spreadsheet: {error}")))?;
    let names = archive.sheet_names().to_vec();
    let mut sheets = Vec::new();
    for name in names {
        let range = archive.worksheet_range(&name).map_err(|error| {
            ConvertError::InvalidInput(format!("cannot read sheet '{name}': {error}"))
        })?;
        let mut rows: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(cell_to_string).collect())
            .collect();
        let headers = if rows.is_empty() {
            Vec::new()
        } else {
            rows.remove(0)
        };
        let mut sheet = Sheet {
            name,
            headers,
            rows,
        };
        if sheet.headers.iter().all(|header| header.trim().is_empty()) {
            sheet.headers = (1..=sheet.headers.len())
                .map(|index| format!("Column {index}"))
                .collect();
        }
        sheets.push(sheet);
    }
    let mut workbook = Workbook { sheets };
    workbook.ensure_headers();
    Ok(workbook)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(value) => value.clone(),
        Data::Float(value) => {
            if value.fract() == 0.0 && value.abs() < 1e15 {
                format!("{}", *value as i64)
            } else {
                value.to_string()
            }
        }
        Data::Int(value) => value.to_string(),
        Data::Bool(value) => value.to_string(),
        Data::DateTime(value) => value.to_string(),
        Data::DateTimeIso(value) => value.clone(),
        Data::DurationIso(value) => value.clone(),
        Data::Error(value) => format!("{value:?}"),
    }
}

pub fn write_workbook(
    path: &Path,
    workbook: &Workbook,
    to: Format,
    options: &ConversionOptions,
) -> Result<()> {
    match to {
        Format::Xlsx => write_xlsx(path, workbook),
        Format::Ods => write_ods(path, workbook),
        Format::Csv => write_delimited(path, workbook, b',', options),
        Format::Tsv => write_delimited(path, workbook, b'\t', options),
        Format::Json => {
            let value = workbook_to_json(workbook);
            std::fs::write(
                path,
                serde_json::to_vec_pretty(&value)
                    .map_err(|error| ConvertError::Serialization(error.to_string()))?,
            )?;
            Ok(())
        }
        Format::Xml => {
            let value = workbook_to_json(workbook);
            std::fs::write(path, json_to_xml(&value, "rows"))?;
            Ok(())
        }
        Format::Html => {
            std::fs::write(path, workbook_to_html(workbook))?;
            Ok(())
        }
        Format::Markdown => {
            std::fs::write(path, workbook_to_markdown(workbook))?;
            Ok(())
        }
        Format::Pdf | Format::Docx | Format::Txt => {
            let document = workbook_document_from(workbook);
            crate::document::write::write_document(path, &document, to, options)
        }
        other => Err(ConvertError::Unsupported {
            from: Format::Csv,
            to: other,
        }),
    }
}

fn write_xlsx(path: &Path, workbook: &Workbook) -> Result<()> {
    let mut output = XlsxWorkbook::new();
    let header_format = XlsxFormat::new()
        .set_bold()
        .set_background_color(Color::RGB(0xE8EDF2))
        .set_border(rust_xlsxwriter::FormatBorder::Thin);
    for sheet in &workbook.sheets {
        let worksheet = output.add_worksheet();
        worksheet
            .set_name(&sheet.name)
            .map_err(|error| ConvertError::Internal(error.to_string()))?;
        for (column, header) in sheet.headers.iter().enumerate() {
            worksheet
                .write_string_with_format(0, column as u16, header, &header_format)
                .map_err(|error| ConvertError::Internal(error.to_string()))?;
        }
        for (row_index, row) in sheet.rows.iter().enumerate() {
            for (column, value) in row.iter().enumerate() {
                if value.is_empty() {
                    continue;
                }
                worksheet
                    .write_string(row_index as u32 + 1, column as u16, value)
                    .map_err(|error| ConvertError::Internal(error.to_string()))?;
            }
        }
        for column in 0..sheet.headers.len() {
            worksheet
                .set_column_width(column as u16, 22)
                .map_err(|error| ConvertError::Internal(error.to_string()))?;
        }
        worksheet.set_freeze_panes(1, 0).ok();
    }
    output
        .save(path)
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    Ok(())
}

fn write_delimited(
    path: &Path,
    workbook: &Workbook,
    delimiter: u8,
    options: &ConversionOptions,
) -> Result<()> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_path(path)
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    if let Some(sheet) = workbook.first() {
        if options.csv.has_headers && !sheet.headers.is_empty() {
            writer
                .write_record(&sheet.headers)
                .map_err(|error| ConvertError::Internal(error.to_string()))?;
        }
        for row in &sheet.rows {
            writer
                .write_record(row)
                .map_err(|error| ConvertError::Internal(error.to_string()))?;
        }
    }
    writer
        .flush()
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    Ok(())
}

fn write_ods(path: &Path, workbook: &Workbook) -> Result<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored)
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    zip.write_all(b"application/vnd.oasis.opendocument.spreadsheet")?;

    let mut content = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" office:version=\"1.2\"><office:body><office:spreadsheet>",
    );
    for sheet in &workbook.sheets {
        content.push_str(&format!(
            "<table:table table:name=\"{}\">",
            crate::document::write::xml_escape(&sheet.name)
        ));
        let columns = sheet.headers.len().max(1);
        content.push_str(&format!(
            "<table:table-column table:number-columns-repeated=\"{columns}\"/>"
        ));
        if !sheet.headers.is_empty() {
            content.push_str("<table:table-row>");
            for header in &sheet.headers {
                content.push_str(&ods_cell(header));
            }
            content.push_str("</table:table-row>");
        }
        for row in &sheet.rows {
            content.push_str("<table:table-row>");
            for index in 0..columns {
                content.push_str(&ods_cell(row.get(index).map(String::as_str).unwrap_or("")));
            }
            content.push_str("</table:table-row>");
        }
        content.push_str("</table:table>");
    }
    content.push_str("</office:spreadsheet></office:body></office:document-content>");
    write_zip_text(&mut zip, "content.xml", &content)?;
    write_zip_text(
        &mut zip,
        "styles.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" office:version=\"1.2\"><office:styles/></office:document-styles>",
    )?;
    write_zip_text(
        &mut zip,
        "meta.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-meta xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" office:version=\"1.2\"><office:meta><meta:generator>DocForge</meta:generator></office:meta></office:document-meta>",
    )?;
    write_zip_text(
        &mut zip,
        "META-INF/manifest.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.2\"><manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.spreadsheet\"/><manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/><manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/></manifest:manifest>",
    )?;
    let cursor = zip
        .finish()
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    std::fs::write(path, cursor.into_inner())?;
    Ok(())
}

fn ods_cell(value: &str) -> String {
    if let Ok(number) = value.parse::<f64>() {
        format!(
            "<table:table-cell office:value-type=\"float\" office:value=\"{number}\"><text:p>{number}</text:p></table:table-cell>"
        )
    } else {
        format!(
            "<table:table-cell office:value-type=\"string\"><text:p>{}</text:p></table:table-cell>",
            crate::document::write::xml_escape(value)
        )
    }
}

fn write_zip_text(zip: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, data: &str) -> Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(name, options)
        .map_err(|error| ConvertError::Archive(error.to_string()))?;
    zip.write_all(data.as_bytes())?;
    Ok(())
}

pub fn workbook_document(
    path: &Path,
    from: Format,
    options: &ConversionOptions,
) -> Result<Document> {
    let workbook = read_workbook(path, from, options)?;
    Ok(workbook_document_from(&workbook))
}

pub fn workbook_document_from(workbook: &Workbook) -> Document {
    let mut document = Document::new(
        workbook
            .first()
            .map(|sheet| sheet.name.clone())
            .unwrap_or_else(|| "Workbook".to_string()),
    );
    for sheet in &workbook.sheets {
        if workbook.sheets.len() > 1 {
            document.push(Block::Heading {
                level: 1,
                text: sheet.name.clone(),
            });
        }
        document.push(Block::Table {
            headers: sheet.headers.clone(),
            rows: sheet.rows.clone(),
        });
    }
    document
}

fn workbook_to_markdown(workbook: &Workbook) -> String {
    let mut out = String::new();
    for sheet in &workbook.sheets {
        out.push_str(&format!("# {}\n\n", sheet.name));
        if !sheet.headers.is_empty() {
            out.push('|');
            for header in &sheet.headers {
                out.push_str(&format!(" {} |", header.replace('|', "\\|")));
            }
            out.push('\n');
            out.push('|');
            for _ in &sheet.headers {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
        for row in &sheet.rows {
            out.push('|');
            for index in 0..sheet.headers.len() {
                out.push_str(&format!(
                    " {} |",
                    row.get(index)
                        .map(|value| value.replace('|', "\\|"))
                        .unwrap_or_default()
                ));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn workbook_to_html(workbook: &Workbook) -> String {
    let mut out = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Workbook</title><style>body{font-family:system-ui,sans-serif;margin:2rem}table{border-collapse:collapse;margin-bottom:2rem}td,th{border:1px solid #ccd3da;padding:.35rem .55rem}th{background:#eef2f6}</style></head><body>",
    );
    for sheet in &workbook.sheets {
        out.push_str(&format!(
            "<h2>{}</h2><table>",
            crate::document::escape_html(&sheet.name)
        ));
        if !sheet.headers.is_empty() {
            out.push_str("<thead><tr>");
            for header in &sheet.headers {
                out.push_str(&format!(
                    "<th>{}</th>",
                    crate::document::escape_html(header)
                ));
            }
            out.push_str("</tr></thead>");
        }
        out.push_str("<tbody>");
        for row in &sheet.rows {
            out.push_str("<tr>");
            for index in 0..sheet.headers.len() {
                out.push_str(&format!(
                    "<td>{}</td>",
                    crate::document::escape_html(row.get(index).map(String::as_str).unwrap_or(""))
                ));
            }
            out.push_str("</tr>");
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</body></html>");
    out
}

pub fn json_to_workbook(value: &serde_json::Value, name: &str) -> Workbook {
    let array = match value {
        serde_json::Value::Array(values) => values.clone(),
        serde_json::Value::Object(_) => vec![value.clone()],
        _ => vec![value.clone()],
    };
    let mut headers: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in &array {
        if let serde_json::Value::Object(object) = item {
            for key in object.keys() {
                if seen.insert(key.clone()) {
                    headers.push(key.clone());
                }
            }
        }
    }
    let rows = if headers.is_empty() {
        headers.push("value".to_string());
        array
            .iter()
            .map(|item| vec![flatten_cell(Some(item))])
            .collect()
    } else {
        array
            .iter()
            .map(|item| {
                headers
                    .iter()
                    .map(|header| flatten_cell(item.get(header)))
                    .collect()
            })
            .collect()
    };
    Workbook {
        sheets: vec![Sheet {
            name: name.to_string(),
            headers,
            rows,
        }],
    }
}

fn flatten_cell(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Bool(value)) => value.to_string(),
        Some(serde_json::Value::Number(value)) => value.to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub fn workbook_to_json(workbook: &Workbook) -> serde_json::Value {
    let Some(sheet) = workbook.first() else {
        return serde_json::Value::Array(Vec::new());
    };
    let rows: Vec<serde_json::Value> = sheet
        .rows
        .iter()
        .map(|row| {
            if sheet.headers.is_empty() {
                serde_json::Value::Array(row.iter().map(|value| infer_json_scalar(value)).collect())
            } else {
                let mut object = serde_json::Map::new();
                for (index, header) in sheet.headers.iter().enumerate() {
                    object.insert(
                        header.clone(),
                        infer_json_scalar(row.get(index).map(String::as_str).unwrap_or("")),
                    );
                }
                serde_json::Value::Object(object)
            }
        })
        .collect();
    serde_json::Value::Array(rows)
}

fn infer_json_scalar(value: &str) -> serde_json::Value {
    if value.is_empty() {
        return serde_json::Value::Null;
    }
    if let Ok(boolean) = value.parse::<bool>() {
        return serde_json::Value::Bool(boolean);
    }
    if let Ok(number) = value.parse::<i64>() {
        return serde_json::Value::Number(number.into());
    }
    if let Ok(number) = value.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return serde_json::Value::Number(number);
        }
    }
    serde_json::Value::String(value.to_string())
}

pub fn xml_to_json(xml: &str) -> Result<serde_json::Value> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| ConvertError::Serialization(format!("invalid XML: {error}")))?;
    let root = document.root_element();
    let value = xml_node_value(root);
    let mut object = serde_json::Map::new();
    object.insert(root.tag_name().name().to_string(), value);
    Ok(serde_json::Value::Object(object))
}

fn xml_node_value(node: roxmltree::Node<'_, '_>) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    for attribute in node.attributes() {
        object.insert(
            format!("@{}", attribute.name()),
            serde_json::Value::String(attribute.value().to_string()),
        );
    }
    let children: Vec<_> = node.children().filter(|child| child.is_element()).collect();
    if children.is_empty() {
        let text = node.text().unwrap_or("").trim();
        if object.is_empty() {
            return serde_json::Value::String(text.to_string());
        }
        if !text.is_empty() {
            object.insert(
                "#text".to_string(),
                serde_json::Value::String(text.to_string()),
            );
        }
        return serde_json::Value::Object(object);
    }
    for child in children {
        let key = child.tag_name().name().to_string();
        let value = xml_node_value(child);
        match object.get_mut(&key) {
            Some(serde_json::Value::Array(values)) => values.push(value),
            Some(existing) => {
                let previous = existing.take();
                *existing = serde_json::Value::Array(vec![previous, value]);
            }
            None => {
                object.insert(key, value);
            }
        }
    }
    serde_json::Value::Object(object)
}

pub fn json_to_xml(value: &serde_json::Value, root: &str) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    write_xml_node(&mut out, root, value, 0);
    out
}

fn write_xml_node(out: &mut String, name: &str, value: &serde_json::Value, depth: usize) {
    let indent = "  ".repeat(depth);
    let name = sanitize_xml_name(name);
    match value {
        serde_json::Value::Object(object) => {
            out.push_str(&format!("{indent}<{name}>"));
            let has_children = object
                .keys()
                .any(|key| !key.starts_with('@') && key != "#text");
            if has_children {
                out.push('\n');
            }
            for (key, child) in object {
                if let Some(attribute) = key.strip_prefix('@') {
                    out.push_str(&format!(
                        " {}=\"{}\"",
                        sanitize_xml_name(attribute),
                        crate::document::write::xml_escape(&scalar_string(child))
                    ));
                }
            }
            if has_children {
                out.push('\n');
            }
            for (key, child) in object {
                if key.starts_with('@') {
                    continue;
                }
                if key == "#text" {
                    out.push_str(&crate::document::write::xml_escape(&scalar_string(child)));
                    continue;
                }
                match child {
                    serde_json::Value::Array(values) => {
                        for item in values {
                            write_xml_node(out, key, item, depth + 1);
                        }
                    }
                    _ => write_xml_node(out, key, child, depth + 1),
                }
            }
            if has_children {
                out.push_str(&indent);
            }
            out.push_str(&format!("</{name}>\n"));
        }
        serde_json::Value::Array(values) => {
            for item in values {
                write_xml_node(out, &name, item, depth);
            }
        }
        serde_json::Value::Null => {
            out.push_str(&format!("{indent}<{name}/>\n"));
        }
        _ => {
            out.push_str(&format!(
                "{indent}<{name}>{}</{name}>\n",
                crate::document::write::xml_escape(&scalar_string(value))
            ));
        }
    }
}

fn sanitize_xml_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("item");
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn scalar_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn text_to_workbook(document: &Document, options: &ConversionOptions) -> Workbook {
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    for block in &document.blocks {
        if let Block::Table {
            headers: table_headers,
            rows: table_rows,
        } = block
        {
            if headers.is_empty() {
                headers = table_headers.clone();
            }
            rows.extend(table_rows.clone());
        }
    }
    if rows.is_empty() {
        let text = document.plain_text();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let cells: Vec<String> = if line.contains('\t') {
                line.split('\t')
                    .map(|cell| cell.trim().to_string())
                    .collect()
            } else {
                split_on_runs(line)
            };
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        if rows.is_empty() {
            return Workbook::default();
        }
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        for row in &mut rows {
            row.resize(width, String::new());
        }
        if options.csv.has_headers && rows.len() > 1 {
            headers = rows.remove(0);
        } else {
            headers = (1..=width).map(|index| format!("Column {index}")).collect();
        }
    }
    let mut workbook = Workbook {
        sheets: vec![Sheet {
            name: "Sheet1".to_string(),
            headers,
            rows,
        }],
    };
    workbook.ensure_headers();
    workbook
}

fn split_on_runs(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut spaces = 0usize;
    for ch in line.chars() {
        if ch == ' ' {
            spaces += 1;
            if spaces >= 2 {
                if !current.trim().is_empty() {
                    cells.push(current.trim().to_string());
                    current.clear();
                }
                spaces = 0;
            } else {
                current.push(ch);
            }
        } else {
            spaces = 0;
            current.push(ch);
        }
    }
    if !current.trim().is_empty() {
        cells.push(current.trim().to_string());
    }
    if cells.is_empty() {
        cells.push(line.trim().to_string());
    }
    cells
}

// ---------- PDF table extraction ----------

pub fn pdf_table_workbook(path: &Path, options: &ConversionOptions) -> Result<Workbook> {
    let text = pdf_extract::extract_text(path).map_err(|error| ConvertError::InvalidFormat {
        format: Format::Pdf,
        detail: error.to_string(),
    })?;
    let document = pdf_text_to_document(&text);
    Ok(text_to_workbook(&document, options))
}

// ---------- vCard and iCalendar ----------

#[derive(Debug, Clone, Default)]
struct Contact {
    name: String,
    first_name: String,
    last_name: String,
    email: String,
    phone: String,
    organization: String,
    title: String,
    address: String,
    note: String,
    url: String,
}

pub fn vcf_document(bytes: &[u8]) -> Result<Document> {
    let contacts = parse_vcf(&decode_text(bytes))?;
    let mut document = Document::new("Contacts");
    let headers = contact_headers();
    let rows = contacts.iter().map(contact_row).collect::<Vec<_>>();
    document.push(Block::Table { headers, rows });
    Ok(document)
}

pub fn vcf_to_csv(bytes: &[u8], path: &Path) -> Result<()> {
    let contacts = parse_vcf(&decode_text(bytes))?;
    let mut writer =
        csv::Writer::from_path(path).map_err(|error| ConvertError::Internal(error.to_string()))?;
    writer
        .write_record(contact_headers())
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    for contact in contacts {
        writer
            .write_record(contact_row(&contact))
            .map_err(|error| ConvertError::Internal(error.to_string()))?;
    }
    writer
        .flush()
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    Ok(())
}

fn contact_headers() -> Vec<String> {
    [
        "name",
        "first_name",
        "last_name",
        "email",
        "phone",
        "organization",
        "title",
        "address",
        "note",
        "url",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect()
}

fn contact_row(contact: &Contact) -> Vec<String> {
    vec![
        contact.name.clone(),
        contact.first_name.clone(),
        contact.last_name.clone(),
        contact.email.clone(),
        contact.phone.clone(),
        contact.organization.clone(),
        contact.title.clone(),
        contact.address.clone(),
        contact.note.clone(),
        contact.url.clone(),
    ]
}

fn parse_vcf(text: &str) -> Result<Vec<Contact>> {
    let lines = unfold_lines(text);
    let mut contacts = Vec::new();
    let mut current: Option<Contact> = None;
    for line in lines {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:VCARD") {
            current = Some(Contact::default());
            continue;
        }
        if upper.starts_with("END:VCARD") {
            if let Some(contact) = current.take() {
                contacts.push(contact);
            }
            continue;
        }
        let Some(contact) = current.as_mut() else {
            continue;
        };
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let property = name.split(';').next().unwrap_or(name).to_ascii_uppercase();
        let value = decode_vcard_value(name, value)?;
        match property.as_str() {
            "FN" => contact.name = value,
            "N" => {
                let parts: Vec<&str> = value.split(';').collect();
                contact.last_name = parts.first().unwrap_or(&"").to_string();
                contact.first_name = parts.get(1).unwrap_or(&"").to_string();
                if contact.name.is_empty() {
                    contact.name = format!("{} {}", contact.first_name, contact.last_name)
                        .trim()
                        .to_string();
                }
            }
            "EMAIL" => {
                if contact.email.is_empty() {
                    contact.email = value;
                }
            }
            "TEL" => {
                if contact.phone.is_empty() {
                    contact.phone = value;
                }
            }
            "ORG" => contact.organization = value.replace(';', " "),
            "TITLE" => contact.title = value,
            "ADR" => contact.address = value.replace(';', " ").trim().to_string(),
            "NOTE" => contact.note = value,
            "URL" => contact.url = value,
            _ => {}
        }
    }
    Ok(contacts)
}

fn decode_vcard_value(name: &str, value: &str) -> Result<String> {
    if name.to_ascii_uppercase().contains("QUOTED-PRINTABLE") {
        quoted_printable::decode(value, quoted_printable::ParseMode::Robust)
            .map(|bytes| decode_text(&bytes))
            .map_err(|error| ConvertError::InvalidInput(error.to_string()))
    } else {
        Ok(value
            .replace("\\n", "\n")
            .replace("\\,", ",")
            .replace("\\;", ";"))
    }
}

fn unfold_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            if let Some(last) = lines.last_mut() {
                last.push_str(line.trim_start());
            }
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}

pub fn csv_to_vcf(path: &Path, output: &Path) -> Result<()> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
    let headers = reader
        .headers()
        .map_err(|error| ConvertError::InvalidInput(error.to_string()))?
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut out = String::new();
    for record in reader.records() {
        let record = record.map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
        let get = |name: &str| -> String {
            headers
                .iter()
                .position(|header| header == name)
                .and_then(|index| record.get(index))
                .unwrap_or("")
                .to_string()
        };
        let name = {
            let full = get("name");
            if !full.is_empty() {
                full
            } else {
                format!("{} {}", get("first_name"), get("last_name"))
                    .trim()
                    .to_string()
            }
        };
        out.push_str("BEGIN:VCARD\r\nVERSION:3.0\r\n");
        out.push_str(&format!("FN:{}\r\n", vcf_escape(&name)));
        out.push_str(&format!(
            "N:{};{};;;\r\n",
            vcf_escape(&get("last_name")),
            vcf_escape(&get("first_name"))
        ));
        for (property, key) in [
            ("EMAIL", "email"),
            ("TEL", "phone"),
            ("ORG", "organization"),
            ("TITLE", "title"),
            ("ADR", "address"),
            ("NOTE", "note"),
            ("URL", "url"),
        ] {
            let value = get(key);
            if !value.is_empty() {
                out.push_str(&format!("{property}:{}\r\n", vcf_escape(&value)));
            }
        }
        out.push_str("END:VCARD\r\n");
    }
    std::fs::write(output, out)?;
    Ok(())
}

fn vcf_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

pub fn ics_document(bytes: &[u8]) -> Result<Document> {
    let events = parse_ics(&decode_text(bytes))?;
    let headers = [
        "summary",
        "start",
        "end",
        "location",
        "description",
        "uid",
        "organizer",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect::<Vec<_>>();
    let rows = events
        .iter()
        .map(|event| {
            vec![
                event.summary.clone(),
                event.start.clone(),
                event.end.clone(),
                event.location.clone(),
                event.description.clone(),
                event.uid.clone(),
                event.organizer.clone(),
            ]
        })
        .collect();
    let mut document = Document::new("Calendar");
    document.push(Block::Table { headers, rows });
    Ok(document)
}

pub fn ics_to_csv(bytes: &[u8], path: &Path) -> Result<()> {
    let events = parse_ics(&decode_text(bytes))?;
    let mut writer =
        csv::Writer::from_path(path).map_err(|error| ConvertError::Internal(error.to_string()))?;
    writer
        .write_record([
            "summary",
            "start",
            "end",
            "location",
            "description",
            "uid",
            "organizer",
        ])
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    for event in events {
        writer
            .write_record([
                event.summary,
                event.start,
                event.end,
                event.location,
                event.description,
                event.uid,
                event.organizer,
            ])
            .map_err(|error| ConvertError::Internal(error.to_string()))?;
    }
    writer
        .flush()
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct CalendarEvent {
    summary: String,
    start: String,
    end: String,
    location: String,
    description: String,
    uid: String,
    organizer: String,
}

fn parse_ics(text: &str) -> Result<Vec<CalendarEvent>> {
    let lines = unfold_lines(text);
    let mut events = Vec::new();
    let mut current: Option<CalendarEvent> = None;
    for line in lines {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:VEVENT") {
            current = Some(CalendarEvent::default());
            continue;
        }
        if upper.starts_with("END:VEVENT") {
            if let Some(event) = current.take() {
                events.push(event);
            }
            continue;
        }
        let Some(event) = current.as_mut() else {
            continue;
        };
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let property = name.split(';').next().unwrap_or(name).to_ascii_uppercase();
        match property.as_str() {
            "SUMMARY" => event.summary = value.to_string(),
            "DTSTART" => event.start = value.to_string(),
            "DTEND" => event.end = value.to_string(),
            "LOCATION" => event.location = value.to_string(),
            "DESCRIPTION" => event.description = value.replace("\\n", "\n"),
            "UID" => event.uid = value.to_string(),
            "ORGANIZER" => event.organizer = value.trim_start_matches("mailto:").to_string(),
            _ => {}
        }
    }
    Ok(events)
}

pub fn csv_to_ics(path: &Path, output: &Path) -> Result<()> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
    let headers = reader
        .headers()
        .map_err(|error| ConvertError::InvalidInput(error.to_string()))?
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//DocForge//EN\r\n");
    for record in reader.records() {
        let record = record.map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
        let get = |name: &str| -> String {
            headers
                .iter()
                .position(|header| header == name)
                .and_then(|index| record.get(index))
                .unwrap_or("")
                .to_string()
        };
        let summary = get("summary");
        let start = get("start");
        let end = get("end");
        let uid = if get("uid").is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            get("uid")
        };
        out.push_str("BEGIN:VEVENT\r\n");
        out.push_str(&format!("UID:{uid}\r\n"));
        if !start.is_empty() {
            out.push_str(&format!("DTSTART:{}\r\n", start.replace(['-', ':'], "")));
        }
        if !end.is_empty() {
            out.push_str(&format!("DTEND:{}\r\n", end.replace(['-', ':'], "")));
        }
        out.push_str(&format!("SUMMARY:{}\r\n", vcf_escape(&summary)));
        for (property, key) in [
            ("LOCATION", "location"),
            ("DESCRIPTION", "description"),
            ("ORGANIZER", "organizer"),
        ] {
            let value = get(key);
            if !value.is_empty() {
                out.push_str(&format!("{property}:{}\r\n", vcf_escape(&value)));
            }
        }
        out.push_str("END:VEVENT\r\n");
    }
    out.push_str("END:VCALENDAR\r\n");
    std::fs::write(output, out)?;
    Ok(())
}

// ---------- Images ----------

pub fn convert_image(
    input: &Path,
    output: &Path,
    from: Format,
    to: Format,
    options: &ConversionOptions,
) -> Result<()> {
    if from == Format::Svg {
        return svg_convert(input, output, to, options);
    }
    if to == Format::Pdf {
        return crate::document::pdf::images_to_pdf(output, &[input.to_path_buf()], options);
    }
    let image = image::open(input)
        .map_err(|error| ConvertError::InvalidInput(format!("cannot decode image: {error}")))?;
    let image = if let Some(max_width) = options.image.max_width {
        if image.width() > max_width {
            image.resize(max_width, u32::MAX, image::imageops::FilterType::Lanczos3)
        } else {
            image
        }
    } else {
        image
    };
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
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
            writer.flush()?;
            Ok(())
        }
        Format::Png => {
            image
                .save_with_format(output, image::ImageFormat::Png)
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
            Ok(())
        }
        Format::Tiff => {
            image
                .save_with_format(output, image::ImageFormat::Tiff)
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
            Ok(())
        }
        Format::Webp => {
            image
                .save_with_format(output, image::ImageFormat::WebP)
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
            Ok(())
        }
        Format::Bmp => {
            image
                .save_with_format(output, image::ImageFormat::Bmp)
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
            Ok(())
        }
        other => Err(ConvertError::Unsupported { from, to: other }),
    }
}

fn svg_convert(input: &Path, output: &Path, to: Format, options: &ConversionOptions) -> Result<()> {
    let data = std::fs::read(input)?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .map_err(|error| ConvertError::InvalidInput(format!("invalid SVG: {error}")))?;
    let size = tree.size();
    let scale = options
        .image
        .max_width
        .map(|max| (max as f32 / size.width()).min(1.0))
        .unwrap_or(1.0);
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| ConvertError::InvalidInput("SVG dimensions are too large".into()))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    if to == Format::Pdf {
        let temp = tempfile::Builder::new()
            .suffix(".png")
            .tempfile_in(output.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|error| ConvertError::Internal(error.to_string()))?;
        pixmap
            .save_png(temp.path())
            .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
        return crate::document::pdf::images_to_pdf(output, &[temp.path().to_path_buf()], options);
    }
    let image = image::RgbaImage::from_raw(width, height, pixmap.data().to_vec())
        .ok_or_else(|| ConvertError::Internal("SVG raster buffer is invalid".into()))?;
    match to {
        Format::Png => {
            image
                .save_with_format(output, image::ImageFormat::Png)
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
        }
        Format::Jpg => {
            let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
            let file = std::fs::File::create(output)?;
            let mut writer = std::io::BufWriter::new(file);
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut writer,
                options.image.quality,
            );
            rgb.write_with_encoder(encoder)
                .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
            writer.flush()?;
        }
        other => {
            return Err(ConvertError::Unsupported {
                from: Format::Svg,
                to: other,
            })
        }
    }
    Ok(())
}

pub fn cbz_to_pdf(
    input: &Path,
    output: &Path,
    workdir: &Path,
    options: &ConversionOptions,
) -> Result<()> {
    use std::io::Read;
    let file = std::fs::File::open(input)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| ConvertError::Archive(error.to_string()))?;
    let mut names: Vec<String> = archive
        .file_names()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            [
                ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff", ".gif",
            ]
            .iter()
            .any(|extension| lower.ends_with(extension))
        })
        .map(str::to_string)
        .collect();
    names.sort();
    if names.is_empty() {
        return Err(ConvertError::InvalidInput(
            "comic archive contains no supported images".into(),
        ));
    }
    let frames_dir = workdir.join("frames");
    std::fs::create_dir_all(&frames_dir)?;
    let mut images = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let mut entry = archive
            .by_name(name)
            .map_err(|error| ConvertError::Archive(error.to_string()))?;
        let extension = name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_else(|| "png".to_string());
        let path = frames_dir.join(format!("page-{index:05}.{extension}"));
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        std::fs::write(&path, bytes)?;
        images.push(path);
    }
    crate::document::pdf::images_to_pdf(output, &images, options)
}

pub fn image_dimensions(path: &Path) -> Result<(u32, u32)> {
    let image = image::image_dimensions(path)
        .map_err(|error| ConvertError::InvalidInput(error.to_string()))?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_json_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("in.csv");
        std::fs::write(&input, "name,age\nAda,36\nGrace,85\n").unwrap();
        let workbook = read_delimited(&input, b',', &ConversionOptions::default()).unwrap();
        let json = workbook_to_json(&workbook);
        assert_eq!(json[0]["name"], "Ada");
        assert_eq!(json[0]["age"], 36);
        let back = json_to_workbook(&json, "Sheet1");
        assert_eq!(back.first().unwrap().rows.len(), 2);
    }

    #[test]
    fn xml_round_trip() {
        let json = serde_json::json!({"catalog": {"item": [{"@id": "1", "name": "A"}, {"@id": "2", "name": "B"}]}});
        let xml = json_to_xml(&json, "root");
        let parsed = xml_to_json(&xml).unwrap();
        assert_eq!(parsed["root"]["catalog"]["item"][1]["name"], "B");
    }

    #[test]
    fn vcard_to_contact() {
        let contacts = parse_vcf(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ada Lovelace\r\nEMAIL:ada@example.com\r\nTEL:+123\r\nEND:VCARD\r\n",
        )
        .unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].email, "ada@example.com");
    }

    #[test]
    fn ics_event_parses() {
        let events = parse_ics(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Demo\r\nDTSTART:20260101T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR",
        )
        .unwrap();
        assert_eq!(events[0].summary, "Demo");
    }
}
