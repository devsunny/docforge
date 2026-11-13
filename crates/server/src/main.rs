use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand};
use docforge_auth::{AuthConfig, AuthState};
use docforge_core::catalog::Catalog;
use docforge_core::engine::EngineRegistry;
use docforge_core::format::Format;
use docforge_core::graph::ConversionGraph;
use docforge_core::options::{ConversionOptions, PageSize};
use docforge_core::service::{output_filename, ConversionService};
use docforge_server::{build_state, config::AppConfig};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "docforge",
    version,
    about = "DocForge document conversion CLI and server",
    long_about = "Convert documents from the terminal with the same planner and engines as the DocForge REST API and MCP server.\n\nExamples:\n  docforge convert report.pdf --to docx -o report.docx\n  docforge convert notes.md pdf\n  cat notes.md | docforge convert - --to pdf --stdout > notes.pdf\n  docforge batch ./inbox --to pdf --output-dir ./out\n  docforge plan msg md\n  docforge serve\n  docforge mcp --stdio"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the REST API, OAuth server, and MCP HTTP endpoint.
    Serve {
        /// Override BIND_ADDR (for example 0.0.0.0:8080).
        #[arg(long)]
        bind: Option<String>,
        /// Override PUBLIC_BASE_URL used in discovery and download links.
        #[arg(long)]
        public_url: Option<String>,
    },
    /// Run a local MCP server over stdio.
    Mcp {
        /// Use line-delimited JSON-RPC over stdin/stdout.
        #[arg(long)]
        stdio: bool,
    },
    /// Print the 100 catalog conversions and live availability.
    Formats {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Only show conversions routable in this installation.
        #[arg(long)]
        available_only: bool,
        /// Filter by exact category name.
        #[arg(long)]
        category: Option<String>,
    },
    /// List conversion engines and whether their tools are installed.
    Engines {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show the shortest route between two formats.
    Plan {
        /// Source format ID, extension, or MIME type.
        source: String,
        /// Target format ID, extension, or MIME type.
        target: String,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        options: CliOptions,
    },
    /// Convert one file locally.
    Convert {
        /// Input file, or '-' to read bytes from stdin.
        input: PathBuf,
        /// Target format (positional alternative to --to).
        #[arg(value_name = "TARGET")]
        target_pos: Option<String>,
        /// Target format ID, extension, or MIME type.
        #[arg(short = 't', long = "to", value_name = "FORMAT")]
        target: Option<String>,
        /// Output path. Defaults to <input-stem>.<target-extension>.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Source format override; detected when omitted.
        #[arg(long = "from", value_name = "FORMAT")]
        source: Option<String>,
        /// Write the converted bytes to stdout; logs go to stderr.
        #[arg(long, conflicts_with = "json")]
        stdout: bool,
        /// Overwrite an existing output file.
        #[arg(long)]
        force: bool,
        /// Emit a machine-readable JSON result.
        #[arg(long)]
        json: bool,
        /// Suppress the human-readable summary.
        #[arg(long)]
        quiet: bool,
        #[command(flatten)]
        options: CliOptions,
    },
    /// Convert every file in a directory.
    Batch {
        /// Directory containing input files.
        input_dir: PathBuf,
        /// Target format for every file.
        #[arg(short = 't', long = "to", value_name = "FORMAT")]
        target: String,
        /// Output directory. Defaults to the current directory.
        #[arg(short = 'o', long)]
        output_dir: Option<PathBuf>,
        /// Recurse into subdirectories.
        #[arg(short, long)]
        recursive: bool,
        /// Overwrite existing output files.
        #[arg(long)]
        force: bool,
        /// Keep converting after a file fails.
        #[arg(long)]
        continue_on_error: bool,
        /// Emit a machine-readable JSON summary.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        options: CliOptions,
    },
}

