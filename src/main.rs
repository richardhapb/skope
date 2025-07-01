use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use analytics::data::RunnerReceiver;
use analytics::data::ServerReceiver;
use analytics::reports::RunnerWriter;
use clap::Parser;
use cli::Cli;
use cli::Commands;
use runner::cli::BashExecutor;
use runner::cli::BashScript;
use runner::cli::ScriptExecutor;
use std::process::Command;
use tracing::info;

use tracing::debug;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod analytics;
mod cli;
mod runner;
mod system;
mod ui;
// mod database;
use analytics::{reports::ServerWriter, requests::ExecAgg, server::Server};
use ui::app::render_app;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let file = std::fs::File::create("/tmp/skope.log").unwrap();
    let registry = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "skope=INFO".into()),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(file));

    // Configure logging based on environment: use file-only when in TUI mode,
    // and both stdout+file when running headless (in Docker, etc.)
    if atty::is(atty::Stream::Stdout) {
        // TUI mode: only log to file to avoid conflicts with the UI
        registry.init();
    } else {
        // Headless mode (Docker, etc.): log to both stdout and file
        registry
            .with(tracing_subscriber::fmt::layer()) // Default stdout layer
            .init();
    }

    let host = std::env::var("SKOPE_HOST").unwrap_or_else(|_| {
        debug!("SKOPE_HOST environment variable not set, using default (0.0.0.0)");
        "0.0.0.0".to_string()
    });
    let port = std::env::var("SKOPE_PORT").unwrap_or_else(|_| {
        debug!("SKOPE_PORT environment variable not set, using default (9001)");
        9001.to_string()
    });

    let cli = Cli::parse();

    match cli.command {
        Commands::Server => {
            let report_writer = Arc::new(ServerWriter::new());
            let data_provider = ServerReceiver::new(report_writer);
            let server: Server<ServerReceiver> =
                Server::new(host, port.parse().unwrap(), data_provider);
            let exec_agg_ref = server.data_provider.exec_agg.clone();

            debug!("Initialized with empty aggregation data");

            server.init_connection().await;
            if atty::is(atty::Stream::Stdout) {
                render_app(exec_agg_ref)
            } else {
                info!("TTY not available; TUI will not be displayed.");
                // Keep the session active
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        Commands::Runner { script, tag } => {
            let report_writer = RunnerWriter::new(&tag);
            let data_provider = RunnerReceiver::new(&tag, report_writer);
            let server: Server<RunnerReceiver<RunnerWriter>> =
                Server::new(host, port.parse().unwrap(), data_provider);
            let script = BashScript::new(&script);
            BashExecutor::execute(script).await?;
            server.init_connection().await;
            Ok(())
        }
        Commands::Diff { base, head } => {
            todo!()
        }
    }
}
