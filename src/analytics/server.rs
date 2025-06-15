use super::reports::{ReportWriter, Reportable};
use super::requests::{AggData, ExecAgg, ExecData};

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, trace};

#[derive(Default, Debug, Clone)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub exec_agg: Arc<RwLock<ExecAgg>>,
    pub exec_data: Arc<RwLock<Vec<ExecData>>>,
    iteration: Arc<Mutex<usize>>,
}

impl Server {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            ..Default::default()
        }
    }

    pub async fn init_connection<T>(self, report_writer: T)
    where
        T: ReportWriter + Clone + Send + 'static,
    {
        let binding = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&binding)
            .await
            .expect(&format!("Error binding {}", binding));

        info!("Skope listening on {}", binding);

        let max_connections = Arc::new(tokio::sync::Semaphore::new(500)); // Limit to 500 concurrent connections

        tokio::spawn(async move {
            let server_instance = self;

            loop {
                // Wait for a connection slot to become available
                let permit = max_connections.clone().acquire_owned().await.unwrap();

                match listener.accept().await {
                    Ok((socket, addr)) => {
                        info!("Client connected: {}", addr);

                        let server_for_client_task = server_instance.clone();
                        let report_writer_inner = report_writer.clone();

                        tokio::spawn(async move {
                            // The permit is dropped when this task completes
                            let _permit = permit;

                            if let Err(e) = server_for_client_task
                                .handle_client(socket, addr, report_writer_inner)
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

    pub async fn handle_client<T>(
        &self,
        mut socket: TcpStream,
        addr: SocketAddr,
        report_writer: T,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: ReportWriter + Clone + Send + 'static,
    {
        // Set a read timeout to prevent hanging connections
        socket.set_nodelay(true)?;

        let mut buf = vec![0u8; 1024];

        let n_exec_ref_inner = self.iteration.clone();

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
                                    self.process_data(parsed).await?;

                            let mut counter = n_exec_ref_inner.lock().await;
                            *counter += 1;

                            let should_generate_report =
                                *counter == report_writer.get_iterations_threshold();
                            // IMPORTANT: Drop the lock as soon as you're done with it to avoid deadlocks
                            // and allow other tasks to acquire the lock.
                            drop(counter);

                            let exec_data_clone = self.exec_data.read().await.clone();
                            let exec_agg_clone = self.exec_data.read().await.clone();

                            if should_generate_report {
                                let reportables: Vec<Box<dyn Reportable>> =
                                    vec![Box::new(exec_data_clone), Box::new(exec_agg_clone)];

                                let report_writer_inner = report_writer.clone();

                                tokio::task::spawn_blocking(move || {
                                    if let Err(e) = report_writer_inner.write_reports(reportables) {
                                        error!(%e, "Error writing reports")
                                    }
                                });
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
        &self,
        parsed: ExecData,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let new_data = AggData::update(&parsed, self.exec_agg.clone()).await;

        // Update the exec aggregation with the new aggregation data
        {
            let mut agg = self.exec_agg.write().await;
            agg.exec_data.insert(parsed.name.clone(), new_data);
            info!(
                "Updated exec_agg with new data point, total: {} items",
                agg.exec_data.len()
            );
        }

        // Store the raw execution data
        self.exec_data.write().await.push(parsed);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::reports::ReportWriter;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::{net::TcpStream, sync::Arc};
    use tokio::sync::RwLock;

    struct AppData {
        exec_data: ExecData,
        #[allow(dead_code)]
        exec_data_vec: Arc<RwLock<Vec<ExecData>>>,
        #[allow(dead_code)]
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

    #[derive(Clone)]
    struct MockReportWriter {
        iterations_threshold: usize,
        generated_flag: Arc<AtomicBool>, // Use Arc<AtomicBool> directly
    }

    impl ReportWriter for MockReportWriter {
        fn write_reports(&self, _reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()> {
            self.generated_flag.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn get_iterations_threshold(&self) -> usize {
            self.iterations_threshold
        }

        fn set_iterations_threshold(&mut self, iterations: usize) {
            self.iterations_threshold = iterations;
        }
    }

    impl MockReportWriter {
        fn new() -> (Self, Arc<AtomicBool>) {
            let flag = Arc::new(AtomicBool::new(false));
            let writer = Self {
                iterations_threshold: 10,
                generated_flag: Arc::clone(&flag), // Clone the Arc for the writer
            };
            (writer, flag) // Return the writer and the original Arc for testing
        }
    }

    struct Client {
        stream: TcpStream,
    }

    impl Client {
        fn new(server_host: &str, server_port: u16) -> Self {
            let stream = TcpStream::connect(format!("{}:{}", server_host, server_port))
                .expect("Failed to connect to server");
            Self { stream }
        }
    }

    #[tokio::test]
    async fn test_generate_report() {
        let (report_writer_mock, generated_flag) = MockReportWriter::new();
        let test_server_port = find_available_port();

        let server = Server::new("127.0.0.1".to_string(), test_server_port);
        let report_writer_mock_clone = report_writer_mock.clone();
        let server_handle = tokio::spawn(async move {
            server.init_connection(report_writer_mock).await;
        });

        // Give server time to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Use a timeout for the entire test
        let test_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut client = Client::new("127.0.0.1", test_server_port);
            let app_data_for_json = AppData::new();

            for i in 0..report_writer_mock_clone.iterations_threshold {
                let mut exec_data_to_send = app_data_for_json.exec_data.clone();
                exec_data_to_send.name = format!("test_exec_{}", i);

                let json_data = serde_json::to_string(&exec_data_to_send)?;
                client.stream.write_all(json_data.as_bytes())?;
                client.stream.flush()?;

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            // Wait for processing
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await;

        server_handle.abort(); // Clean up server

        assert!(test_result.is_ok(), "Test timed out");
        assert!(
            generated_flag.load(Ordering::SeqCst),
            "Report was not generated"
        );
    }

    // Helper function to find available port
    fn find_available_port() -> u16 {
        use std::net::TcpListener;
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
}
