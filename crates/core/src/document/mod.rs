//! A small, engine-neutral document model.
//!
//! Every text-bearing converter reads into this model and renders from it. It
//! intentionally models block structure rather than pixel-perfect layout: the
//! native engines are dependency-light fallbacks, while LibreOffice, Pandoc,
//! and the other optional tools provide high-fidelity routes.

pub mod pdf;
pub mod pptx;
pub mod read;
pub mod write;

use crate::error::{ConvertError, Result};
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Document {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    List {
        ordered: bool,
        items: Vec<String>,
    },
    Quote(String),
    Code {
        language: Option<String>,
        text: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    PageBreak,
}

impl Document {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            blocks: Vec::new(),
        }
    }

    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn word_count(&self) -> usize {
        self.plain_text()
            .split_whitespace()
            .filter(|word| !word.is_empty())
            .count()
    }

    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Heading { text, .. } => {
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                Block::Paragraph(text) => {
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                Block::List { ordered, items } => {
                    for (index, item) in items.iter().enumerate() {
                        if *ordered {
                            out.push_str(&format!("{}. ", index + 1));
                        } else {
                            out.push_str("- ");
                        }
                        out.push_str(item);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::Quote(text) => {
                    for line in text.lines() {
                        out.push_str("> ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::Code { text, .. } => {
                    out.push_str(text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::Table { headers, rows } => {
                    if !headers.is_empty() {
                        out.push_str(&headers.join("\t"));
                        out.push('\n');
                    }
                    for row in rows {
                        out.push_str(&row.join("\t"));
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::PageBreak => out.push('\u{000c}'),
            }
        }
        out.trim_end().to_string()
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Heading { level, text } => {
                    let hashes = "#".repeat((*level).clamp(1, 6) as usize);
                    out.push_str(&format!("{hashes} {text}\n\n"));
                }
                Block::Paragraph(text) => {
                    out.push_str(text.trim());
                    out.push_str("\n\n");
                }
                Block::List { ordered, items } => {
                    for (index, item) in items.iter().enumerate() {
                        if *ordered {
                            out.push_str(&format!("{}. {}\n", index + 1, item.trim()));
                        } else {
                            out.push_str(&format!("- {}\n", item.trim()));
                        }
                    }
                    out.push('\n');
                }
                Block::Quote(text) => {
                    for line in text.lines() {
                        out.push_str("> ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::Code { language, text } => {
                    out.push_str("```");
                    if let Some(language) = language {
                        out.push_str(language);
                    }
                    out.push('\n');
                    out.push_str(text.trim_end());
                    out.push_str("\n```\n\n");
                }
                Block::Table { headers, rows } => {
                    if headers.is_empty() {
                        continue;
                    }
                    out.push('|');
                    for header in headers {
                        out.push(' ');
                        out.push_str(&escape_markdown_cell(header));
                        out.push_str(" |");
                    }
                    out.push('\n');
                    out.push('|');
                    for _ in headers {
                        out.push_str(" --- |");
                    }
                    out.push('\n');
                    for row in rows {
                        out.push('|');
                        for index in 0..headers.len() {
                            out.push(' ');
                            out.push_str(&escape_markdown_cell(
                                row.get(index).map(String::as_str).unwrap_or(""),
                            ));
                            out.push_str(" |");
                        }
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::PageBreak => out.push_str("\n---\n\n"),
            }
        }
        out.trim_end().to_string()
    }

    pub fn to_html_fragment(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            match block {
                Block::Heading { level, text } => {
                    let level = (*level).clamp(1, 6);
                    out.push_str(&format!("<h{level}>{}</h{level}>\n", escape_html(text)));
                }
                Block::Paragraph(text) => {
                    out.push_str(&format!("<p>{}</p>\n", escape_html(text)));
                }
                Block::List { ordered, items } => {
                    let tag = if *ordered { "ol" } else { "ul" };
                    out.push_str(&format!("<{tag}>\n"));
                    for item in items {
                        out.push_str(&format!("<li>{}</li>\n", escape_html(item)));
                    }
                    out.push_str(&format!("</{tag}>\n"));
                }
                Block::Quote(text) => {
                    out.push_str(&format!(
                        "<blockquote><p>{}</p></blockquote>\n",
                        escape_html(text)
                    ));
                }
                Block::Code { language, text } => {
                    let class = language
                        .as_ref()
                        .map(|language| format!(" class=\"language-{}\"", escape_html(language)))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "<pre><code{class}>{}</code></pre>\n",
                        escape_html(text)
                    ));
                }
                Block::Table { headers, rows } => {
                    out.push_str("<table>\n");
                    if !headers.is_empty() {
                        out.push_str("<thead><tr>");
                        for header in headers {
                            out.push_str(&format!("<th>{}</th>", escape_html(header)));
                        }
                        out.push_str("</tr></thead>\n");
                    }
                    out.push_str("<tbody>\n");
                    for row in rows {
                        out.push_str("<tr>");
                        for cell in row {
                            out.push_str(&format!("<td>{}</td>", escape_html(cell)));
                        }
                        out.push_str("</tr>\n");
                    }
                    out.push_str("</tbody>\n</table>\n");
                }
                Block::PageBreak => out.push_str("<div class=\"page-break\"></div>\n"),
            }
        }
        out
    }

    pub fn to_html_document(&self) -> String {
        let title = self.title.as_deref().unwrap_or("Converted document");
        format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n<style>\nbody {{ max-width: 52rem; margin: 3rem auto; padding: 0 1.25rem; font: 16px/1.65 system-ui, -apple-system, Segoe UI, sans-serif; color: #1d232a; }}\npre {{ background: #f4f6f8; padding: 1rem; overflow: auto; border-radius: 6px; }}\ncode {{ font-family: ui-monospace, SFMono-Regular, Consolas, monospace; }}\ntable {{ border-collapse: collapse; width: 100%; margin: 1rem 0; }}\nth, td {{ border: 1px solid #d7dde3; padding: .45rem .6rem; text-align: left; }}\nth {{ background: #f4f6f8; }}\nblockquote {{ margin-left: 0; padding-left: 1rem; border-left: 3px solid #c8d0d8; color: #52606d; }}\n.page-break {{ page-break-after: always; }}\n</style>\n</head>\n<body>\n<h1>{title}</h1>\n{body}</body>\n</html>\n",
            title = escape_html(title),
            body = self.to_html_fragment()
        )
    }

    pub fn to_latex(&self) -> String {
        let mut out = String::from(
            "\\documentclass[11pt]{article}\n\\usepackage[utf8]{inputenc}\n\\usepackage{hyperref}\n\\begin{document}\n",
        );
        for block in &self.blocks {
            match block {
                Block::Heading { level, text } => {
                    let command = match level {
                        1 => "section",
                        2 => "subsection",
                        3 => "subsubsection",
                        _ => "paragraph",
                    };
                    out.push_str(&format!("\\{command}{{{}}}\n", escape_latex(text)));
                }
                Block::Paragraph(text) => {
                    out.push_str(&escape_latex(text));
                    out.push_str("\n\n");
                }
                Block::List { ordered, items } => {
                    let environment = if *ordered { "enumerate" } else { "itemize" };
                    out.push_str(&format!("\\begin{{{environment}}}\n"));
                    for item in items {
                        out.push_str(&format!("\\item {}\n", escape_latex(item)));
                    }
                    out.push_str(&format!("\\end{{{environment}}}\n"));
                }
                Block::Quote(text) => {
                    out.push_str(&format!(
                        "\\begin{{quote}}\n{}\n\\end{{quote}}\n",
                        escape_latex(text)
                    ));
                }
                Block::Code { text, .. } => {
                    out.push_str("\\begin{verbatim}\n");
                    out.push_str(text);
                    out.push_str("\n\\end{verbatim}\n");
                }
                Block::Table { headers, rows } => {
                    let columns = headers.len().max(1);
                    out.push_str(&format!("\\begin{{tabular}}{{{}}}\n", "l".repeat(columns)));
                    if !headers.is_empty() {
                        out.push_str(
                            &headers
                                .iter()
                                .map(|cell| escape_latex(cell))
                                .collect::<Vec<_>>()
                                .join(" & "),
                        );
                        out.push_str(" \\\\\n\\hline\n");
                    }
                    for row in rows {
                        out.push_str(
                            &row.iter()
                                .map(|cell| escape_latex(cell))
                                .collect::<Vec<_>>()
                                .join(" & "),
                        );
                        out.push_str(" \\\\\n");
                    }
                    out.push_str("\\end{tabular}\n");
                }
                Block::PageBreak => out.push_str("\\newpage\n"),
            }
        }
        out.push_str("\\end{document}\n");
        out
    }

    /// Basic paragraph extraction. Blank lines separate paragraphs; single
    /// newlines are retained inside a paragraph.
    pub fn from_plain_text(text: &str) -> Self {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut blocks = Vec::new();
        for part in normalized.split("\n\n") {
            let trimmed = part.trim_matches('\n').trim_end();
            if trimmed.trim().is_empty() {
                continue;
            }
            blocks.push(Block::Paragraph(trimmed.to_string()));
        }
        if blocks.is_empty() && !normalized.trim().is_empty() {
            blocks.push(Block::Paragraph(normalized.trim().to_string()));
        }
        Self {
            title: None,
            blocks,
        }
    }

    /// Small line-oriented Markdown reader. It deliberately handles the common
    /// authoring subset; Pandoc is preferred when installed for exotic input.
    pub fn from_markdown(text: &str) -> Self {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = normalized.lines().collect();
        let mut blocks = Vec::new();
        let mut index = 0usize;
        let mut title = None;

        while index < lines.len() {
            let line = lines[index];
            let trimmed = line.trim();
            if trimmed.is_empty() {
                index += 1;
                continue;
            }

            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                let fence = if trimmed.starts_with("```") {
                    "```"
                } else {
                    "~~~"
                };
                let language = trimmed.trim_start_matches(fence).trim();
                let language = if language.is_empty() {
                    None
                } else {
                    Some(language.to_string())
                };
                index += 1;
                let mut code = Vec::new();
                while index < lines.len() && !lines[index].trim().starts_with(fence) {
                    code.push(lines[index]);
                    index += 1;
                }
                index += 1;
                blocks.push(Block::Code {
                    language,
                    text: code.join("\n"),
                });
                continue;
            }

            if let Some((level, heading)) = markdown_heading(trimmed) {
                if level == 1 && title.is_none() {
                    title = Some(heading.clone());
                }
                blocks.push(Block::Heading {
                    level,
                    text: heading,
                });
                index += 1;
                continue;
            }

            if trimmed.starts_with('>') {
                let mut quote = Vec::new();
                while index < lines.len() && lines[index].trim().starts_with('>') {
                    quote.push(lines[index].trim().trim_start_matches('>').trim());
                    index += 1;
                }
                blocks.push(Block::Quote(quote.join(" ")));
                continue;
            }

            if let Some(item) = markdown_list_item(trimmed) {
                let ordered = item.0;
                let mut items = vec![item.1];
                index += 1;
                while index < lines.len() {
                    let next = lines[index].trim();
                    if next.is_empty() {
                        break;
                    }
                    if let Some(item) = markdown_list_item(next) {
                        if item.0 == ordered {
                            items.push(item.1);
                            index += 1;
                            continue;
                        }
                    }
                    if lines[index].starts_with("  ") || lines[index].starts_with('\t') {
                        if let Some(last) = items.last_mut() {
                            last.push(' ');
                            last.push_str(next);
                        }
                        index += 1;
                        continue;
                    }
                    break;
                }
                blocks.push(Block::List { ordered, items });
                continue;
            }

            if trimmed.starts_with('|') && index + 1 < lines.len() {
                if let Some(table) = parse_pipe_table(&lines[index..]) {
                    blocks.push(Block::Table {
                        headers: table.0,
                        rows: table.1,
                    });
                    index += table.2;
                    continue;
                }
            }

            let mut paragraph = vec![trimmed.to_string()];
            index += 1;
            while index < lines.len() {
                let next = lines[index].trim();
                if next.is_empty()
                    || next.starts_with('#')
                    || next.starts_with('>')
                    || next.starts_with("```")
                    || next.starts_with("~~~")
                    || markdown_list_item(next).is_some()
                    || (next.starts_with('|') && lines.get(index + 1).is_some())
                {
                    break;
                }
                paragraph.push(next.to_string());
                index += 1;
            }
            blocks.push(Block::Paragraph(paragraph.join(" ")));
        }

        Self { title, blocks }
    }
}

fn markdown_heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = line[hashes..].trim();
    if rest.is_empty() {
        return None;
    }
    Some((hashes as u8, rest.trim_end_matches('#').trim().to_string()))
}

fn markdown_list_item(line: &str) -> Option<(bool, String)> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some((false, rest.trim().to_string()));
    }
    let dot = line.find(". ")?;
    if dot == 0 || !line[..dot].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((true, line[dot + 2..].trim().to_string()))
}

fn parse_pipe_table(lines: &[&str]) -> Option<(Vec<String>, Vec<Vec<String>>, usize)> {
    let header_line = lines.first()?;
    if !header_line.trim().starts_with('|') {
        return None;
    }
    let headers = split_pipe_row(header_line);
    if headers.is_empty() || lines.len() < 2 {
        return None;
    }
    let separator = split_pipe_row(lines[1]);
    if separator.is_empty()
        || !separator.iter().all(|cell| {
            let cell = cell.trim();
            cell.starts_with('-') && cell.chars().all(|c| c == '-' || c == ':')
        })
    {
        return None;
    }
    let mut rows = Vec::new();
    let mut consumed = 2;
    for line in lines.iter().skip(2) {
        if !line.trim().starts_with('|') {
            break;
        }
        let mut row = split_pipe_row(line);
        row.resize(headers.len(), String::new());
        rows.push(row);
        consumed += 1;
    }
    Some((headers, rows, consumed))
}

fn split_pipe_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    trimmed
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (text, _, _) = encoding_rs::UTF_16LE.decode(&bytes[2..]);
        return text.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (text, _, _) = encoding_rs::UTF_16BE.decode(&bytes[2..]);
        return text.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            text.into_owned()
        }
    }
}

