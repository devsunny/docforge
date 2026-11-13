//! Optional external engines.
//!
//! Every engine probes for its tool at startup. Missing tools are still listed
//! by `/api/v1/engines`, but they are excluded from the conversion graph, so the
//! service never routes work to a binary that is not installed.

use crate::document::write::page_dimensions;
use crate::engine::{Engine, EngineContext, Support};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use async_trait::async_trait;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

pub fn detected_engines() -> Vec<Arc<dyn Engine>> {
    vec![
        Arc::new(LibreOfficeEngine::detect()),
        Arc::new(PandocEngine::detect()),
        Arc::new(PopplerEngine::detect()),
        Arc::new(GhostscriptEngine::detect()),
        Arc::new(ImageMagickEngine::detect()),
        Arc::new(CalibreEngine::detect()),
        Arc::new(TesseractEngine::detect()),
        Arc::new(IworkEngine::detect()),
        Arc::new(SevenZipEngine::detect()),
        Arc::new(GxpsEngine::detect()),
        Arc::new(DjvuEngine::detect()),
        Arc::new(AntiwordEngine::detect()),
        Arc::new(FfmpegEngine::detect()),
    ]
}

fn locate(names: &[&str], extra: &[PathBuf]) -> Option<PathBuf> {
    for name in names {
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
    }
    for path in extra {
        if path.is_file() {
            return Some(path.clone());
        }
    }
    None
}

#[cfg(windows)]
fn windows_candidates(relative: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for root in [
        std::env::var_os("PROGRAMFILES"),
        std::env::var_os("PROGRAMFILES(X86)"),
        std::env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    {
        let mut path = PathBuf::from(root);
        for part in relative {
            path.push(part);
        }
        candidates.push(path);
    }
    candidates
}

#[cfg(not(windows))]
fn windows_candidates(_relative: &[&str]) -> Vec<PathBuf> {
    Vec::new()
}

fn locate_lo() -> Option<PathBuf> {
    let mut extra = windows_candidates(&["LibreOffice", "program", "soffice.exe"]);
    extra.extend([
        PathBuf::from("/usr/bin/soffice"),
        PathBuf::from("/usr/lib/libreoffice/program/soffice"),
        PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"),
    ]);
    locate(&["soffice", "libreoffice"], &extra)
}

fn tool_descriptor(id: &'static str, tool: &'static str, bin: Option<PathBuf>) -> Tool {
    Tool { id, tool, bin }
}

#[derive(Debug, Clone)]
struct Tool {
    id: &'static str,
    tool: &'static str,
    bin: Option<PathBuf>,
}

impl Tool {
    fn available(&self) -> bool {
        self.bin.is_some()
    }
    fn program(&self) -> Result<&Path> {
        self.bin
            .as_deref()
            .ok_or_else(|| ConvertError::ToolMissing {
                tool: self.tool.to_string(),
            })
    }
}

async fn run(
    ctx: &EngineContext<'_>,
    program: &Path,
    args: Vec<OsString>,
    engine: &str,
) -> Result<std::process::Output> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let timeout = ctx.timeout.min(Duration::from_secs(900));
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| ConvertError::Timeout {
            seconds: timeout.as_secs(),
        })?
        .map_err(|error| ConvertError::engine(engine, error))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        return Err(ConvertError::engine(
            engine,
            format!(
                "{} exited with {}{}",
                program.display(),
                output.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", truncate(detail, 1200))
                }
            ),
        ));
    }
    Ok(output)
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(limit).collect();
        out.push('…');
        out
    }
}

