//! Conversion orchestration: detect, plan, execute, verify.

use crate::catalog::Catalog;
use crate::detect::{detect_file, DetectedFormat};
use crate::engine::{EngineContext, EngineRegistry};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::graph::{ConversionGraph, ConversionPlan};
use crate::options::ConversionOptions;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::time::timeout;

#[derive(Clone)]
pub struct ConversionService {
    catalog: Arc<Catalog>,
    graph: Arc<ConversionGraph>,
    registry: Arc<EngineRegistry>,
    temp_root: PathBuf,
    max_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversionOutcome {
    pub source_format: Format,
    pub target_format: Format,
    pub output_bytes: u64,
    pub duration_ms: u128,
    pub plan: ConversionPlan,
}

impl ConversionService {
    pub fn new(
        catalog: Arc<Catalog>,
        graph: Arc<ConversionGraph>,
        registry: Arc<EngineRegistry>,
        temp_root: PathBuf,
        max_timeout_secs: u64,
    ) -> Self {
        Self {
            catalog,
            graph,
            registry,
            temp_root,
            max_timeout_secs,
        }
    }

    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    pub fn graph(&self) -> &Arc<ConversionGraph> {
        &self.graph
    }

    pub fn registry(&self) -> &Arc<EngineRegistry> {
        &self.registry
    }

    pub fn detect(
        &self,
        path: &Path,
        filename: Option<&str>,
        mime: Option<&str>,
    ) -> Result<DetectedFormat> {
        detect_file(path, filename, mime)
    }

    pub fn plan(
        &self,
        from: Format,
        to: Format,
        options: &ConversionOptions,
    ) -> Result<ConversionPlan> {
        options.validate().map_err(ConvertError::InvalidInput)?;
        self.graph.plan(from, to, options)
    }

