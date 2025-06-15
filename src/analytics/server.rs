use super::reports::Reportable;
use super::requests::{AggData, ExecAgg, ExecData};

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, trace};

pub async fn init_connection(host: &str, port: &str, exec_agg: &Arc<RwLock<ExecAgg>>) {
    let binding = format!("{}:{}", host, port);
    let listener = TcpListener::bind(&binding)
        .await
        .expect(&format!("Error binding {}", binding));

    info!("Skope listening on {}", binding);

    let max_connections = Arc::new(tokio::sync::Semaphore::new(500)); // Limit to 500 concurrent connections

    let n_exec: Arc<Mutex<u16>> = Arc::new(Mutex::new(0));
    let exec_data: Arc<RwLock<Vec<ExecData>>> = Arc::new(RwLock::new(vec![]));
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
}

pub async fn handle_client(
    mut socket: TcpStream,
    addr: SocketAddr,
    exec_data: Arc<RwLock<Vec<ExecData>>>,
    exec_agg: Arc<RwLock<ExecAgg>>,
    n_exec: Arc<Mutex<u16>>,
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
                                process_data(parsed, &exec_data, &exec_agg).await?;

                            // Increment execution counter and check if reports should be generated
                            {
                                let mut counter = n_exec.lock().await;
                                *counter += 1;
                                if *counter % 10 == 0 {
                                    exec_agg.read().await.generate_report(None)?;
                                    exec_data.read().await.generate_report(None)?;
                                }
                            }

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

pub async fn process_data(
    parsed: ExecData,
    exec_data: &Arc<RwLock<Vec<ExecData>>>,
    exec_agg: &Arc<RwLock<ExecAgg>>,
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};

    struct AppData {
        exec_data: ExecData,
        exec_data_vec: Arc<RwLock<Vec<ExecData>>>,
        exec_agg: Arc<RwLock<ExecAgg>>,
    }

    impl AppData {
        fn new() -> Self {
            Self {
                exec_data: ExecData::default(),
                exec_data_vec: Arc::new(RwLock::new(vec![])),
                exec_agg: Arc::new(RwLock::new(ExecAgg::default())),
            }
        }
    }

    #[tokio::test]
    // Ensure the counter is updated by reference properly
    async fn test_counter() {
        let counter = Arc::new(Mutex::new(0));

        let data = AppData::new();

        process_data(data.exec_data.clone(), &data.exec_data_vec, &data.exec_agg)
            .await
            .unwrap();
        assert_eq!(*counter.lock().await, 1);
        process_data(data.exec_data, &data.exec_data_vec, &data.exec_agg)
            .await
            .unwrap();
        assert_eq!(*counter.lock().await, 2);
    }

    #[tokio::test]
    async fn test_generate_report() {
        let data = AppData::new();
        let temp_dir = std::env::temp_dir();
        let path = std::path::Path::new(temp_dir.to_str().unwrap());
        let filepath = path.join("test_report.json");

        for _ in 0..11 {
            process_data(data.exec_data.clone(), &data.exec_data_vec, &data.exec_agg)
                .await
                .unwrap();
        }

        // Verify if the file was created
        assert!(std::path::Path::exists(&filepath), "File doesn't exist");
    }
}