async fn wait_for_output(dir: &Path, extension: &str, attempts: usize) -> Option<PathBuf> {
    for _ in 0..attempts {
        if let Some(path) = find_output(dir, extension) {
            return Some(path);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    find_output(dir, extension)
}

fn find_output(dir: &Path, extension: &str) -> Option<PathBuf> {
    let mut matches: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
            {
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH);
                Some((modified, path))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by_key(|(modified, _)| *modified);
    matches.pop().map(|(_, path)| path)
}

fn finish_output(found: PathBuf, output: &Path) -> Result<()> {
    if found != output {
        if output.exists() {
            std::fs::remove_file(output)?;
        }
        std::fs::copy(&found, output)?;
    }
    let metadata = std::fs::metadata(output)?;
    if metadata.len() == 0 {
        return Err(ConvertError::MissingOutput);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LibreOffice
// ---------------------------------------------------------------------------

pub struct LibreOfficeEngine {
    tool: Tool,
}

impl LibreOfficeEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("libreoffice", "soffice", locate_lo()),
        }
    }

    fn supports_pair(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tool.available() || from == to {
            return None;
        }
        let document_source = matches!(
            from,
            Format::Doc | Format::Docx | Format::Odt | Format::Rtf | Format::Txt | Format::Html
        );
        let spreadsheet_source = matches!(
            from,
            Format::Xls | Format::Xlsx | Format::Ods | Format::Csv | Format::Tsv
        );
        let presentation_source = matches!(from, Format::Ppt | Format::Pptx | Format::Odp);
        let document_target = matches!(
            to,
            Format::Doc
                | Format::Docx
                | Format::Odt
                | Format::Rtf
                | Format::Pdf
                | Format::Txt
                | Format::Html
        );
        let spreadsheet_target = matches!(
            to,
            Format::Xls
                | Format::Xlsx
                | Format::Ods
                | Format::Csv
                | Format::Pdf
                | Format::Html
                | Format::Txt
        );
        let presentation_target = matches!(
            to,
            Format::Ppt | Format::Pptx | Format::Odp | Format::Pdf | Format::Html | Format::Txt
        );
        if (document_source && document_target)
            || (spreadsheet_source && spreadsheet_target)
            || (presentation_source && presentation_target)
        {
            let layout = to == Format::Pdf;
            let cost = if layout { 6 } else { 8 };
            Some(Support::new(cost, "LibreOffice layout-preserving conversion").layout_if(layout))
        } else {
            None
        }
    }

    fn filter(from: Format, to: Format) -> String {
        match to {
            Format::Pdf => {
                if matches!(from, Format::Xls | Format::Xlsx | Format::Ods | Format::Csv) {
                    "pdf:calc_pdf_Export".to_string()
                } else if matches!(from, Format::Ppt | Format::Pptx | Format::Odp) {
                    "pdf:impress_pdf_Export".to_string()
                } else {
                    "pdf:writer_pdf_Export".to_string()
                }
            }
            Format::Docx => "docx:MS Word 2007 XML".to_string(),
            Format::Doc => "doc:MS Word 97".to_string(),
            Format::Odt => "odt:writer8".to_string(),
            Format::Rtf => "rtf:Rich Text Format".to_string(),
            Format::Pptx => "pptx:Impress MS PowerPoint 2007 XML".to_string(),
            Format::Ppt => "ppt:MS PowerPoint 97".to_string(),
            Format::Odp => "odp:impress8".to_string(),
            Format::Xlsx => "xlsx:Calc MS Excel 2007 XML".to_string(),
            Format::Xls => "xls:MS Excel 97".to_string(),
            Format::Ods => "ods:calc8".to_string(),
            Format::Csv => "csv:Text - txt - csv (StarCalc):44,34,76".to_string(),
            Format::Txt => "txt".to_string(),
            Format::Html => "html".to_string(),
            other => other.extension().to_string(),
        }
    }
}

trait SupportExt {
    fn layout_if(self, layout: bool) -> Self;
}

impl SupportExt for Support {
    fn layout_if(mut self, layout: bool) -> Self {
        if layout {
            self = self.layout();
        }
        self
    }
}

#[async_trait]
impl Engine for LibreOfficeEngine {
    fn id(&self) -> &'static str {
        self.tool.id
    }
    fn name(&self) -> &'static str {
        "LibreOffice"
    }
    fn description(&self) -> &'static str {
        "High-fidelity office document, spreadsheet, and presentation conversion"
    }
    fn priority(&self) -> u32 {
        20
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        self.supports_pair(from, to)
    }

    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let profile = ctx.workdir.join("lo-profile");
        std::fs::create_dir_all(&profile)?;
        let profile_uri = format!(
            "file:///{}",
            profile
                .display()
                .to_string()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        let args: Vec<OsString> = vec![
            "--headless".into(),
            "--norestore".into(),
            "--invisible".into(),
            "--nologo".into(),
            "--nodefault".into(),
            "--nofirststartwizard".into(),
            format!("-env:UserInstallation={profile_uri}").into(),
            "--convert-to".into(),
            Self::filter(ctx.from, ctx.to).into(),
            "--outdir".into(),
            ctx.workdir.as_os_str().to_owned(),
            ctx.input.as_os_str().to_owned(),
        ];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        let extension = ctx.to.extension();
        let expected = ctx
            .input
            .file_stem()
            .map(|stem| {
                ctx.workdir
                    .join(format!("{}.{}", stem.to_string_lossy(), extension))
            })
            .filter(|path| path.exists());
        let found = expected
            .or_else(|| find_output(ctx.workdir, extension))
            .or(wait_for_output(ctx.workdir, extension, 75).await);
        let found = found.ok_or_else(|| {
            let entries = std::fs::read_dir(ctx.workdir)
                .ok()
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            ConvertError::engine(
                self.id(),
                format!(
                    "LibreOffice produced no .{extension} output in {} (entries: {:?})",
                    ctx.workdir.display(),
                    entries
                ),
            )
        })?;
        finish_output(found, ctx.output)
    }
}

// ---------------------------------------------------------------------------
// Pandoc
// ---------------------------------------------------------------------------

pub struct PandocEngine {
    tool: Tool,
}