    /// Convert `input` into `output`. The caller owns both paths and is
    /// responsible for their lifecycle.
    pub async fn convert_file(
        &self,
        input: &Path,
        output: &Path,
        from: Format,
        to: Format,
        options: &ConversionOptions,
    ) -> Result<ConversionOutcome> {
        options.validate().map_err(ConvertError::InvalidInput)?;
        if !input.is_file() {
            return Err(ConvertError::InvalidInput(
                "input file does not exist".into(),
            ));
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let started = Instant::now();
        let plan = self.graph.plan(from, to, options)?;

        if plan.identity {
            if input != output {
                std::fs::copy(input, output)?;
            }
            return self.finish(from, to, output, plan, started);
        }

        std::fs::create_dir_all(&self.temp_root)?;
        let workdir = tempfile::Builder::new()
            .prefix("docforge-step-")
            .tempdir_in(&self.temp_root)
            .map_err(|error| ConvertError::Internal(error.to_string()))?;

        let mut current = input.to_path_buf();
        let mut step_paths = Vec::new();
        for (index, step) in plan.steps.iter().enumerate() {
            let is_last = index + 1 == plan.steps.len();
            let step_output = if is_last {
                output.to_path_buf()
            } else {
                let path = workdir
                    .path()
                    .join(format!("step-{index:02}.{}", step.to.extension()));
                step_paths.push(path.clone());
                path
            };
            if step.from != from && index == 0 {
                return Err(ConvertError::Internal(
                    "plan does not start at the requested source format".into(),
                ));
            }
            let duration = options.timeout(120, self.max_timeout_secs);
            let ctx = EngineContext {
                input: &current,
                output: &step_output,
                from: step.from,
                to: step.to,
                options,
                workdir: workdir.path(),
                timeout: duration,
            };
            let engine = self.registry.get(&step.engine).ok_or_else(|| {
                ConvertError::Internal(format!("planned engine '{}' is unavailable", step.engine))
            })?;
            if !engine.available() {
                return Err(ConvertError::ToolMissing {
                    tool: engine.tool().unwrap_or(&step.engine).to_string(),
                });
            }
            timeout(duration, self.registry.run(&step.engine, &ctx))
                .await
                .map_err(|_| ConvertError::Timeout {
                    seconds: duration.as_secs(),
                })??;
            if !step_output.is_file() {
                return Err(ConvertError::Engine {
                    engine: step.engine.clone(),
                    message: "engine reported success but produced no output file".into(),
                });
            }
            current = step_output;
        }

        // The TempDir cleans up intermediates when it drops.
        drop(step_paths);
        self.finish(from, to, output, plan, started)
    }

    fn finish(
        &self,
        from: Format,
        to: Format,
        output: &Path,
        plan: ConversionPlan,
        started: Instant,
    ) -> Result<ConversionOutcome> {
        let metadata = std::fs::metadata(output).map_err(|_| ConvertError::MissingOutput)?;
        if metadata.len() == 0 {
            return Err(ConvertError::MissingOutput);
        }
        Ok(ConversionOutcome {
            source_format: from,
            target_format: to,
            output_bytes: metadata.len(),
            duration_ms: started.elapsed().as_millis(),
            plan,
        })
    }
}

/// Convert a byte buffer, returning bytes and metadata. Used by the synchronous
/// REST endpoint and MCP inline conversion.
pub async fn convert_bytes(
    service: &ConversionService,
    input: &[u8],
    filename: &str,
    source: Option<Format>,
    target: Format,
    options: &ConversionOptions,
) -> Result<(Vec<u8>, ConversionOutcome, Format)> {
    let dir = tempfile::Builder::new()
        .prefix("docforge-inline-")
        .tempdir_in(&service.temp_root)
        .or_else(|_| tempfile::tempdir())
        .map_err(|error| ConvertError::Internal(error.to_string()))?;
    let safe_name = sanitize_filename(filename);
    let provisional = dir
        .path()
        .join(format!("input.{}", source.unwrap_or(target).extension()));
    std::fs::write(&provisional, input)?;
    let detected = detect_file(&provisional, Some(&safe_name), None)?;
    let from = source.unwrap_or(detected.format);
    // Rename with the true extension so external tools that use extensions
    // (LibreOffice in particular) receive the right hint.
    let input_path = dir.path().join(format!("input.{}", from.extension()));
    if input_path != provisional {
        std::fs::rename(&provisional, &input_path)?;
    }
    let output_path = dir.path().join(format!("output.{}", target.extension()));
    let outcome = service
        .convert_file(&input_path, &output_path, from, target, options)
        .await?;
    let bytes = std::fs::read(&output_path)?;
    Ok((bytes, outcome, from))
}

pub fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("document")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ' | '(' | ')') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = base.trim_matches(['.', ' ']).to_string();
    if trimmed.is_empty() {
        "document".to_string()
    } else if trimmed.len() > 180 {
        trimmed.chars().take(180).collect()
    } else {
        trimmed
    }
}

pub fn output_filename(input_name: &str, target: Format) -> String {
    let stem = input_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(input_name);
    format!("{}.{}", sanitize_filename(stem), target.extension())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::engine::EngineRegistry;
    use crate::graph::ConversionGraph;

    fn service() -> ConversionService {
        let catalog = Arc::new(Catalog::load().unwrap());
        let registry = Arc::new(EngineRegistry::native_only());
        let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
        ConversionService::new(catalog, graph, registry, std::env::temp_dir(), 120)
    }

    #[tokio::test]
    async fn converts_text_to_pdf_and_docx() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.txt");
        std::fs::write(&input, "Hello DocForge\n\nSecond paragraph.").unwrap();
        let pdf = temp.path().join("output.pdf");
        let outcome = service()
            .convert_file(
                &input,
                &pdf,
                Format::Txt,
                Format::Pdf,
                &ConversionOptions::default(),
            )
            .await
            .unwrap();
        assert!(outcome.output_bytes > 0);
        assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));

        let docx = temp.path().join("output.docx");
        service()
            .convert_file(
                &input,
                &docx,
                Format::Txt,
                Format::Docx,
                &ConversionOptions::default(),
            )
            .await
            .unwrap();
        assert!(docx.is_file());
    }

    #[test]
    fn sanitizes_paths() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a b.docx"), "a b.docx");
    }
}
