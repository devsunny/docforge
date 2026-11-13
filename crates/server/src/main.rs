use anyhow::Context;
use clap::{Parser, Subcommand};
use docforge_core::format::Format;
use docforge_core::options::ConversionOptions;
use docforge_core::service::output_filename;
use docforge_server::{build_state, config::AppConfig};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "docforge",
    version,
    about = "Document conversion REST API and MCP server",
    long_about = "DocForge converts documents between PDF, Office, Markdown, HTML, spreadsheets, images, e-books, and structured data. Run `docforge serve` for the REST + MCP server or `docforge mcp --stdio` for a local stdio MCP server."
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
    /// Run an MCP server for a local client.
    Mcp {
        /// Use line-delimited JSON-RPC over stdin/stdout (the only supported mode).
        #[arg(long, default_value_t = true)]
        stdio: bool,
    },
    /// Print the 100 catalog conversions and live availability.
    Formats,
    /// Convert a file locally using the same planner and engines as the API.
    Convert {
        input: PathBuf,
        target: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        ocr: bool,
        #[arg(long, default_value = "eng")]
        ocr_language: String,
    },
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
                anyhow::bail!("only --stdio transport is available from the CLI; HTTP MCP is mounted by `docforge serve`");
            }
            let state = build_state(load_config()?)
                .await
                .context("configuration is invalid")?;
            docforge_mcp::stdio::run(state.mcp.clone()).await?;
            Ok(())
        }
        Command::Formats => {
            let state = build_state(load_config()?).await?;
            println!("RANK  SOURCE  TARGET  CATEGORY                               AVAILABLE");
            for entry in state.service.catalog().entries() {
                println!(
                    "{:<5} {:<8} {:<8} {:<38} {}",
                    entry.rank,
                    entry.source_format,
                    entry.target_format,
                    entry.category,
                    state.service.graph().is_catalog_available(entry.rank)
                );
            }
            Ok(())
        }
        Command::Convert {
            input,
            target,
            output,
            source,
            ocr,
            ocr_language,
        } => {
            let state = build_state(load_config()?).await?;
            let target = Format::parse(&target)?;
            let source = match source {
                Some(source) => Format::parse(&source)?,
                None => {
                    state
                        .service
                        .detect(
                            &input,
                            input.file_name().and_then(|name| name.to_str()),
                            None,
                        )?
                        .format
                }
            };
            let output = output.unwrap_or_else(|| {
                PathBuf::from(output_filename(
                    input
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("output"),
                    target,
                ))
            });
            let options = ConversionOptions {
                ocr,
                ocr_language,
                ..Default::default()
            };
            let outcome = state
                .service
                .convert_file(&input, &output, source, target, &options)
                .await?;
            println!(
                "{} -> {} in {} ms ({} bytes)\nroute: {}",
                source.label(),
                target.label(),
                outcome.duration_ms,
                outcome.output_bytes,
                outcome.plan.summary()
            );
            println!("output: {}", output.display());
            Ok(())
        }
    }
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
        .unwrap_or_else(|_| EnvFilter::new("docforge=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