impl PandocEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("pandoc", "pandoc", locate(&["pandoc"], &[])),
        }
    }

    fn reader(from: Format) -> Option<&'static str> {
        match from {
            Format::Markdown => Some("markdown"),
            Format::Html => Some("html"),
            Format::Docx => Some("docx"),
            Format::Odt => Some("odt"),
            Format::Epub => Some("epub"),
            Format::Tex => Some("latex"),
            Format::Txt => Some("plain"),
            Format::Rtf => Some("rtf"),
            _ => None,
        }
    }

    fn writer(to: Format) -> Option<&'static str> {
        match to {
            Format::Markdown => Some("markdown"),
            Format::Html => Some("html5"),
            Format::Docx => Some("docx"),
            Format::Odt => Some("odt"),
            Format::Epub => Some("epub3"),
            Format::Tex => Some("latex"),
            Format::Txt => Some("plain"),
            Format::Rtf => Some("rtf"),
            Format::Pptx => Some("pptx"),
            _ => None,
        }
    }
}

#[async_trait]
impl Engine for PandocEngine {
    fn id(&self) -> &'static str {
        "pandoc"
    }
    fn name(&self) -> &'static str {
        "Pandoc"
    }
    fn description(&self) -> &'static str {
        "Markup, Office, EPUB, and slide conversion with Pandoc"
    }
    fn priority(&self) -> u32 {
        10
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tool.available() || from == to {
            return None;
        }
        Self::reader(from)?;
        Self::writer(to)?;
        Some(Support::new(7, "Pandoc structural conversion"))
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let reader = Self::reader(ctx.from).ok_or(ConvertError::Unsupported {
            from: ctx.from,
            to: ctx.to,
        })?;
        let writer = Self::writer(ctx.to).ok_or(ConvertError::Unsupported {
            from: ctx.from,
            to: ctx.to,
        })?;
        let mut args: Vec<OsString> = vec![
            "-f".into(),
            reader.into(),
            "-t".into(),
            writer.into(),
            "-o".into(),
            ctx.output.as_os_str().to_owned(),
            ctx.input.as_os_str().to_owned(),
        ];
        if !matches!(ctx.to, Format::Txt | Format::Markdown) {
            args.insert(4, "--standalone".into());
        }
        if let Some(parent) = ctx.input.parent() {
            args.push(format!("--resource-path={}", parent.display()).into());
        }
        run(ctx, self.tool.program()?, args, self.id()).await?;
        let metadata = std::fs::metadata(ctx.output).map_err(|_| ConvertError::MissingOutput)?;
        if metadata.len() == 0 {
            return Err(ConvertError::MissingOutput);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Poppler
// ---------------------------------------------------------------------------

pub struct PopplerEngine {
    pdftotext: Tool,
    pdftoppm: Tool,
    pdftohtml: Tool,
    pdftocairo: Tool,
}

impl PopplerEngine {
    pub fn detect() -> Self {
        Self {
            pdftotext: tool_descriptor("poppler", "pdftotext", locate(&["pdftotext"], &[])),
            pdftoppm: tool_descriptor("poppler", "pdftoppm", locate(&["pdftoppm"], &[])),
            pdftohtml: tool_descriptor("poppler", "pdftohtml", locate(&["pdftohtml"], &[])),
            pdftocairo: tool_descriptor("poppler", "pdftocairo", locate(&["pdftocairo"], &[])),
        }
    }
}

#[async_trait]
impl Engine for PopplerEngine {
    fn id(&self) -> &'static str {
        "poppler"
    }
    fn name(&self) -> &'static str {
        "Poppler PDF tools"
    }
    fn description(&self) -> &'static str {
        "PDF text, HTML, and page rasterization"
    }
    fn priority(&self) -> u32 {
        15
    }
    fn available(&self) -> bool {
        self.pdftotext.available() || self.pdftoppm.available() || self.pdftohtml.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some("pdftotext")
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if from != Format::Pdf {
            return None;
        }
        match to {
            Format::Txt if self.pdftotext.available() => {
                Some(Support::new(9, "Poppler text extraction"))
            }
            Format::Html if self.pdftohtml.available() => {
                Some(Support::new(9, "Poppler HTML export"))
            }
            Format::Svg if self.pdftocairo.available() => Some(Support::new(
                14,
                "Poppler Cairo SVG export (first/selected page)",
            )),
            Format::Png | Format::Jpg | Format::Tiff if self.pdftoppm.available() => Some(
                Support::new(16, "Poppler page rasterization (first/selected page)"),
            ),
            _ => None,
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        match ctx.to {
            Format::Txt => {
                let args: Vec<OsString> = vec![
                    "-layout".into(),
                    "-enc".into(),
                    "UTF-8".into(),
                    ctx.input.as_os_str().to_owned(),
                    ctx.output.as_os_str().to_owned(),
                ];
                run(ctx, self.pdftotext.program()?, args, self.id()).await?;
            }
            Format::Html => {
                let args: Vec<OsString> = vec![
                    "-s".into(),
                    "-noframes".into(),
                    "-enc".into(),
                    "UTF-8".into(),
                    ctx.input.as_os_str().to_owned(),
                    ctx.output.as_os_str().to_owned(),
                ];
                run(ctx, self.pdftohtml.program()?, args, self.id()).await?;
                if !ctx.output.exists() {
                    let html = ctx.output.with_extension("html");
                    if html.exists() {
                        std::fs::rename(html, ctx.output)?;
                    }
                }
            }
            Format::Svg => {
                let page = ctx.options.page.unwrap_or(1);
                let args: Vec<OsString> = vec![
                    "-svg".into(),
                    "-f".into(),
                    page.to_string().into(),
                    "-l".into(),
                    page.to_string().into(),
                    ctx.input.as_os_str().to_owned(),
                    ctx.output.as_os_str().to_owned(),
                ];
                run(ctx, self.pdftocairo.program()?, args, self.id()).await?;
            }
            Format::Png | Format::Jpg | Format::Tiff => {
                let flag = match ctx.to {
                    Format::Png => "-png",
                    Format::Jpg => "-jpeg",
                    Format::Tiff => "-tiff",
                    _ => unreachable!(),
                };
                let page = ctx.options.page.unwrap_or(1);
                let prefix = ctx.workdir.join("page");
                let args: Vec<OsString> = vec![
                    "-f".into(),
                    page.to_string().into(),
                    "-l".into(),
                    page.to_string().into(),
                    "-r".into(),
                    ctx.options.pdf.dpi.to_string().into(),
                    flag.into(),
                    ctx.input.as_os_str().to_owned(),
                    prefix.as_os_str().to_owned(),
                ];
                run(ctx, self.pdftoppm.program()?, args, self.id()).await?;
                let expected = find_output(ctx.workdir, ctx.to.extension())
                    .or_else(|| find_output(ctx.workdir, "png"))
                    .ok_or(ConvertError::MissingOutput)?;
                finish_output(expected, ctx.output)?;
            }
            other => {
                return Err(ConvertError::Unsupported {
                    from: Format::Pdf,
                    to: other,
                })
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Ghostscript
// ---------------------------------------------------------------------------

pub struct GhostscriptEngine {
    tool: Tool,
}

impl GhostscriptEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor(
                "ghostscript",
                "gs",
                locate(&["gs", "gswin64c", "gswin32c"], &[]),
            ),
        }
    }

    fn supports_pair(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tool.available() || from == to {
            return None;
        }
        match (from, to) {
            (Format::Ps, Format::Pdf) => {
                Some(Support::new(8, "Ghostscript PostScript distillation"))
            }
            (Format::Pdf, Format::Ps) => Some(Support::new(12, "Ghostscript PostScript output")),
            (Format::Pdf, Format::Png | Format::Jpg | Format::Tiff) => Some(Support::new(
                16,
                "Ghostscript page rasterization (first/selected page)",
            )),
            (Format::Xps, Format::Pdf) => Some(Support::new(12, "Ghostscript XPS conversion")),
            _ => None,
        }
    }
}

#[async_trait]
impl Engine for GhostscriptEngine {
    fn id(&self) -> &'static str {
        "ghostscript"
    }
    fn name(&self) -> &'static str {
        "Ghostscript"
    }
    fn description(&self) -> &'static str {
        "PostScript, PDF, XPS, and raster conversion"
    }
    fn priority(&self) -> u32 {
        25
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        self.supports_pair(from, to)
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let mut args: Vec<OsString> = vec!["-dBATCH".into(), "-dNOPAUSE".into(), "-dSAFER".into()];
        match ctx.to {
            Format::Pdf => {
                args.push("-sDEVICE=pdfwrite".into());
                args.push(format!("-sOutputFile={}", ctx.output.display()).into());
            }
            Format::Ps => {
                args.push("-sDEVICE=ps2write".into());
                args.push(format!("-sOutputFile={}", ctx.output.display()).into());
            }
            Format::Png | Format::Jpg | Format::Tiff => {
                let device = match ctx.to {
                    Format::Png => "png16m",
                    Format::Jpg => "jpeg",
                    Format::Tiff => "tiff24nc",
                    _ => unreachable!(),
                };
                args.push(format!("-sDEVICE={device}").into());
                args.push(format!("-r{}", ctx.options.pdf.dpi).into());
                let page = ctx.options.page.unwrap_or(1);
                args.push(format!("-dFirstPage={page}").into());
                args.push(format!("-dLastPage={page}").into());
                args.push(format!("-sOutputFile={}", ctx.output.display()).into());
            }
            other => {
                return Err(ConvertError::Unsupported {
                    from: ctx.from,
                    to: other,
                })
            }
        }
        args.push(ctx.input.as_os_str().to_owned());
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ImageMagick
// ---------------------------------------------------------------------------

pub struct ImageMagickEngine {
    tool: Tool,
}

impl ImageMagickEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("imagemagick", "magick", locate(&["magick"], &[])),
        }
    }
}