#[derive(Args, Debug, Clone)]
struct CliOptions {
    /// Run OCR for image/scanned-PDF sources.
    #[arg(long)]
    ocr: bool,
    /// Tesseract language(s), e.g. eng, deu, or eng+deu.
    #[arg(long, default_value = "eng")]
    ocr_language: String,
    /// Prefer layout-preserving engines.
    #[arg(long)]
    preserve_layout: bool,
    /// PDF rasterization/output DPI (36..1200).
    #[arg(long)]
    pdf_dpi: Option<u16>,
    /// JPEG quality (1..100).
    #[arg(long)]
    image_quality: Option<u8>,
    /// PDF page size: a4, letter, or legal.
    #[arg(long, value_name = "SIZE")]
    page_size: Option<String>,
    /// Use landscape PDF pages.
    #[arg(long)]
    landscape: bool,
    /// PDF margin in millimetres.
    #[arg(long)]
    margin_mm: Option<f32>,
    /// Hard conversion timeout in seconds.
    #[arg(long)]
    timeout_secs: Option<u64>,
    /// Single-byte CSV/TSV delimiter (default ',' or tab).
    #[arg(long)]
    csv_delimiter: Option<String>,
}

impl CliOptions {
    fn into_options(self) -> anyhow::Result<ConversionOptions> {
        let mut options = ConversionOptions {
            ocr: self.ocr,
            ocr_language: self.ocr_language,
            preserve_layout: self.preserve_layout,
            timeout_secs: self.timeout_secs,
            ..Default::default()
        };
        if let Some(dpi) = self.pdf_dpi {
            options.pdf.dpi = dpi;
        }
        if let Some(quality) = self.image_quality {
            options.image.quality = quality;
        }
        if let Some(size) = self.page_size {
            options.pdf.page_size = match size.to_ascii_lowercase().as_str() {
                "a4" => PageSize::A4,
                "letter" => PageSize::Letter,
                "legal" => PageSize::Legal,
                other => bail!("page size must be a4, letter, or legal (got '{other}')"),
            };
        }
        options.pdf.landscape = self.landscape;
        if let Some(margin) = self.margin_mm {
            options.pdf.margin_mm = margin;
        }
        if let Some(delimiter) = self.csv_delimiter {
            options.csv.delimiter = delimiter;
        }
        options
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid options: {error}"))?;
        Ok(options)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { bind, public_url } => serve(bind, public_url).await,
        Command::Mcp { stdio } => {
            if !stdio {
                bail!("only --stdio is supported from the CLI; use `docforge serve` for HTTP MCP");
            }
            let service = local_service()?;
            let auth = Arc::new(AuthState::new(AuthConfig::for_test("http://localhost")));
            let mcp = Arc::new(docforge_mcp::McpState::new(
                service,
                auth,
                "http://localhost",
            ));
            docforge_mcp::stdio::run(mcp).await?;
            Ok(())
        }
        Command::Formats {
            json,
            available_only,
            category,
        } => formats(json, available_only, category),
        Command::Engines { json } => engines(json),
        Command::Plan {
            source,
            target,
            json,
            options,
        } => plan(source, target, json, options),
        Command::Convert {
            input,
            target_pos,
            target,
            output,
            source,
            stdout,
            force,
            json,
            quiet,
            options,
        } => {
            convert(
                input, target_pos, target, output, source, stdout, force, json, quiet, options,
            )
            .await
        }
        Command::Batch {
            input_dir,
            target,
            output_dir,
            recursive,
            force,
            continue_on_error,
            json,
            options,
        } => {
            batch(
                input_dir,
                target,
                output_dir,
                recursive,
                force,
                continue_on_error,
                json,
                options,
            )
            .await
        }
    }
}

fn local_service() -> anyhow::Result<Arc<ConversionService>> {
    let catalog = Arc::new(Catalog::load()?);
    let registry = Arc::new(EngineRegistry::detect());
    let graph = Arc::new(ConversionGraph::build(catalog.clone(), &registry));
    Ok(Arc::new(ConversionService::new(
        catalog,
        graph,
        registry,
        cli_temp_root(),
        300,
    )))
}

