use tracing::debug;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod analytics;
mod ui;
use analytics::{requests::ExecAgg, server::Server, reports::DefaultWriter};
use ui::app::render_app;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let file = std::fs::File::create("/tmp/skope.log").unwrap();
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "skope=INFO".into()),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(file))
        .init();

    let host = std::env::var("SKOPE_HOST").unwrap_or_else(|_| {
        debug!("SKOPE_HOST environment variable not set, using default (0.0.0.0)");
        "0.0.0.0".to_string()
    });
    let port = std::env::var("SKOPE_PORT").unwrap_or_else(|_| {
        debug!("SKOPE_PORT environment variable not set, using default (9001)");
        9001.to_string()
    });

    let report_writer = DefaultWriter::new(10);
    let server: Server = Server::new(host, port.parse().unwrap());
    debug!("Initialized with empty aggregation data");

    let exec_agg_ref = server.exec_agg.clone();
    server.init_connection(report_writer).await;
    render_app(exec_agg_ref)
}