#[async_trait]
impl Engine for ImageMagickEngine {
    fn id(&self) -> &'static str {
        "imagemagick"
    }
    fn name(&self) -> &'static str {
        "ImageMagick"
    }
    fn description(&self) -> &'static str {
        "Image format conversion, HEIC, SVG, and PDF rasterization"
    }
    fn priority(&self) -> u32 {
        20
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tool.available() || from == to {
            return None;
        }
        let image_source = matches!(
            from,
            Format::Jpg
                | Format::Png
                | Format::Tiff
                | Format::Heic
                | Format::Webp
                | Format::Bmp
                | Format::Svg
        );
        let image_target = matches!(
            to,
            Format::Jpg | Format::Png | Format::Tiff | Format::Webp | Format::Bmp | Format::Pdf
        );
        if image_source && image_target {
            return Some(Support::new(
                if from == Format::Heic { 9 } else { 12 },
                "ImageMagick image conversion",
            ));
        }
        if from == Format::Pdf && matches!(to, Format::Jpg | Format::Png | Format::Tiff) {
            return Some(Support::new(
                17,
                "ImageMagick PDF rasterization (first page)",
            ));
        }
        None
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let mut args: Vec<OsString> = Vec::new();
        if ctx.from == Format::Pdf {
            args.push("-density".into());
            args.push(ctx.options.pdf.dpi.to_string().into());
            args.push(format!("{}[0]", ctx.input.display()).into());
        } else {
            args.push(ctx.input.as_os_str().to_owned());
        }
        if matches!(ctx.to, Format::Jpg) {
            args.push("-background".into());
            args.push("white".into());
            args.push("-alpha".into());
            args.push("remove".into());
            args.push("-quality".into());
            args.push(ctx.options.image.quality.to_string().into());
        }
        args.push(ctx.output.as_os_str().to_owned());
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Calibre
// ---------------------------------------------------------------------------

pub struct CalibreEngine {
    tool: Tool,
}

impl CalibreEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("calibre", "ebook-convert", locate(&["ebook-convert"], &[])),
        }
    }
}