/// Pick a writable scratch directory. The system temp directory is normally
/// correct, but locked-down Windows/OneDrive environments can deny access to
/// it; fall back to a project-local directory.
fn cli_temp_root() -> PathBuf {
    if let Ok(value) = std::env::var("DOCFORGE_TMP") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    let mut candidates = vec![std::env::temp_dir().join("docforge-cli")];
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current.join(".docforge-tmp"));
    }
    for candidate in candidates {
        if std::fs::create_dir_all(&candidate).is_err() {
            continue;
        }
        if tempfile::Builder::new()
            .prefix("docforge-probe-")
            .tempdir_in(&candidate)
            .is_err()
        {
            continue;
        }
        let probe = candidate.join(format!(".write-probe-{}", uuid::Uuid::new_v4()));
        if std::fs::write(&probe, b"docforge").is_ok() {
            let _ = std::fs::remove_file(probe);
            return candidate;
        }
    }
    std::env::temp_dir()
}

fn formats(json: bool, available_only: bool, category: Option<String>) -> anyhow::Result<()> {
    let service = local_service()?;
    let rows: Vec<_> = service
        .catalog()
        .entries()
        .iter()
        .filter(|entry| {
            category
                .as_deref()
                .map(|value| entry.category.eq_ignore_ascii_case(value))
                .unwrap_or(true)
        })
        .filter(|entry| !available_only || service.graph().is_catalog_available(entry.rank))
        .collect();
    if json {
        let values: Vec<serde_json::Value> = rows
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "rank": entry.rank,
                    "source": entry.source_format,
                    "target": entry.target_format,
                    "category": entry.category,
                    "use_case": entry.use_case,
                    "available": service.graph().is_catalog_available(entry.rank),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        println!("RANK  SOURCE  TARGET  CATEGORY                               AVAILABLE");
        for entry in rows {
            println!(
                "{:<5} {:<8} {:<8} {:<38} {}",
                entry.rank,
                entry.source_format,
                entry.target_format,
                entry.category,
                service.graph().is_catalog_available(entry.rank)
            );
        }
    }
    Ok(())
}

fn engines(json: bool) -> anyhow::Result<()> {
    let service = local_service()?;
    let engines = service.registry().descriptors();
    if json {
        println!("{}", serde_json::to_string_pretty(&engines)?);
    } else {
        println!("{:<14} {:<9} {:<10} DESCRIPTION", "ID", "AVAILABLE", "TOOL");
        for engine in engines {
            println!(
                "{:<14} {:<9} {:<10} {}",
                engine.id,
                engine.available,
                engine.tool.unwrap_or_else(|| "-".to_string()),
                engine.description
            );
        }
    }
    Ok(())
}

