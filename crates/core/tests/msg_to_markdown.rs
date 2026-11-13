use docforge_core::catalog::Catalog;
use docforge_core::engine::EngineRegistry;
use docforge_core::format::Format;
use docforge_core::graph::ConversionGraph;
use docforge_core::options::ConversionOptions;
use docforge_core::service::ConversionService;
use std::sync::Arc;

#[tokio::test]
async fn outlook_msg_converts_to_markdown() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.msg");
    assert!(fixture.is_file(), "missing MSG fixture");
    let catalog = Arc::new(Catalog::load().unwrap());
    let registry = Arc::new(EngineRegistry::native_only());
    let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
    let service = ConversionService::new(catalog, graph, registry, std::env::temp_dir(), 120);
    let output = std::env::temp_dir().join(format!("docforge-msg-{}.md", uuid::Uuid::new_v4()));
    let outcome = service
        .convert_file(
            &fixture,
            &output,
            Format::Msg,
            Format::Markdown,
            &ConversionOptions::default(),
        )
        .await
        .expect("MSG -> Markdown should be routable");
    assert!(outcome.output_bytes > 0);
    let markdown = std::fs::read_to_string(&output).unwrap();
    assert!(markdown.contains("From:"), "markdown was: {markdown}");
    assert!(markdown.starts_with('#'), "markdown was: {markdown}");
    let _ = std::fs::remove_file(output);
}

#[test]
fn msg_to_markdown_is_planned() {
    let catalog = Arc::new(Catalog::load().unwrap());
    let registry = EngineRegistry::native_only();
    let graph = ConversionGraph::build(catalog, &registry);
    let plan = graph
        .plan(Format::Msg, Format::Markdown, &ConversionOptions::default())
        .unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].from, Format::Msg);
    assert_eq!(plan.steps[0].to, Format::Markdown);
}