#[async_trait]
impl Engine for CalibreEngine {
    fn id(&self) -> &'static str {
        "calibre"
    }
    fn name(&self) -> &'static str {
        "Calibre"
    }
    fn description(&self) -> &'static str {
        "EPUB, MOBI, PDF, and Word e-book conversion"
    }
    fn priority(&self) -> u32 {
        20
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tool.available() || from == to {
            return None;
        }
        if matches!(from, Format::Epub | Format::Mobi)
            && matches!(to, Format::Epub | Format::Mobi | Format::Pdf | Format::Docx)
        {
            Some(Support::new(8, "Calibre e-book conversion"))
        } else if from == Format::Docx && to == Format::Epub {
            Some(Support::new(10, "Calibre DOCX to EPUB"))
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let args: Vec<OsString> = vec![
            ctx.input.as_os_str().to_owned(),
            ctx.output.as_os_str().to_owned(),
            "--quiet".into(),
        ];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tesseract OCR
// ---------------------------------------------------------------------------

pub struct TesseractEngine {
    tesseract: Tool,
    pdftoppm: Tool,
    pdfunite: Tool,
}

impl TesseractEngine {
    pub fn detect() -> Self {
        Self {
            tesseract: tool_descriptor("tesseract", "tesseract", locate(&["tesseract"], &[])),
            pdftoppm: tool_descriptor("tesseract", "pdftoppm", locate(&["pdftoppm"], &[])),
            pdfunite: tool_descriptor("tesseract", "pdfunite", locate(&["pdfunite"], &[])),
        }
    }

    fn language(&self, options: &ConversionOptions) -> String {
        options.ocr_language.clone()
    }
}

#[async_trait]
impl Engine for TesseractEngine {
    fn id(&self) -> &'static str {
        "tesseract"
    }
    fn name(&self) -> &'static str {
        "Tesseract OCR"
    }
    fn description(&self) -> &'static str {
        "OCR for images and scanned PDFs, including searchable PDF output"
    }
    fn priority(&self) -> u32 {
        15
    }
    fn available(&self) -> bool {
        self.tesseract.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tesseract.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tesseract.available() {
            return None;
        }
        match (from, to) {
            (
                Format::Jpg | Format::Png | Format::Tiff | Format::Webp | Format::Bmp,
                Format::Txt | Format::Pdf,
            ) => Some(Support::new(12, "Tesseract OCR").ocr()),
            (Format::Pdf, Format::Txt) if self.pdftoppm.available() => {
                Some(Support::new(18, "Tesseract OCR over rasterized PDF pages").ocr())
            }
            (Format::Pdf, Format::Pdf)
                if self.pdftoppm.available() && self.pdfunite.available() =>
            {
                Some(Support::new(20, "Tesseract searchable PDF").ocr().layout())
            }
            _ => None,
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let language = self.language(ctx.options);
        match ctx.to {
            Format::Txt if ctx.from != Format::Pdf => {
                let args: Vec<OsString> = vec![
                    ctx.input.as_os_str().to_owned(),
                    "stdout".into(),
                    "-l".into(),
                    language.into(),
                ];
                let output = run(ctx, self.tesseract.program()?, args, self.id()).await?;
                std::fs::write(ctx.output, output.stdout)?;
            }
            Format::Pdf if ctx.from != Format::Pdf => {
                let prefix = ctx.workdir.join("ocr-page");
                let args: Vec<OsString> = vec![
                    ctx.input.as_os_str().to_owned(),
                    prefix.as_os_str().to_owned(),
                    "-l".into(),
                    language.into(),
                    "pdf".into(),
                ];
                run(ctx, self.tesseract.program()?, args, self.id()).await?;
                let found = find_output(ctx.workdir, "pdf").ok_or(ConvertError::MissingOutput)?;
                finish_output(found, ctx.output)?;
            }
            Format::Txt | Format::Pdf if ctx.from == Format::Pdf => {
                if !self.pdftoppm.available() {
                    return Err(ConvertError::ToolMissing {
                        tool: "pdftoppm".into(),
                    });
                }
                let raster_prefix = ctx.workdir.join("scan");
                let args: Vec<OsString> = vec![
                    "-r".into(),
                    ctx.options.pdf.dpi.to_string().into(),
                    "-png".into(),
                    ctx.input.as_os_str().to_owned(),
                    raster_prefix.as_os_str().to_owned(),
                ];
                run(ctx, self.pdftoppm.program()?, args, self.id()).await?;
                let mut pages: Vec<PathBuf> = std::fs::read_dir(ctx.workdir)?
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("scan-") && name.ends_with(".png"))
                    })
                    .collect();
                pages.sort();
                if pages.is_empty() {
                    return Err(ConvertError::MissingOutput);
                }
                if ctx.to == Format::Txt {
                    let mut text = String::new();
                    for page in pages {
                        let args: Vec<OsString> = vec![
                            page.as_os_str().to_owned(),
                            "stdout".into(),
                            "-l".into(),
                            language.clone().into(),
                        ];
                        let output = run(ctx, self.tesseract.program()?, args, self.id()).await?;
                        text.push_str(&String::from_utf8_lossy(&output.stdout));
                        text.push_str("\n\n");
                    }
                    std::fs::write(ctx.output, text)?;
                } else {
                    if !self.pdfunite.available() {
                        return Err(ConvertError::ToolMissing {
                            tool: "pdfunite".into(),
                        });
                    }
                    let mut pdfs = Vec::new();
                    for (index, page) in pages.iter().enumerate() {
                        let prefix = ctx.workdir.join(format!("ocr-{index:04}"));
                        let args: Vec<OsString> = vec![
                            page.as_os_str().to_owned(),
                            prefix.as_os_str().to_owned(),
                            "-l".into(),
                            language.clone().into(),
                            "pdf".into(),
                        ];
                        run(ctx, self.tesseract.program()?, args, self.id()).await?;
                        let pdf = prefix.with_extension("pdf");
                        if pdf.exists() {
                            pdfs.push(pdf);
                        }
                    }
                    if pdfs.is_empty() {
                        return Err(ConvertError::MissingOutput);
                    }
                    let mut args: Vec<OsString> = pdfs
                        .iter()
                        .map(|path| path.as_os_str().to_owned())
                        .collect();
                    args.push(ctx.output.as_os_str().to_owned());
                    run(ctx, self.pdfunite.program()?, args, self.id()).await?;
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
}

// ---------------------------------------------------------------------------
// macOS iWork
// ---------------------------------------------------------------------------

pub struct IworkEngine {
    tool: Tool,
}

impl IworkEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor(
                "iwork",
                "osascript",
                if cfg!(target_os = "macos") {
                    locate(&["osascript"], &[])
                } else {
                    None
                },
            ),
        }
    }

    fn app(from: Format) -> Option<&'static str> {
        match from {
            Format::Pages => Some("Pages"),
            Format::Key => Some("Keynote"),
            Format::Numbers => Some("Numbers"),
            _ => None,
        }
    }
}