fn plan(source: String, target: String, json: bool, options: CliOptions) -> anyhow::Result<()> {
    let service = local_service()?;
    let source = Format::parse(&source)?;
    let target = Format::parse(&target)?;
    let options = options.into_options()?;
    let plan = service.plan(source, target, &options)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "source": source,
                "target": target,
                "summary": plan.summary(),
                "plan": plan,
            }))?
        );
    } else {
        println!("{} -> {}", source.label(), target.label());
        println!("route: {}", plan.summary());
        for (index, step) in plan.steps.iter().enumerate() {
            println!(
                "  {}. {} -> {}  [{}]{}",
                index + 1,
                step.from.label(),
                step.to.label(),
                step.engine,
                if step.ocr { "  (OCR)" } else { "" }
            );
            println!("     {}", step.note);
        }
        for note in &plan.notes {
            println!("note: {note}");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn convert(
    input: PathBuf,
    target_pos: Option<String>,
    target: Option<String>,
    output: Option<PathBuf>,
    source: Option<String>,
    stdout: bool,
    force: bool,
    json: bool,
    quiet: bool,
    options: CliOptions,
) -> anyhow::Result<()> {
    let target = match (target, target_pos) {
        (Some(flag), Some(positional)) if !flag.eq_ignore_ascii_case(&positional) => {
            bail!("two different target formats were provided: --to {flag} and {positional}")
        }
        (Some(flag), _) => flag,
        (None, Some(positional)) => positional,
        (None, None) => {
            bail!("a target format is required: use `--to <format>` or a positional target")
        }
    };
    let target = Format::parse(&target)?;
    let options = options.into_options()?;
    let service = local_service()?;

    let from_stdin = input.as_os_str() == "-";
    if from_stdin && output.is_none() && !stdout {
        bail!("reading from stdin requires --output <path> or --stdout");
    }
    let (input, temp_input) = if from_stdin {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes)
            .context("cannot read stdin")?;
        if bytes.is_empty() {
            bail!("stdin was empty");
        }
        let path = cli_temp_root().join(format!("docforge-stdin-{}.bin", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes)?;
        (path.clone(), Some(path))
    } else {
        if !input.is_file() {
            bail!("input file does not exist: {}", input.display());
        }
        (input, None)
    };

    let source = match source {
        Some(source) => Format::parse(&source)?,
        None => {
            service
                .detect(
                    &input,
                    input.file_name().and_then(|name| name.to_str()),
                    None,
                )?
                .format
        }
    };

    let desired_output = output.clone().unwrap_or_else(|| {
        PathBuf::from(output_filename(
            input
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output"),
            target,
        ))
    });
    let temp_output = stdout.then(|| {
        cli_temp_root().join(format!(
            "docforge-stdout-{}.{}",
            uuid::Uuid::new_v4(),
            target.extension()
        ))
    });
    let output_path = temp_output.as_ref().unwrap_or(&desired_output);
    if output_path.exists() && !force && !stdout {
        bail!(
            "output file already exists: {} (use --force to overwrite)",
            output_path.display()
        );
    }

    let outcome = service
        .convert_file(&input, output_path, source, target, &options)
        .await?;

    if stdout {
        let bytes = std::fs::read(output_path)?;
        std::io::stdout().write_all(&bytes)?;
        std::io::stdout().flush()?;
    }

    if let Some(temp) = temp_output {
        let _ = std::fs::remove_file(temp);
    }
    if let Some(temp) = temp_input {
        let _ = std::fs::remove_file(temp);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "input": input,
                "output": if stdout { serde_json::Value::Null } else { serde_json::json!(desired_output) },
                "source_format": source,
                "target_format": target,
                "bytes": outcome.output_bytes,
                "duration_ms": outcome.duration_ms,
                "route": outcome.plan.summary(),
                "plan": outcome.plan,
            }))?
        );
    } else if !quiet {
        let summary = format!(
            "{} -> {} in {} ms ({} bytes)\nroute: {}",
            source.label(),
            target.label(),
            outcome.duration_ms,
            outcome.output_bytes,
            outcome.plan.summary()
        );
        if stdout {
            eprintln!("{summary}");
        } else {
            println!("{summary}");
            println!("output: {}", desired_output.display());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn batch(
    input_dir: PathBuf,
    target: String,
    output_dir: Option<PathBuf>,
    recursive: bool,
    force: bool,
    continue_on_error: bool,
    json: bool,
    options: CliOptions,
) -> anyhow::Result<()> {
    if !input_dir.is_dir() {
        bail!("input directory does not exist: {}", input_dir.display());
    }
    let target = Format::parse(&target)?;
    let options = options.into_options()?;
    let service = local_service()?;
    let output_dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&output_dir)?;
    let files = collect_files(&input_dir, recursive)?;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut results = Vec::new();

    for input in files {
        let source = match service.detect(
            &input,
            input.file_name().and_then(|name| name.to_str()),
            None,
        ) {
            Ok(detected) => detected.format,
            Err(error) => {
                failed += 1;
                eprintln!("skip {}: {error}", input.display());
                if !continue_on_error {
                    bail!("batch stopped at {}", input.display());
                }
                continue;
            }
        };
        let file_name = input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output");
        let output = output_dir.join(output_filename(file_name, target));
        if output.exists() && !force {
            failed += 1;
            eprintln!(
                "skip {}: output exists (use --force): {}",
                input.display(),
                output.display()
            );
            if !continue_on_error {
                bail!("batch stopped because output exists");
            }
            continue;
        }
        match service
            .convert_file(&input, &output, source, target, &options)
            .await
        {
            Ok(outcome) => {
                succeeded += 1;
                if !json {
                    println!(
                        "{} -> {} ({}, {} bytes)",
                        input.display(),
                        output.display(),
                        outcome.plan.summary(),
                        outcome.output_bytes
                    );
                }
                results.push(serde_json::json!({
                    "input": input,
                    "output": output,
                    "source_format": source,
                    "target_format": target,
                    "bytes": outcome.output_bytes,
                    "duration_ms": outcome.duration_ms,
                    "route": outcome.plan.summary(),
                }));
            }
            Err(error) => {
                failed += 1;
                eprintln!("failed {}: {error}", input.display());
                if !continue_on_error {
                    bail!("batch stopped at {}", input.display());
                }
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "succeeded": succeeded,
                "failed": failed,
                "results": results,
            }))?
        );
    } else {
        println!("{succeeded} converted, {failed} failed");
    }
    if failed > 0 && !continue_on_error {
        bail!("batch finished with {failed} failure(s)");
    }
    Ok(())
}