pub fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_markdown_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

pub fn escape_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '$' => out.push_str("\\$"),
            '&' => out.push_str("\\&"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '%' => out.push_str("\\%"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            _ => out.push(ch),
        }
    }
    out
}

/// Convert an HTML document into the block model using a browser-grade parser.
pub fn document_from_html(html: &str) -> Document {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .map(|element| normalize_whitespace(&element.text().collect::<String>()))
        .filter(|title| !title.is_empty());

    let selector = Selector::parse("h1, h2, h3, h4, h5, h6, p, li, pre, blockquote, table").ok();
    let mut blocks = Vec::new();
    if let Some(selector) = selector {
        for element in document.select(&selector) {
            let tag = element.value().name();
            match tag {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = tag[1..].parse::<u8>().unwrap_or(1);
                    let text = normalize_whitespace(&element.text().collect::<String>());
                    if !text.is_empty() {
                        blocks.push(Block::Heading { level, text });
                    }
                }
                "p" => {
                    // Paragraphs nested in list items or blockquotes are
                    // represented by the parent element.
                    let in_list = element
                        .ancestors()
                        .any(|node| node.value().as_element().is_some_and(|e| e.name() == "li"));
                    let in_quote = element.ancestors().any(|node| {
                        node.value()
                            .as_element()
                            .is_some_and(|e| e.name() == "blockquote")
                    });
                    let in_table = element
                        .ancestors()
                        .any(|node| node.value().as_element().is_some_and(|e| e.name() == "td"));
                    if in_list || in_quote || in_table {
                        continue;
                    }
                    let text = normalize_whitespace(&element.text().collect::<String>());
                    if !text.is_empty() {
                        blocks.push(Block::Paragraph(text));
                    }
                }
                "li" => {
                    let text = normalize_whitespace(&element.text().collect::<String>());
                    if text.is_empty() {
                        continue;
                    }
                    if let Some(Block::List { items, .. }) = blocks.last_mut() {
                        items.push(text);
                    } else {
                        blocks.push(Block::List {
                            ordered: element.ancestors().any(|node| {
                                node.value().as_element().is_some_and(|e| e.name() == "ol")
                            }),
                            items: vec![text],
                        });
                    }
                }
                "pre" => {
                    let text = element.text().collect::<String>();
                    if !text.trim().is_empty() {
                        blocks.push(Block::Code {
                            language: None,
                            text: text.trim_end().to_string(),
                        });
                    }
                }
                "blockquote" => {
                    let text = normalize_whitespace(&element.text().collect::<String>());
                    if !text.is_empty() {
                        blocks.push(Block::Quote(text));
                    }
                }
                "table" => {
                    let row_selector = Selector::parse("tr").ok();
                    let cell_selector = Selector::parse("th, td").ok();
                    let (Some(row_selector), Some(cell_selector)) = (row_selector, cell_selector)
                    else {
                        continue;
                    };
                    let mut rows: Vec<Vec<String>> = Vec::new();
                    for row in element.select(&row_selector) {
                        let cells: Vec<String> = row
                            .select(&cell_selector)
                            .map(|cell| normalize_whitespace(&cell.text().collect::<String>()))
                            .collect();
                        if !cells.is_empty() {
                            rows.push(cells);
                        }
                    }
                    if rows.is_empty() {
                        continue;
                    }
                    let headers = rows.remove(0);
                    blocks.push(Block::Table { headers, rows });
                }
                _ => {}
            }
        }
    }

    Document { title, blocks }
}

pub fn parse_json_value(bytes: &[u8]) -> Result<serde_json::Value> {
    serde_json::from_slice(bytes)
        .map_err(|error| ConvertError::Serialization(format!("invalid JSON input: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_round_trip_has_structure() {
        let document = Document::from_markdown(
            "# Title\n\nHello **world**.\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n",
        );
        assert!(matches!(
            document.blocks[0],
            Block::Heading { level: 1, .. }
        ));
        assert!(document.plain_text().contains("Hello"));
        assert!(document.to_markdown().contains("```rust"));
    }

    #[test]
    fn html_reader_finds_headings_and_tables() {
        let document = document_from_html(
            "<html><head><title>Report</title></head><body><h1>Intro</h1><p>Hello</p><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table></body></html>",
        );
        assert_eq!(document.title.as_deref(), Some("Report"));
        assert!(document
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Table { headers, .. } if headers.len() == 2)));
    }

    #[test]
    fn text_decoder_handles_bom_and_cp1252() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhello"), "hello");
        assert_eq!(decode_text(b"caf\xE9"), "café");
    }
}