#[async_trait]
impl Engine for IworkEngine {
    fn id(&self) -> &'static str {
        "iwork"
    }
    fn name(&self) -> &'static str {
        "Apple iWork"
    }
    fn description(&self) -> &'static str {
        "Apple Pages, Keynote, and Numbers export (macOS only)"
    }
    fn priority(&self) -> u32 {
        10
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if !self.tool.available() {
            return None;
        }
        if Self::app(from).is_some()
            && matches!(to, Format::Pdf | Format::Docx | Format::Pptx | Format::Xlsx)
        {
            Some(Support::new(10, "AppleScript iWork export").layout())
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let app = Self::app(ctx.from).ok_or(ConvertError::Unsupported {
            from: ctx.from,
            to: ctx.to,
        })?;
        let export_type = match (ctx.from, ctx.to) {
            (Format::Pages, Format::Docx) => "Microsoft Word",
            (Format::Key, Format::Pptx) => "Microsoft PowerPoint",
            (Format::Numbers, Format::Xlsx) => "Microsoft Excel",
            (_, Format::Pdf) => "PDF",
            _ => "PDF",
        };
        let script = format!(
            "tell application \"{app}\"\nset d to open POSIX file \"{}\"\nexport d to POSIX file \"{}\" as {export_type}\nclose d saving no\nend tell",
            ctx.input.display(),
            ctx.output.display()
        );
        let args: Vec<OsString> = vec!["-e".into(), script.into()];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 7-Zip (CBR) and misc.
// ---------------------------------------------------------------------------

pub struct SevenZipEngine {
    tool: Tool,
}

impl SevenZipEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor(
                "sevenzip",
                "7z",
                locate(
                    &["7z", "7za", "unar"],
                    &windows_candidates(&["7-Zip", "7z.exe"]),
                ),
            ),
        }
    }
}

