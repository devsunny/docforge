//! Dependency-light native engine.
//!
//! Native conversions are best-effort, structure-preserving fallbacks. They are
//! deliberately cheap to run and work in air-gapped/offline deployments; the
//! optional external engines provide higher-fidelity layout when installed.

use crate::data;
use crate::document::read::read_document;
use crate::document::write::write_document;
use crate::document::{Block, Document};
use crate::engine::{Engine, EngineContext, Support};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use async_trait::async_trait;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Document,
    OfficePdf,
    PdfImage,
    PdfTable,
    Workbook,
    Structured,
    Image,
    SvgImage,
    CbzPdf,
    VcfToCsv,
    CsvToVcf,
    IcsToCsv,
    CsvToIcs,
    Email,
}

#[derive(Default)]
pub struct NativeEngine;

impl NativeEngine {
    pub fn new() -> Self {
        Self
    }

    fn route(from: Format, to: Format) -> Option<(Route, Support)> {
        if from == to {
            return None;
        }

        if from == Format::Cbz && to == Format::Pdf {
            return Some((
                Route::CbzPdf,
                Support::new(22, "native CBZ page rasterization"),
            ));
        }
        if from == Format::Vcf && to == Format::Csv {
            return Some((Route::VcfToCsv, Support::new(8, "native vCard parser")));
        }
        if from == Format::Csv && to == Format::Vcf {
            return Some((Route::CsvToVcf, Support::new(8, "native vCard writer")));
        }
        if from == Format::Ics && to == Format::Csv {
            return Some((Route::IcsToCsv, Support::new(8, "native iCalendar parser")));
        }
        if from == Format::Csv && to == Format::Ics {
            return Some((Route::CsvToIcs, Support::new(8, "native iCalendar writer")));
        }
        if from == Format::Pdf && matches!(to, Format::Csv | Format::Xlsx | Format::Xml) {
            return Some((
                Route::PdfTable,
                Support::new(60, "text/table heuristic; layout is approximate"),
            ));
        }

        // OOXML to PDF through Typst. This outranks LibreOffice (cost 6) on
        // ties, and the native document/workbook route is a fallback if the
        // richer renderer rejects a file.
        if to == Format::Pdf && matches!(from, Format::Docx | Format::Pptx | Format::Xlsx) {
            return Some((
                Route::OfficePdf,
                Support::new(6, "native Office renderer (Typst layout)").layout(),
            ));
        }

        // PDF page rasterization. Poppler/Ghostscript remain optional for
        // unsupported PDF features and vector SVG output.
        if from == Format::Pdf && matches!(to, Format::Jpg | Format::Png | Format::Tiff) {
            return Some((
                Route::PdfImage,
                Support::new(10, "native PDF page rasterizer").layout(),
            ));
        }

        // Spreadsheet/data conversions.
        if is_workbook_source(from) && is_workbook_target(to) {
            let cost = if matches!(to, Format::Pdf) { 18 } else { 10 };
            return Some((Route::Workbook, Support::new(cost, "native workbook model")));
        }
        if is_structured_source(from)
            && matches!(
                to,
                Format::Json
                    | Format::Yaml
                    | Format::Toml
                    | Format::Xml
                    | Format::Csv
                    | Format::Tsv
                    | Format::Xlsx
                    | Format::Ods
                    | Format::Pdf
                    | Format::Docx
                    | Format::Html
                    | Format::Markdown
                    | Format::Txt
            )
            && (matches!(to, Format::Json | Format::Yaml | Format::Toml | Format::Xml)
                || !is_workbook_source(from))
        {
            return Some((
                Route::Structured,
                Support::new(9, "native structured-data conversion"),
            ));
        }

        // Document pipeline.
        if is_document_source(from) && is_document_target(to) {
            let cost = if from == Format::Pdf && to == Format::Pptx {
                38
            } else if matches!(from, Format::Pdf | Format::Pptx | Format::Epub) {
                22
            } else {
                12
            };
            let note = if from == Format::Pdf && to == Format::Pptx {
                "native text-only slide reconstruction"
            } else {
                "native document model (text structure, not pixel layout)"
            };
            return Some((Route::Document, Support::new(cost, note)));
        }

        // Email.
        if from == Format::Eml && is_document_target(to) {
            return Some((Route::Email, Support::new(14, "native RFC 822 parser")));
        }

        // Images.
        if to == Format::Pdf && is_native_image(from) {
            let cost = if from == Format::Svg { 15 } else { 10 };
            return Some((
                Route::Image,
                Support::new(cost, "native image decode + PDF page"),
            ));
        }
        if from == Format::Svg && matches!(to, Format::Png | Format::Jpg) {
            return Some((Route::SvgImage, Support::new(12, "native SVG rasterizer")));
        }
        if is_native_raster(from) && is_native_raster(to) {
            return Some((Route::Image, Support::new(10, "native image re-encode")));
        }

        None
    }
}

