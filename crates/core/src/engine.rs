//! Conversion engine abstraction and registry.
//!
//! An engine advertises pair support up front. The planner builds a weighted
//! graph from those advertisements, so adding an engine automatically makes
//! its conversions routable (including as an intermediate hop).

use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use async_trait::async_trait;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct Support {
    /// Lower is better. A direct native conversion is usually 10; a lossy
    /// fallback bridge is 80+.
    pub cost: u32,
    /// This route performs OCR and should only be used when OCR is requested.
    pub ocr: bool,
    /// This route tries to preserve visual layout.
    pub layout: bool,
    /// Human-readable explanation shown by `/conversions/plan`.
    pub note: String,
}

impl Support {
    pub fn new(cost: u32, note: impl Into<String>) -> Self {
        Self {
            cost,
            ocr: false,
            layout: false,
            note: note.into(),
        }
    }

    pub fn ocr(mut self) -> Self {
        self.ocr = true;
        self
    }

    pub fn layout(mut self) -> Self {
        self.layout = true;
        self
    }
}

pub struct EngineContext<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub from: Format,
    pub to: Format,
    pub options: &'a ConversionOptions,
    /// A private directory the engine may use for intermediates.
    pub workdir: &'a Path,
    pub timeout: Duration,
}

impl EngineContext<'_> {
    pub fn intermediate(&self, name: &str) -> PathBuf {
        self.workdir.join(name)
    }
}

#[async_trait]
pub trait Engine: Send + Sync {
    /// Stable machine ID (`native`, `libreoffice`, `pandoc`, ...).
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// Lower priority wins ties when two engines have the same cost.
    fn priority(&self) -> u32 {
        100
    }
    fn available(&self) -> bool;
    /// Optional external binary this engine depends on.
    fn tool(&self) -> Option<&'static str> {
        None
    }
    /// Return support for a directed pair, or `None`.
    fn supports(&self, from: Format, to: Format) -> Option<Support>;
    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()>;
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub available: bool,
    pub tool: Option<String>,
    pub priority: u32,
}

pub struct EngineRegistry {
    engines: Vec<Arc<dyn Engine>>,
}

impl EngineRegistry {
    pub fn new(engines: Vec<Arc<dyn Engine>>) -> Self {
        let mut engines = engines;
        engines.sort_by_key(|e| (e.priority(), e.id()));
        Self { engines }
    }

    pub fn native_only() -> Self {
        Self::new(vec![
            Arc::new(crate::native::NativeEngine::new()),
            Arc::new(crate::ocr::NativeOcrEngine),
        ])
    }

    /// Build the default registry, probing optional external tools.
    pub fn detect() -> Self {
        let mut engines: Vec<Arc<dyn Engine>> = vec![
            Arc::new(crate::native::NativeEngine::new()),
            Arc::new(crate::ocr::NativeOcrEngine),
        ];
        engines.extend(crate::external::detected_engines());
        Self::new(engines)
    }

    pub fn engines(&self) -> &[Arc<dyn Engine>] {
        &self.engines
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Engine>> {
        self.engines.iter().find(|e| e.id() == id).cloned()
    }

    pub fn descriptors(&self) -> Vec<EngineDescriptor> {
        self.engines
            .iter()
            .map(|engine| EngineDescriptor {
                id: engine.id().to_string(),
                name: engine.name().to_string(),
                description: engine.description().to_string(),
                available: engine.available(),
                tool: engine.tool().map(str::to_string),
                priority: engine.priority(),
            })
            .collect()
    }

    pub fn available_count(&self) -> usize {
        self.engines.iter().filter(|e| e.available()).count()
    }

    /// Best advertised support for a pair, considering only available engines.
    pub fn best_support(&self, from: Format, to: Format) -> Option<BestSupport> {
        let mut best: Option<BestSupport> = None;
        for engine in &self.engines {
            if !engine.available() {
                continue;
            }
            let Some(support) = engine.supports(from, to) else {
                continue;
            };
            let candidate = BestSupport {
                engine: engine.id().to_string(),
                engine_name: engine.name().to_string(),
                priority: engine.priority(),
                support,
            };
            let replace = match &best {
                None => true,
                Some(current) => {
                    candidate.support.cost < current.support.cost
                        || (candidate.support.cost == current.support.cost
                            && candidate.priority < current.priority)
                }
            };
            if replace {
                best = Some(candidate);
            }
        }
        best
    }

    pub async fn run(&self, engine_id: &str, ctx: &EngineContext<'_>) -> Result<()> {
        let engine = self.get(engine_id).ok_or_else(|| {
            ConvertError::Internal(format!("engine '{engine_id}' is not registered"))
        })?;
        if !engine.available() {
            return Err(ConvertError::ToolMissing {
                tool: engine.tool().unwrap_or(engine_id).to_string(),
            });
        }
        engine.convert(ctx).await
    }
}

#[derive(Debug, Clone)]
pub struct BestSupport {
    pub engine: String,
    pub engine_name: String,
    pub priority: u32,
    pub support: Support,
}
