//! The product conversion catalog embedded at compile time.
//!
//! Keeping the catalog as data (instead of hand-written `match` arms) means the
//! REST API, MCP tools, planner, and documentation all report exactly the same
//! 100 conversions.

use crate::format::{Format, FormatParseError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

const CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../catalog/conversions.json"
));

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("catalog JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("catalog must contain exactly 100 conversions, found {0}")]
    WrongCount(usize),
    #[error("catalog rank {0} is duplicated")]
    DuplicateRank(u16),
    #[error("catalog ranks must be 1..=100, found {0}")]
    BadRank(u16),
    #[error("catalog entry {rank} names an unknown format: {source}")]
    Format { rank: u16, source: FormatParseError },
    #[error("catalog entry {rank} has no source or target formats")]
    Empty { rank: u16 },
    #[error("catalog pair {from:?} -> {to:?} is duplicated")]
    DuplicatePair { from: Format, to: Format },
}

#[derive(Debug, Clone, Deserialize)]
struct RawCatalog {
    conversions: Vec<RawEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawEntry {
    rank: u16,
    source_format: String,
    target_format: String,
    category: String,
    use_case: String,
}

/// One row from `catalog/conversions.json`, with normalized formats.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub rank: u16,
    pub source_format: String,
    pub target_format: String,
    pub source_formats: Vec<Format>,
    pub target_formats: Vec<Format>,
    pub category: String,
    pub use_case: String,
    pub requires_ocr: bool,
    pub searchable: bool,
}

impl CatalogEntry {
    pub fn matches(&self, source: Format, target: Format) -> bool {
        self.source_formats.contains(&source) && self.target_formats.contains(&target)
    }

    /// Pair IDs are stable and useful as cache keys and UI keys.
    pub fn pair_ids(&self) -> Vec<(Format, Format)> {
        let mut pairs = Vec::new();
        for source in &self.source_formats {
            for target in &self.target_formats {
                pairs.push((*source, *target));
            }
        }
        pairs
    }