fn is_document_source(format: Format) -> bool {
    matches!(
        format,
        Format::Pdf
            | Format::Docx
            | Format::Odt
            | Format::Rtf
            | Format::Epub
            | Format::Pptx
            | Format::Html
            | Format::Markdown
            | Format::Txt
            | Format::Tex
            | Format::Vcf
            | Format::Ics
            | Format::Msg
    )
}

fn is_document_target(format: Format) -> bool {
    matches!(
        format,
        Format::Pdf
            | Format::Docx
            | Format::Odt
            | Format::Rtf
            | Format::Epub
            | Format::Pptx
            | Format::Html
            | Format::Markdown
            | Format::Txt
            | Format::Tex
    )
}

fn is_workbook_source(format: Format) -> bool {
    matches!(
        format,
        Format::Csv
            | Format::Tsv
            | Format::Xlsx
            | Format::Xls
            | Format::Ods
            | Format::Json
            | Format::Xml
    )
}

fn is_workbook_target(format: Format) -> bool {
    matches!(
        format,
        Format::Csv
            | Format::Tsv
            | Format::Xlsx
            | Format::Ods
            | Format::Json
            | Format::Xml
            | Format::Html
            | Format::Markdown
            | Format::Pdf
            | Format::Docx
            | Format::Txt
    )
}

fn is_structured_source(format: Format) -> bool {
    matches!(
        format,
        Format::Json | Format::Yaml | Format::Toml | Format::Xml
    )
}

fn is_native_raster(format: Format) -> bool {
    matches!(
        format,
        Format::Jpg | Format::Png | Format::Tiff | Format::Webp | Format::Bmp
    )
}

fn is_native_image(format: Format) -> bool {
    is_native_raster(format) || format == Format::Svg
}

#[async_trait]
impl Engine for NativeEngine {
    fn id(&self) -> &'static str {
        "native"
    }

    fn name(&self) -> &'static str {
        "DocForge native"
    }

    fn description(&self) -> &'static str {
        "Pure-Rust document, data, image, contact, and comic conversions"
    }

    fn priority(&self) -> u32 {
        0
    }

    fn available(&self) -> bool {
        true
    }

    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        Self::route(from, to).map(|(_, support)| support)
    }

    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let (route, _) = Self::route(ctx.from, ctx.to).ok_or(ConvertError::Unsupported {
            from: ctx.from,
            to: ctx.to,
        })?;
        match route {
            Route::Document | Route::Email => {
                let document = read_document(ctx.input, ctx.from, ctx.options)?;
                write_document(ctx.output, &document, ctx.to, ctx.options)?;
            }
            Route::OfficePdf => {
                if let Err(error) =
                    crate::office::office_to_pdf(ctx.input, ctx.output, ctx.from, ctx.options)
                {
                    tracing::warn!(
                        engine = "office2pdf",
                        from = %ctx.from,
                        error = %error,
                        "native Office renderer failed; falling back to the structural model"
                    );
                    if ctx.from == Format::Xlsx {
                        let workbook = data::read_workbook(ctx.input, ctx.from, ctx.options)?;
                        data::write_workbook(ctx.output, &workbook, ctx.to, ctx.options)?;
                    } else {
                        let document = read_document(ctx.input, ctx.from, ctx.options)?;
                        write_document(ctx.output, &document, ctx.to, ctx.options)?;
                    }
                }
            }
            Route::PdfImage => {
                crate::pdf_raster::pdf_to_image(ctx.input, ctx.output, ctx.to, ctx.options)?;
            }
            Route::PdfTable => {
                let workbook = data::pdf_table_workbook(ctx.input, ctx.options)?;
                data::write_workbook(ctx.output, &workbook, ctx.to, ctx.options)?;
            }
            Route::Workbook => {
                let workbook = data::read_workbook(ctx.input, ctx.from, ctx.options)?;
                data::write_workbook(ctx.output, &workbook, ctx.to, ctx.options)?;
            }
            Route::Structured => {
                structured_convert(ctx)?;
            }
            Route::Image | Route::SvgImage => {
                data::convert_image(ctx.input, ctx.output, ctx.from, ctx.to, ctx.options)?;
            }
            Route::CbzPdf => {
                data::cbz_to_pdf(ctx.input, ctx.output, ctx.workdir, ctx.options)?;
            }
            Route::VcfToCsv => {
                let bytes = std::fs::read(ctx.input)?;
                data::vcf_to_csv(&bytes, ctx.output)?;
            }
            Route::CsvToVcf => {
                data::csv_to_vcf(ctx.input, ctx.output)?;
            }
            Route::IcsToCsv => {
                let bytes = std::fs::read(ctx.input)?;
                data::ics_to_csv(&bytes, ctx.output)?;
            }
            Route::CsvToIcs => {
                data::csv_to_ics(ctx.input, ctx.output)?;
            }
        }
        ensure_output(ctx.output)
    }
}

