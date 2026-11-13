//! Experimental DeckCraft PPTX → PDF engine.
//!
//! This adapter is feature-gated behind `deckcraft` and is a spike, not the
//! default PPTX path. It uses DeckCraft's public PPTX importer and PDF writer.

use crate::engine::{Engine, EngineContext, Support};
use crate::error::{ConvertError, Result};
use crate::format::Format;
use async_trait::async_trait;

#[derive(Default)]
pub struct DeckCraftEngine;

#[async_trait]
impl Engine for DeckCraftEngine {
    fn id(&self) -> &'static str {
        "deckcraft"
    }

    fn name(&self) -> &'static str {
        "DeckCraft PPTX"
    }

    fn description(&self) -> &'static str {
        "Experimental DeckCraft PPTX import and PDF export"
    }

    fn priority(&self) -> u32 {
        3
    }

    fn available(&self) -> bool {
        true
    }

    fn supports(&self, from: Format, to: Format) -> Option<Support> {
        (from == Format::Pptx && to == Format::Pdf).then(|| {
            Support::new(
                4,
                "DeckCraft slide renderer + selectable PDF text layer (spike)",
            )
            .layout()
        })
    }

    async fn convert(&self, ctx: &EngineContext<'_>) -> Result<()> {
        let bytes = std::fs::read(ctx.input)?;
        let presentation = deckcraft_pptx::import(&bytes)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        let options = deckcraft_pdf::PdfOptions {
            dpi: f64::from(ctx.options.pdf.dpi),
            text_layer: true,
            ..Default::default()
        };
        let pdf = deckcraft_pdf::export(&presentation, &options)
            .map_err(|error| ConvertError::engine(self.id(), format!("{error}")))?;
        std::fs::write(ctx.output, pdf)?;
        Ok(())
    }
}
