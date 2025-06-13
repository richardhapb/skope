use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::{collections::HashMap, hash::Hash, io::Write};
use tokio::{io::AsyncReadExt, net::TcpListener};
use tracing::{debug, error, info, trace};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
enum RequestType {
    HttpRequest,
    Function,
    CodeBlock,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
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

#[derive(Deserialize, Serialize, Debug)]
struct AggData {
    name: String,
    total_execs: i32,
    total_exec_time: f32,
    total_memory_usage: f32,
    avg_exec_memory: f32,
    avg_exec_time: f32,
}

impl AggData {
    fn new() -> Self {
        Self {
            name: String::new(),
            total_exec_time: 0.0,
            total_execs: 0,
            total_memory_usage: 0.0,
            avg_exec_time: 0.0,
            avg_exec_memory: 0.0,
        }
    }

    fn update(&self, exec_data: &ExecData) -> Self {
        let total_exec_time = self.total_exec_time + exec_data.exec_time;
        let total_memory_usage = self.total_memory_usage + exec_data.exec_memory_usage;
        let total_execs = self.total_execs + 1;
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
        Self {
            name: safe_name,
            total_execs: total_execs,
            total_memory_usage: total_memory_usage,
            total_exec_time: self.total_exec_time + exec_data.exec_time,
            avg_exec_time: total_exec_time / total_execs as f32,
            avg_exec_memory: self.total_memory_usage / total_execs as f32,
        }
    }
}

#[derive(Debug)]
struct ExecAgg {
    exec_data: HashMap<ExecData, AggData>,
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
        for (exec_data, agg_data) in &self.exec_data {
            // Use the name from exec_data as the key and the agg_data as the value
            map.serialize_entry(&exec_data.name, agg_data)?;
        }
        map.serialize_entry(
            "total_memory_global",
            &self
                .exec_data
                .keys()
                .max_by(|a, b| {
                    a.timestamp
                        .partial_cmp(&b.timestamp)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|k| k.total_memory_global)
                .unwrap_or(0.0),
        )?;

        // Finalize and return the map
        map.end()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
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
    let listener = TcpListener::bind(&binding).await?;

    info!("Skope listening on {}", binding);

    let agg_data = AggData::new();
    let mut exec_data: Vec<ExecData> = vec![];
    let mut exec_agg = ExecAgg::new();

    let mut n_exec = 0;

    loop {
        let (mut socket, addr) = listener.accept().await?;

        info!("Client connected: {}", addr);
        let mut buf = vec![0u8; 1024];
        loop {
            match socket.read(&mut buf).await {
                Ok(0) => {
                    info!(%addr, "Connection closed");
                    break;
                }
                Ok(n) => {
                    debug!(%addr, "Data received");
                    let data = String::from_utf8_lossy(&buf[..n]);
                    trace!(%data, "Received message");
                    let parsed = serde_json::from_str::<ExecData>(&data)?;
                    info!(?parsed);

                    let updated = agg_data.update(&parsed);
                    exec_agg.exec_data.insert(parsed.clone(), updated);

                    exec_data.push(parsed);

                    n_exec += 1;

                    if n_exec % 10 == 0 {
                        exec_agg.generate_report()?;
                        generate_apps_report(&exec_data)?;
                    }
                }
                Err(e) => {
                    error!("Error reading request: {}", e);
                    break;
                }
            }
        }
    }
}

fn generate_apps_report(execs_data: &Vec<ExecData>) -> std::io::Result<()>{
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