fn ensure_output(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).map_err(|_| ConvertError::MissingOutput)?;
    if metadata.len() == 0 {
        return Err(ConvertError::MissingOutput);
    }
    Ok(())
}

fn read_value(path: &Path, format: Format) -> Result<serde_json::Value> {
    let bytes = std::fs::read(path)?;
    match format {
        Format::Json => serde_json::from_slice(&bytes)
            .map_err(|error| ConvertError::Serialization(error.to_string())),
        Format::Yaml => serde_yaml::from_slice(&bytes)
            .map_err(|error| ConvertError::Serialization(error.to_string())),
        Format::Toml => {
            let text = crate::document::decode_text(&bytes);
            let value: toml::Value = toml::from_str(&text)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            serde_json::to_value(value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))
        }
        Format::Xml => data::xml_to_json(&crate::document::decode_text(&bytes)),
        other => Err(ConvertError::Unsupported {
            from: other,
            to: Format::Json,
        }),
    }
}

fn structured_convert(ctx: &EngineContext<'_>) -> Result<()> {
    let value = read_value(ctx.input, ctx.from)?;
    match ctx.to {
        Format::Json => {
            std::fs::write(
                ctx.output,
                serde_json::to_vec_pretty(&value)
                    .map_err(|error| ConvertError::Serialization(error.to_string()))?,
            )?;
        }
        Format::Yaml => {
            let text = serde_yaml::to_string(&value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            std::fs::write(ctx.output, text)?;
        }
        Format::Toml => {
            let toml_value: toml::Value = serde_json::from_value(value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            let text = toml::to_string_pretty(&toml_value)
                .map_err(|error| ConvertError::Serialization(error.to_string()))?;
            std::fs::write(ctx.output, text)?;
        }
        Format::Xml => {
            std::fs::write(ctx.output, data::json_to_xml(&value, "root"))?;
        }
        Format::Csv
        | Format::Tsv
        | Format::Xlsx
        | Format::Ods
        | Format::Pdf
        | Format::Docx
        | Format::Html
        | Format::Markdown
        | Format::Txt => {
            let workbook = data::json_to_workbook(&value, "Sheet1");
            if matches!(
                ctx.to,
                Format::Pdf | Format::Docx | Format::Html | Format::Markdown
            ) {
                let document = data::workbook_document_from(&workbook);
                write_document(ctx.output, &document, ctx.to, ctx.options)?;
            } else {
                data::write_workbook(ctx.output, &workbook, ctx.to, ctx.options)?;
            }
        }
        other => {
            return Err(ConvertError::Unsupported {
                from: ctx.from,
                to: other,
            })
        }
    }
    Ok(())
}

/// Helper for tests and the CLI: render a value as a Markdown table.
pub fn value_markdown(value: &serde_json::Value) -> String {
    let workbook = data::json_to_workbook(value, "data");
    let mut document = Document::new("Data");
    for sheet in &workbook.sheets {
        document.push(Block::Table {
            headers: sheet.headers.clone(),
            rows: sheet.rows.clone(),
        });
    }
    document.to_markdown()
}

/// The native engine is also exposed as a deterministic conversion function for
/// library users and tests.
pub async fn convert_native(
    input: &Path,
    output: &Path,
    from: Format,
    to: Format,
    options: &ConversionOptions,
    workdir: &Path,
) -> Result<()> {
    let engine = NativeEngine::new();
    let ctx = EngineContext {
        input,
        output,
        from,
        to,
        options,
        workdir,
        timeout: std::time::Duration::from_secs(120),
    };
    engine.convert(&ctx).await?;
    ensure_output(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_core_routes() {
        for (from, to) in [
            (Format::Txt, Format::Pdf),
            (Format::Docx, Format::Pdf),
            (Format::Pdf, Format::Docx),
            (Format::Pdf, Format::Pptx),
            (Format::Csv, Format::Xlsx),
            (Format::Json, Format::Yaml),
            (Format::Vcf, Format::Csv),
            (Format::Cbz, Format::Pdf),
            (Format::Jpg, Format::Pdf),
            (Format::Svg, Format::Pdf),
        ] {
            assert!(
                NativeEngine::route(from, to).is_some(),
                "missing native route {from:?} -> {to:?}"
            );
        }
    }

    #[test]
    fn does_not_claim_heic() {
        assert!(NativeEngine::route(Format::Heic, Format::Jpg).is_none());
    }
}
