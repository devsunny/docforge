//! Weighted conversion graph and shortest-route planner.

use crate::catalog::{Catalog, CatalogEntry};
use crate::engine::EngineRegistry;
use crate::error::{ConvertError, Result};
use crate::format::Format;
use crate::options::ConversionOptions;
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

const HOP_PENALTY: u32 = 8;

fn is_ocr_source(format: Format) -> bool {
    matches!(
        format,
        Format::Pdf
            | Format::Jpg
            | Format::Png
            | Format::Tiff
            | Format::Heic
            | Format::Webp
            | Format::Bmp
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub from: Format,
    pub to: Format,
    pub engine: String,
    pub engine_name: String,
    pub cost: u32,
    pub ocr: bool,
    pub layout: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStep {
    pub from: Format,
    pub to: Format,
    pub engine: String,
    pub engine_name: String,
    pub cost: u32,
    pub ocr: bool,
    pub layout: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversionPlan {
    pub source: Format,
    pub target: Format,
    pub steps: Vec<PlanStep>,
    pub total_cost: u32,
    pub direct: bool,
    pub identity: bool,
    pub catalog_rank: Option<u16>,
    pub notes: Vec<String>,
}

impl ConversionPlan {
    pub fn engine_ids(&self) -> Vec<String> {
        self.steps.iter().map(|s| s.engine.clone()).collect()
    }

    pub fn summary(&self) -> String {
        if self.identity {
            return "byte-for-byte copy".to_string();
        }
        self.steps
            .iter()
            .map(|step| {
                format!(
                    "{} -> {} ({})",
                    step.from.label(),
                    step.to.label(),
                    step.engine
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogAvailability {
    pub rank: u16,
    pub available: bool,
    pub direct: bool,
    pub route: Option<String>,
    pub total_cost: Option<u32>,
}

pub struct ConversionGraph {
    catalog: Arc<Catalog>,
    edges: Vec<GraphEdge>,
    outgoing: HashMap<Format, Vec<usize>>,
    availability: HashMap<u16, bool>,
    engines: Vec<crate::engine::EngineDescriptor>,
}

impl ConversionGraph {
    pub fn build(catalog: Arc<Catalog>, registry: &EngineRegistry) -> Self {
        let mut edges = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for from in Format::ALL {
            for to in Format::ALL {
                if from == to {
                    continue;
                }
                for engine in registry.engines() {
                    if !engine.available() {
                        continue;
                    }
                    let Some(support) = engine.supports(*from, *to) else {
                        continue;
                    };
                    let key = (
                        *from,
                        *to,
                        engine.id().to_string(),
                        support.ocr,
                        support.layout,
                    );
                    if !seen.insert(key) {
                        continue;
                    }
                    edges.push(GraphEdge {
                        from: *from,
                        to: *to,
                        engine: engine.id().to_string(),
                        engine_name: engine.name().to_string(),
                        cost: support.cost,
                        ocr: support.ocr,
                        layout: support.layout,
                        note: support.note,
                    });
                }
            }
        }

        let mut outgoing: HashMap<Format, Vec<usize>> = HashMap::new();
        for (index, edge) in edges.iter().enumerate() {
            outgoing.entry(edge.from).or_default().push(index);
        }
        for indices in outgoing.values_mut() {
            indices.sort_by_key(|index| edges[*index].cost);
        }

        let mut graph = Self {
            catalog,
            edges,
            outgoing,
            availability: HashMap::new(),
            engines: registry.descriptors(),
        };

        let mut availability = HashMap::new();
        for entry in graph.catalog.entries() {
            let ocr = entry.requires_ocr || entry.searchable;
            let available = graph
                .plan_with_ocr(entry.source_formats[0], entry.target_formats[0], ocr)
                .is_ok()
                || entry
                    .pair_ids()
                    .iter()
                    .any(|(source, target)| graph.plan_with_ocr(*source, *target, ocr).is_ok());
            availability.insert(entry.rank, available);
        }
        graph.availability = availability;
        graph
    }

    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }

    pub fn engines(&self) -> &[crate::engine::EngineDescriptor] {
        &self.engines
    }

    pub fn is_catalog_available(&self, rank: u16) -> bool {
        self.availability.get(&rank).copied().unwrap_or(false)
    }

    /// Direct targets advertised by any available engine. This includes
    /// extended routes that are not among the 100 ranked catalog rows, such as
    /// `MSG -> MD` from the native Outlook parser.
    pub fn direct_targets(&self, from: Format) -> Vec<Format> {
        let mut targets: Vec<Format> = self
            .outgoing
            .get(&from)
            .map(|indices| indices.iter().map(|index| self.edges[*index].to).collect())
            .unwrap_or_default();
        targets.sort();
        targets.dedup();
        targets
    }

    pub fn catalog_availability(&self) -> Vec<CatalogAvailability> {
        self.catalog
            .entries()
            .iter()
            .map(|entry| {
                let available = self.is_catalog_available(entry.rank);
                let plan = self.catalog_plan(entry).ok();
                CatalogAvailability {
                    rank: entry.rank,
                    available,
                    direct: plan.as_ref().map(|p| p.direct).unwrap_or(false),
                    route: plan.as_ref().map(|p| p.summary()),
                    total_cost: plan.map(|p| p.total_cost),
                }
            })
            .collect()
    }

    /// Find the best plan for a catalog row, including alias pairs such as
    /// `EML / MSG` and OCR-qualified rows.
    pub fn catalog_plan(&self, entry: &CatalogEntry) -> Result<ConversionPlan> {
        let ocr = entry.requires_ocr || entry.searchable;
        let mut best: Option<ConversionPlan> = None;
        for (source, target) in entry.pair_ids() {
            match self.plan_with_ocr(source, target, ocr) {
                Ok(plan) => {
                    if best
                        .as_ref()
                        .map(|current| plan.total_cost < current.total_cost)
                        .unwrap_or(true)
                    {
                        best = Some(plan);
                    }
                }
                Err(_) => continue,
            }
        }
        best.ok_or_else(|| ConvertError::NoRoute {
            from: entry.source_formats[0],
            to: entry.target_formats[0],
            detail: format!(
                "catalog rank {} requires an OCR-capable engine or an additional converter",
                entry.rank
            ),
        })
    }

    pub fn plan(
        &self,
        from: Format,
        to: Format,
        options: &ConversionOptions,
    ) -> Result<ConversionPlan> {
        self.plan_with_ocr(from, to, options.ocr)
    }

    fn plan_with_ocr(&self, from: Format, to: Format, ocr: bool) -> Result<ConversionPlan> {
        if from == to && !ocr {
            return Ok(ConversionPlan {
                source: from,
                target: to,
                steps: Vec::new(),
                total_cost: 0,
                direct: true,
                identity: true,
                catalog_rank: self
                    .catalog
                    .best_for(from, to, false)
                    .map(|entry| entry.rank),
                notes: vec!["Source and target format are identical; copying bytes.".to_string()],
            });
        }

        let mut dist: HashMap<Format, u32> = HashMap::new();
        let mut previous: HashMap<Format, (Format, usize)> = HashMap::new();
        let mut heap = BinaryHeap::new();
        dist.insert(from, 0);
        heap.push(Reverse((0u32, from)));

        while let Some(Reverse((cost, current))) = heap.pop() {
            if cost > *dist.get(&current).unwrap_or(&u32::MAX) {
                continue;
            }
            if current == to {
                break;
            }
            let Some(indices) = self.outgoing.get(&current) else {
                continue;
            };
            for index in indices {
                let edge = &self.edges[*index];
                if edge.ocr && !ocr {
                    continue;
                }
                // When OCR is explicitly requested, do not silently fall back
                // to a text-layer extraction from a raster/PDF source.
                if ocr && !edge.ocr && is_ocr_source(edge.from) {
                    continue;
                }
                let mut weight = edge.cost.saturating_add(HOP_PENALTY);
                if edge.layout {
                    weight = weight.saturating_sub(4);
                }
                if ocr && edge.ocr {
                    weight = weight.saturating_sub(3);
                }
                let next = cost.saturating_add(weight);
                if next < *dist.get(&edge.to).unwrap_or(&u32::MAX) {
                    dist.insert(edge.to, next);
                    previous.insert(edge.to, (current, *index));
                    heap.push(Reverse((next, edge.to)));
                }
            }
        }

        if !dist.contains_key(&to) {
            let catalog_hint = self
                .catalog
                .best_for(from, to, ocr)
                .map(|entry| format!(" Catalog rank {} names this pair.", entry.rank))
                .unwrap_or_default();
            let tools: Vec<&str> = self
                .engines
                .iter()
                .filter(|engine| !engine.available)
                .filter_map(|engine| engine.tool.as_deref())
                .collect();
            let tool_hint = if tools.is_empty() {
                String::new()
            } else {
                format!(
                    " Install one of these optional tools and restart: {}.",
                    tools.join(", ")
                )
            };
            return Err(ConvertError::NoRoute {
                from,
                to,
                detail: format!(
                    "no installed engine advertises a usable route from {} to {}.{} {}",
                    from.label(),
                    to.label(),
                    catalog_hint,
                    tool_hint
                ),
            });
        }

        let mut steps_rev = Vec::new();
        let mut cursor = to;
        while cursor != from {
            let Some((prev, edge_index)) = previous.get(&cursor).copied() else {
                return Err(ConvertError::Internal(
                    "planner previous-node chain is incomplete".into(),
                ));
            };
            let edge = &self.edges[edge_index];
            steps_rev.push(PlanStep {
                from: edge.from,
                to: edge.to,
                engine: edge.engine.clone(),
                engine_name: edge.engine_name.clone(),
                cost: edge.cost,
                ocr: edge.ocr,
                layout: edge.layout,
                note: edge.note.clone(),
            });
            cursor = prev;
        }
        steps_rev.reverse();

        let total_cost = dist.get(&to).copied().unwrap_or(0);
        let direct = steps_rev.len() == 1;
        let catalog_rank = if direct {
            self.catalog
                .find(from, to)
                .into_iter()
                .find(|entry| {
                    if ocr {
                        entry.requires_ocr || entry.searchable
                    } else {
                        !entry.requires_ocr && !entry.searchable
                    }
                })
                .map(|entry| entry.rank)
        } else {
            None
        };

        let mut notes = Vec::new();
        if !direct {
            notes.push(format!(
                "No direct engine route was selected; using {} hops.",
                steps_rev.len()
            ));
        }
        if steps_rev.iter().any(|step| step.ocr) {
            notes.push(
                "At least one step uses OCR; accuracy depends on scan quality and language data."
                    .to_string(),
            );
        }
        if steps_rev
            .iter()
            .any(|step| step.note.contains("lossy") || step.note.contains("text"))
        {
            notes.push(
                "Some steps are text/layout approximations rather than exact visual clones."
                    .to_string(),
            );
        }

        Ok(ConversionPlan {
            source: from,
            target: to,
            steps: steps_rev,
            total_cost,
            direct,
            identity: false,
            catalog_rank,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Format;

    #[test]
    fn identity_is_free() {
        let catalog = Arc::new(Catalog::load().unwrap());
        let registry = EngineRegistry::native_only();
        let graph = ConversionGraph::build(catalog, &registry);
        let plan = graph
            .plan(Format::Txt, Format::Txt, &ConversionOptions::default())
            .unwrap();
        assert!(plan.identity);
        assert_eq!(plan.total_cost, 0);
    }

    #[test]
    fn ocr_request_does_not_use_a_text_layer() {
        let catalog = Arc::new(Catalog::load().unwrap());
        let registry = EngineRegistry::native_only();
        let graph = ConversionGraph::build(catalog, &registry);
        let options = ConversionOptions {
            ocr: true,
            ..Default::default()
        };
        let plan = graph.plan(Format::Pdf, Format::Txt, &options).unwrap();
        assert_eq!(plan.steps[0].engine, "native-ocr");
        assert!(plan.steps[0].ocr);
    }

    #[test]
    fn native_graph_covers_core_pairs() {
        let catalog = Arc::new(Catalog::load().unwrap());
        let registry = EngineRegistry::native_only();
        let graph = ConversionGraph::build(catalog, &registry);
        for (source, target) in [
            (Format::Txt, Format::Pdf),
            (Format::Markdown, Format::Docx),
            (Format::Docx, Format::Pdf),
            (Format::Pdf, Format::Markdown),
            (Format::Csv, Format::Xlsx),
            (Format::Json, Format::Yaml),
            (Format::Jpg, Format::Pdf),
        ] {
            graph
                .plan(source, target, &ConversionOptions::default())
                .unwrap_or_else(|e| panic!("{source:?} -> {target:?}: {e}"));
        }
    }
}