fn collect_files(dir: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                files.extend(collect_files(&path, true)?);
            }
            continue;
        }
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

async fn serve(bind: Option<String>, public_url: Option<String>) -> anyhow::Result<()> {
    let mut config = load_config()?;
    if let Some(bind) = bind {
        config.bind = bind;
    }
    if let Some(public_url) = public_url {
        config.public_base_url = public_url.trim_end_matches('/').to_string();
    }
    config.auth.validate().map_err(anyhow::Error::msg)?;
    let state = build_state(config).await?;
    let _cleanup = {
        let state = state.clone();
        let ttl = state.config.job_ttl;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(900));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                state.jobs.cleanup(ttl);
            }
        })
    };
    let _auth_cleanup = docforge_auth::spawn_cleanup(state.auth.clone());
    let app = docforge_server::app(state.clone());
    let listener = tokio::net::TcpListener::bind(&state.config.bind)
        .await
        .with_context(|| format!("cannot bind {}", state.config.bind))?;
    tracing::info!(
        bind = %state.config.bind,
        public_url = %state.config.public_base_url,
        issuer = %state.config.auth.issuer,
        engines = state.service.registry().available_count(),
        "DocForge is ready"
    );
    println!("DocForge listening on http://{}", state.config.bind);
    println!("REST API: {}", state.config.public_base_url);
    println!(
        "MCP endpoint: {}/mcp (OAuth 2.1 + PKCE + DCR)",
        state.config.public_base_url
    );
    println!(
        "OAuth discovery: {}/.well-known/oauth-authorization-server",
        state.config.auth.issuer
    );
    println!("API docs: {}/docs", state.config.public_base_url);
    if state.config.auth.demo_mode && !state.config.auth.is_external() {
        println!(
            "Demo login: username '{}' (set AUTH_PASSWORD to change)",
            state.config.auth.username
        );
    }
    if state.config.auth.is_external() {
        println!(
            "Identity provider: {} (Keycloak admin console: http://localhost:8081)",
            state.config.auth.authorization_server_issuer()
        );
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

fn load_config() -> anyhow::Result<AppConfig> {
    AppConfig::from_env().map_err(anyhow::Error::msg)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("docforge=warn,tower_http=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