    pub fn to_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "rank": self.rank,
            "source": self.source_format,
            "target": self.target_format,
            "source_formats": self.source_formats.iter().map(|f| f.id()).collect::<Vec<_>>(),
            "target_formats": self.target_formats.iter().map(|f| f.id()).collect::<Vec<_>>(),
            "category": self.category,
            "use_case": self.use_case,
            "requires_ocr": self.requires_ocr,
            "searchable": self.searchable,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CategorySummary {
    pub name: String,
    pub count: usize,
    pub ranks: Vec<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogStats {
    pub conversions: usize,
    pub categories: usize,
    pub source_formats: usize,
    pub target_formats: usize,
    pub ocr_conversions: usize,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
    by_pair: HashMap<(Format, Format), Vec<usize>>,
}

impl Catalog {
    /// Parse and validate the embedded catalog.
    pub fn load() -> Result<Self, CatalogError> {
        Self::from_json(CATALOG_JSON)
    }

    pub fn from_json(json: &str) -> Result<Self, CatalogError> {
        let raw: RawCatalog = serde_json::from_str(json)?;
        if raw.conversions.len() != 100 {
            return Err(CatalogError::WrongCount(raw.conversions.len()));
        }

        let mut entries = Vec::with_capacity(raw.conversions.len());
        let mut seen_ranks = HashSet::new();
        let mut seen_pairs = HashSet::new();

        for raw_entry in raw.conversions {
            if !(1..=100).contains(&raw_entry.rank) {
                return Err(CatalogError::BadRank(raw_entry.rank));
            }
            if !seen_ranks.insert(raw_entry.rank) {
                return Err(CatalogError::DuplicateRank(raw_entry.rank));
            }

            let sources = Format::parse_catalog_label(&raw_entry.source_format).map_err(|e| {
                CatalogError::Format {
                    rank: raw_entry.rank,
                    source: e,
                }
            })?;
            let targets = Format::parse_catalog_label(&raw_entry.target_format).map_err(|e| {
                CatalogError::Format {
                    rank: raw_entry.rank,
                    source: e,
                }
            })?;
            if sources.is_empty() || targets.is_empty() {
                return Err(CatalogError::Empty {
                    rank: raw_entry.rank,
                });
            }

            let requires_ocr = Format::label_requests_ocr(&raw_entry.source_format)
                || Format::label_requests_ocr(&raw_entry.target_format);
            let searchable = Format::label_is_searchable(&raw_entry.target_format);
            let entry = CatalogEntry {
                rank: raw_entry.rank,
                source_format: raw_entry.source_format,
                target_format: raw_entry.target_format,
                source_formats: sources,
                target_formats: targets,
                category: raw_entry.category,
                use_case: raw_entry.use_case,
                requires_ocr,
                searchable,
            };

            for (from, to) in entry.pair_ids() {
                if !seen_pairs.insert((from, to)) {
                    return Err(CatalogError::DuplicatePair { from, to });
                }
            }
            entries.push(entry);
        }

        entries.sort_by_key(|e| e.rank);
        for (index, entry) in entries.iter().enumerate() {
            let expected = (index + 1) as u16;
            if entry.rank != expected {
                return Err(CatalogError::BadRank(entry.rank));
            }
        }

        let mut by_pair: HashMap<(Format, Format), Vec<usize>> = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            for (source, target) in entry.pair_ids() {
                by_pair.entry((source, target)).or_default().push(index);
            }
        }

        Ok(Self { entries, by_pair })
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn get(&self, rank: u16) -> Option<&CatalogEntry> {
        self.entries.get((rank as usize).checked_sub(1)?)
    }

    pub fn find(&self, source: Format, target: Format) -> Vec<&CatalogEntry> {
        self.by_pair
            .get(&(source, target))
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|i| self.entries.get(*i))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn best_for(
        &self,
        source: Format,
        target: Format,
        ocr_requested: bool,
    ) -> Option<&CatalogEntry> {
        self.find(source, target).into_iter().find(|entry| {
            if entry.searchable {
                ocr_requested
            } else {
                entry.requires_ocr == ocr_requested || !entry.requires_ocr
            }
        })
    }

    pub fn contains_pair(&self, source: Format, target: Format) -> bool {
        self.by_pair.contains_key(&(source, target))
    }

    pub fn categories(&self) -> Vec<CategorySummary> {
        let mut map: BTreeMap<String, Vec<u16>> = BTreeMap::new();
        for entry in &self.entries {
            map.entry(entry.category.clone())
                .or_default()
                .push(entry.rank);
        }
        map.into_iter()
            .map(|(name, ranks)| CategorySummary {
                name,
                count: ranks.len(),
                ranks,
            })
            .collect()
    }

    pub fn stats(&self) -> CatalogStats {
        let mut sources = HashSet::new();
        let mut targets = HashSet::new();
        for entry in &self.entries {
            sources.extend(entry.source_formats.iter().copied());
            targets.extend(entry.target_formats.iter().copied());
        }
        CatalogStats {
            conversions: self.entries.len(),
            categories: self.categories().len(),
            source_formats: sources.len(),
            target_formats: targets.len(),
            ocr_conversions: self.entries.iter().filter(|e| e.requires_ocr).count(),
        }
    }

    pub fn targets_for(&self, source: Format) -> Vec<Format> {
        let mut targets: Vec<Format> = self
            .entries
            .iter()
            .filter(|e| e.source_formats.contains(&source))
            .flat_map(|e| e.target_formats.iter().copied())
            .collect();
        targets.sort();
        targets.dedup();
        targets
    }

    pub fn sources_for(&self, target: Format) -> Vec<Format> {
        let mut sources: Vec<Format> = self
            .entries
            .iter()
            .filter(|e| e.target_formats.contains(&target))
            .flat_map(|e| e.source_formats.iter().copied())
            .collect();
        sources.sort();
        sources.dedup();
        sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid() {
        let catalog = Catalog::load().expect("catalog must load");
        assert_eq!(catalog.entries().len(), 100);
        let stats = catalog.stats();
        assert_eq!(stats.categories, 6);
        assert_eq!(stats.conversions, 100);
        assert!(stats.source_formats >= 30);
        assert!(stats.target_formats >= 20);
    }

    #[test]
    fn ranks_are_ordered_and_unique() {
        let catalog = Catalog::load().unwrap();
        for (index, entry) in catalog.entries().iter().enumerate() {
            assert_eq!(entry.rank as usize, index + 1);
        }
    }

    #[test]
    fn important_pairs_are_present() {
        let catalog = Catalog::load().unwrap();
        for (source, target) in [
            (Format::Pdf, Format::Docx),
            (Format::Docx, Format::Pdf),
            (Format::Pdf, Format::Markdown),
            (Format::Xlsx, Format::Csv),
            (Format::Jpg, Format::Pdf),
            (Format::Epub, Format::Pdf),
            (Format::Cbz, Format::Pdf),
        ] {
            assert!(
                catalog.contains_pair(source, target),
                "missing {source:?} -> {target:?}"
            );
        }
    }

    #[test]
    fn ocr_pair_is_flagged() {
        let catalog = Catalog::load().unwrap();
        let entry = catalog.get(69).unwrap();
        assert!(entry.requires_ocr);
        assert!(entry.searchable);
        assert_eq!(entry.source_formats, vec![Format::Pdf]);
        assert_eq!(entry.target_formats, vec![Format::Pdf]);
    }
}