#[async_trait]
impl Engine for SevenZipEngine {
    fn id(&self) -> &'static str {
        "sevenzip"
    }
    fn name(&self) -> &'static str {
        "7-Zip archive tools"
    }
    fn description(&self) -> &'static str {
        "RAR comic archive extraction"
    }
    fn priority(&self) -> u32 {
        20
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if self.tool.available() && from == Format::Cbr && to == Format::Pdf {
            Some(Support::new(
                20,
                "7-Zip CBR extraction + native PDF assembly",
            ))
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let dir = ctx.workdir.join("cbr");
        std::fs::create_dir_all(&dir)?;
        let args: Vec<OsString> = vec![
            "x".into(),
            "-y".into(),
            format!("-o{}", dir.display()).into(),
            ctx.input.as_os_str().to_owned(),
        ];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        let mut images = Vec::new();
        collect_images(&dir, &mut images)?;
        images.sort();
        crate::document::pdf::images_to_pdf(ctx.output, &images, ctx.options)
    }
}

fn collect_images(dir: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_images(&path, output)?;
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "bmp" | "tif" | "tiff" | "gif"
                )
            })
        {
            output.push(path);
        }
    }
    Ok(())
}

pub struct GxpsEngine {
    tool: Tool,
}

impl GxpsEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("gxps", "gxps", locate(&["gxps"], &[])),
        }
    }
}

#[async_trait]
impl Engine for GxpsEngine {
    fn id(&self) -> &'static str {
        "gxps"
    }
    fn name(&self) -> &'static str {
        "gxps"
    }
    fn description(&self) -> &'static str {
        "Microsoft XPS to PDF conversion"
    }
    fn priority(&self) -> u32 {
        15
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if self.tool.available() && from == Format::Xps && to == Format::Pdf {
            Some(Support::new(10, "gxps XPS-to-PDF conversion"))
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let args: Vec<OsString> = vec![
            "-sDEVICE=pdfwrite".into(),
            "-sOutputFile".into(),
            ctx.output.as_os_str().to_owned(),
            "-dBATCH".into(),
            "-dNOPAUSE".into(),
            ctx.input.as_os_str().to_owned(),
        ];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

pub struct DjvuEngine {
    tool: Tool,
    pdftoppm: Tool,
    cjb2: Tool,
    djvm: Tool,
}

impl DjvuEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("djvu", "ddjvu", locate(&["ddjvu"], &[])),
            pdftoppm: tool_descriptor("djvu", "pdftoppm", locate(&["pdftoppm"], &[])),
            cjb2: tool_descriptor("djvu", "cjb2", locate(&["cjb2"], &[])),
            djvm: tool_descriptor("djvu", "djvm", locate(&["djvm"], &[])),
        }
    }
}

