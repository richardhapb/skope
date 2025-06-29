use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use tracing::debug;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod analytics;
mod ui;
// mod database;
use analytics::{reports::DefaultWriter, requests::ExecAgg, server::Server};
use ui::app::render_app;

#[tokio::main]
async fn main() -> std::io::Result<()> {
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

    let report_writer = Arc::new(DefaultWriter::new());
    let server: Server = Server::new(host, port.parse().unwrap(), report_writer);
    debug!("Initialized with empty aggregation data");

    let exec_agg_ref = server.exec_agg.clone();
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
