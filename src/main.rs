use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::net::SocketAddr;
use std::sync::Arc;
use std::{collections::HashMap, hash::Hash, io::Write};
use tokio::sync::RwLock;
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
};
use tracing::{debug, error, info, trace};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod ui;
use ui::app::render_app;

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default)]
enum RequestType {
    HttpRequest,
    #[default]
    Function,
    CodeBlock,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
struct ExecData {
    exec_type: RequestType,
    name: String,
    module: String,
    timestamp: f32,
    exec_memory_usage: f32,
    total_memory_global: f32,
    exec_time: f32,
}

impl Hash for ExecData {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq for ExecData {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ExecData {}

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
struct AggData {
    name: String,
    total_execs: i32,
    total_exec_time: f32,
    total_memory_usage: f32,
    avg_exec_memory: f32,
    avg_exec_time: f32,
}

impl AggData {
    async fn update(exec_data: &ExecData, exec_agg: Arc<RwLock<ExecAgg>>) -> Self {
        let name = exec_data.name.clone();
        let safe_name = name
            .replace("/", "-")
            .replace("?", "x")
            .replace("\\", "_")
            .replace(":", "_")
            .replace("*", "_")
            .replace("\"", "_")
            .replace("<", "_")
            .replace(">", "_")
            .replace("|", "_")
            .replace(" ", "_");

        let prev_data = {
            let agg = exec_agg.read().await;
            agg.exec_data.get(&name).cloned()
        };

        let mut total_exec_time = exec_data.exec_time;
        let mut total_memory_usage = exec_data.exec_memory_usage;
        let mut total_execs = 1; // current

        // If exists data, use it
        if let Some(prev_data) = prev_data {
            total_exec_time = prev_data.total_exec_time + exec_data.exec_time;
            total_memory_usage = prev_data.total_memory_usage + exec_data.exec_memory_usage;
            total_execs = prev_data.total_execs + 1;
        }

        Self {
            name: safe_name,
            total_execs: total_execs,
            total_memory_usage: total_memory_usage,
            total_exec_time: total_exec_time,
            avg_exec_time: total_exec_time / total_execs as f32,
            avg_exec_memory: total_memory_usage / total_execs as f32,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct ExecAgg {
    exec_data: HashMap<String, AggData>,
}

impl ExecAgg {
    fn new() -> Self {
        Self {
            exec_data: HashMap::new(),
        }
    }

    fn generate_report(&self) -> std::io::Result<()> {
        if !fs::exists("reports")? {
            fs::create_dir("reports")?;
        }
        let report_path = "reports/report_agg.json";
        let mut file = File::create(report_path)?;

        let app_data = serde_json::to_string(&self)?;

        file.write_all(app_data.as_bytes())?;
        info!(%report_path, "Aggregate report written");

        Ok(())
    }
}

impl Serialize for ExecAgg {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // Create a map serializer with the number of items in exec_data
        let mut map = serializer.serialize_map(Some(self.exec_data.len()))?;

        // Iterate through the hashmap entries
        for (name, agg_data) in &self.exec_data {
            // Use the name as the key and the agg_data as the value
            map.serialize_entry(name, agg_data)?;
        }

        // Finalize and return the map
        map.end()
    }
}

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

    let binding = format!("{}:{}", host, port);
    let listener = TcpListener::bind(&binding)
        .await
        .expect(&format!("Error binding {}", binding));

    info!("Skope listening on {}", binding);

    let exec_data: Arc<RwLock<Vec<ExecData>>> = Arc::new(RwLock::new(vec![]));
    let exec_agg = Arc::new(RwLock::new(ExecAgg::new()));
    let n_exec: Arc<RwLock<u16>> = Arc::new(RwLock::new(0));

    debug!("Initialized with empty aggregation data");

    let max_connections = Arc::new(tokio::sync::Semaphore::new(500)); // Limit to 500 concurrent connections

    let exec_agg_ref = exec_agg.clone();
    tokio::spawn(async move {
        loop {
            // Wait for a connection slot to become available
            let permit = max_connections.clone().acquire_owned().await.unwrap();

            match listener.accept().await {
                Ok((socket, addr)) => {
                    info!("Client connected: {}", addr);

                    let exec_data_ref = exec_data.clone();
                    let exec_agg_ref = exec_agg_ref.clone();
                    let n_exec_ref = n_exec.clone();

                    tokio::spawn(async move {
                        // The permit is dropped when this task completes
                        let _permit = permit;

                        if let Err(e) =
                            handle_client(socket, addr, exec_data_ref, exec_agg_ref, n_exec_ref)
                                .await
                        {
                            error!(%e, "Error handling client");
                        }
                    });
                }
                Err(e) => {
                    // If we couldn't accept a connection, release the permit immediately
                    drop(permit);
                    error!("Error connecting to client: {}", e);
                    // Delay to avoids tight loop on errors
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });

    render_app(exec_agg)
}

fn generate_apps_report(execs_data: &Vec<ExecData>) -> std::io::Result<()> {
    if !fs::exists("reports")? {
        fs::create_dir("reports")?;
    }
    let report_path = "reports/report_apps.json";
    let mut file = File::create(report_path)?;

    let app_data = serde_json::to_string(&execs_data)?;

    file.write_all(app_data.as_bytes())?;
    info!(%report_path, "Apps report written");

    Ok(())
}

async fn handle_client(
    mut socket: TcpStream,
    addr: SocketAddr,
    exec_data: Arc<RwLock<Vec<ExecData>>>,
    exec_agg: Arc<RwLock<ExecAgg>>,
    n_exec: Arc<RwLock<u16>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Set a read timeout to prevent hanging connections
    socket.set_nodelay(true)?;

    let mut buf = vec![0u8; 1024];

    // Use a timeout for read operations
    let idle_timeout = tokio::time::sleep(std::time::Duration::from_secs(2));
    tokio::pin!(idle_timeout);

    loop {
        tokio::select! {
            _ = &mut idle_timeout => {
                debug!(%addr, "Connection idle timeout");
                return Ok(());
            }
            read_result = socket.read(&mut buf) => {
                match read_result {
                    Ok(0) => {
                        info!(%addr, "Connection closed");
                        return Ok(());
                    }
                    Ok(n) => {
                        debug!(%addr, bytes = n, "Data received");
                        let data = String::from_utf8_lossy(&buf[..n]);
                        trace!(%data, "Received message");

                        // Try to parse the data as JSON
                        match serde_json::from_str::<ExecData>(&data) {
                            Ok(parsed) => {
                                info!(?parsed);
                                process_data(parsed, &exec_data, &exec_agg, &n_exec).await?;
                            }
                            Err(e) => {
                                error!(%e, "Error parsing data");
                                return Err(Box::new(e))
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error reading request: {}", e);
                        return Err(Box::new(e));
                    }
                }
            }
        }
    }
}

async fn process_data(
    parsed: ExecData,
    exec_data: &Arc<RwLock<Vec<ExecData>>>,
    exec_agg: &Arc<RwLock<ExecAgg>>,
    n_exec: &Arc<RwLock<u16>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let new_data = AggData::update(&parsed, exec_agg.clone()).await;

    // Update the exec aggregation with the new aggregation data
    {
        let mut agg = exec_agg.write().await;
        agg.exec_data.insert(parsed.name.clone(), new_data);
        info!(
            "Updated exec_agg with new data point, total: {} items",
            agg.exec_data.len()
        );
    }

    // Store the raw execution data
    exec_data.write().await.push(parsed);

    // Increment execution counter and check if reports should be generated
    {
        let mut counter = n_exec.write().await;
        *counter += 1;
        if *counter % 10 == 0 {
            exec_agg.read().await.generate_report()?;
            generate_apps_report(&exec_data.read().await.to_vec())?;
        }
    }

    Ok(())
}