#[async_trait]
impl Engine for DjvuEngine {
    fn id(&self) -> &'static str {
        "djvu"
    }
    fn name(&self) -> &'static str {
        "DjVuLibre"
    }
    fn description(&self) -> &'static str {
        "DjVu to PDF/TIFF conversion"
    }
    fn priority(&self) -> u32 {
        15
    }
    fn available(&self) -> bool {
        self.tool.available()
            || (self.pdftoppm.available() && self.cjb2.available() && self.djvm.available())
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if self.tool.available() && from == Format::Djvu && matches!(to, Format::Pdf | Format::Tiff)
        {
            Some(Support::new(12, "DjVuLibre conversion"))
        } else if from == Format::Pdf
            && to == Format::Djvu
            && self.pdftoppm.available()
            && self.cjb2.available()
            && self.djvm.available()
        {
            Some(Support::new(30, "PDF rasterization + DjVu compression"))
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        if ctx.from == Format::Pdf && ctx.to == Format::Djvu {
            let prefix = ctx.workdir.join("djvu-page");
            let args: Vec<OsString> = vec![
                "-mono".into(),
                "-r".into(),
                ctx.options.pdf.dpi.to_string().into(),
                ctx.input.as_os_str().to_owned(),
                prefix.as_os_str().to_owned(),
            ];
            run(ctx, self.pdftoppm.program()?, args, self.id()).await?;
            let mut pages: Vec<PathBuf> = std::fs::read_dir(ctx.workdir)?
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with("djvu-page-") && name.ends_with(".pbm")
                        })
                })
                .collect();
            pages.sort();
            if pages.is_empty() {
                return Err(ConvertError::MissingOutput);
            }
            let mut djvus = Vec::new();
            for (index, page) in pages.iter().enumerate() {
                let output = ctx.workdir.join(format!("djvu-{index:04}.djvu"));
                let args: Vec<OsString> =
                    vec![page.as_os_str().to_owned(), output.as_os_str().to_owned()];
                run(ctx, self.cjb2.program()?, args, self.id()).await?;
                if output.exists() {
                    djvus.push(output);
                }
            }
            if djvus.is_empty() {
                return Err(ConvertError::MissingOutput);
            }
            let mut args: Vec<OsString> = vec!["-c".into(), ctx.output.as_os_str().to_owned()];
            args.extend(djvus.iter().map(|path| path.as_os_str().to_owned()));
            run(ctx, self.djvm.program()?, args, self.id()).await?;
            return Ok(());
        }
        let format = if ctx.to == Format::Pdf { "pdf" } else { "tiff" };
        let args: Vec<OsString> = vec![
            format!("-format={format}").into(),
            ctx.input.as_os_str().to_owned(),
            ctx.output.as_os_str().to_owned(),
        ];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

pub struct AntiwordEngine {
    tool: Tool,
}

impl AntiwordEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("antiword", "antiword", locate(&["antiword"], &[])),
        }
    }
}

#[async_trait]
impl Engine for AntiwordEngine {
    fn id(&self) -> &'static str {
        "antiword"
    }
    fn name(&self) -> &'static str {
        "antiword"
    }
    fn description(&self) -> &'static str {
        "Legacy Word 97-2003 text extraction"
    }
    fn priority(&self) -> u32 {
        30
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if self.tool.available() && from == Format::Doc && to == Format::Txt {
            Some(Support::new(12, "antiword legacy DOC text extraction"))
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let args: Vec<OsString> = vec![ctx.input.as_os_str().to_owned(), "-w".into(), "0".into()];
        let output = run(ctx, self.tool.program()?, args, self.id()).await?;
        std::fs::write(ctx.output, output.stdout)?;
        Ok(())
    }
}

pub struct FfmpegEngine {
    tool: Tool,
}

impl FfmpegEngine {
    pub fn detect() -> Self {
        Self {
            tool: tool_descriptor("ffmpeg", "ffmpeg", locate(&["ffmpeg"], &[])),
        }
    }
}

#[async_trait]
impl Engine for FfmpegEngine {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }
    fn name(&self) -> &'static str {
        "FFmpeg"
    }
    fn description(&self) -> &'static str {
        "HEIC and image decoding via FFmpeg"
    }
    fn priority(&self) -> u32 {
        30
    }
    fn available(&self) -> bool {
        self.tool.available()
    }
    fn tool(&self) -> Option<&'static str> {
        Some(self.tool.tool)
    }
    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        if self.tool.available() && from == Format::Heic && matches!(to, Format::Jpg | Format::Png)
        {
            Some(Support::new(14, "FFmpeg HEIC decode"))
        } else {
            None
        }
    }
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let args: Vec<OsString> = vec![
            "-y".into(),
            "-i".into(),
            ctx.input.as_os_str().to_owned(),
            "-frames:v".into(),
            "1".into(),
            ctx.output.as_os_str().to_owned(),
        ];
        run(ctx, self.tool.program()?, args, self.id()).await?;
        Ok(())
    }
}

/// Page geometry helper shared by engines that need it.
pub fn pdf_page_geometry(options: &ConversionOptions) -> (f32, f32) {
    page_dimensions(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tools_are_not_available() {
        // Construct a probe with a definitely invalid path.
        let engine = PandocEngine {
            tool: Tool {
                id: "pandoc",
                tool: "pandoc",
                bin: None,
            },
        };
        assert!(!engine.available());
        assert!(engine.supports(Format::Markdown, Format::Html).is_none());
    }

    #[test]
    fn libreoffice_filter_is_stable() {
        assert_eq!(
            LibreOfficeEngine::filter(Format::Docx, Format::Pdf),
            "pdf:writer_pdf_Export"
        );
    }
}
