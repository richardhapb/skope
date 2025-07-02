use crate::analytics::requests::DataComparator;
use crate::system::manager::SystemCapturer;

use super::reports::{ReportWriter, Reportable, RunnerWriter, ServerWriter};
use super::requests::{AggData, ExecAgg, ExecData, to_safe_name};

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

pub trait DataProvider {
    async fn main_loop(self, listener: TcpListener) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn handle_client(
        &mut self,
        socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    fn should_close_connection(&self) -> bool;

    async fn reponse_success(&self, socket: &mut TcpStream) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Responding to client with a successful response");

        let response = b"HTTP/1.1 200 OK
Content-Length: 0";

        socket.write_all(response).await?;
        socket.flush().await?;

        Ok(())
    }

    async fn reponse_bad_request(
        &self,
        socket: &mut TcpStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Responding to client with a bad request response");

        let response = b"HTTP/1.1 404 Bad Request
Content-Length: 0";

        socket.write_all(response).await?;
        socket.flush().await?;

        Ok(())
    }
}

/// Receive and process the data in Server mode
#[derive(Debug, Clone)]
pub struct ServerReceiver {
    pub exec_agg: Arc<RwLock<ExecAgg>>,
    pub exec_data: Arc<RwLock<Vec<ExecData>>>,
    pub report_writer: Arc<dyn ReportWriter>,
    iteration: Arc<AtomicUsize>,
    iterations_threshold: usize,
}

impl Default for ServerReceiver {
    fn default() -> Self {
        let report_writer = Arc::new(ServerWriter::new());
        Self {
            exec_agg: Arc::new(RwLock::new(ExecAgg::default())),
            exec_data: Arc::new(RwLock::new(vec![])),
            report_writer,
            iteration: Arc::new(AtomicUsize::new(0)),
            iterations_threshold: 10,
        }
    }
}

impl DataProvider for ServerReceiver {
    /// Handle the main loop that receives connections.
    async fn main_loop(self, listener: TcpListener) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let max_connections = Arc::new(tokio::sync::Semaphore::new(500)); // Limit to 500 concurrent connections

        tokio::spawn(async move {
            loop {
                // Wait for a connection slot to become available
                let permit = max_connections.clone().acquire_owned().await.unwrap();

                match listener.accept().await {
                    Ok((socket, addr)) => {
                        info!("Client connected: {}", addr);

                        let mut server_for_client_task = self.clone();

                        tokio::spawn(async move {
                            // The permit is dropped when this task completes
                            let _permit = permit;

                            if let Err(e) = server_for_client_task.handle_client(socket, addr).await {
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
        Ok(())
    }

    /// Handle a client connection with the provided data
    async fn handle_client(
        &mut self,
        mut socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Set a read timeout to prevent hanging connections
        socket.set_nodelay(true)?;

        let mut buf = vec![0u8; 1024];

        let iteration_inner = self.iteration.clone();

        // Use a timeout for read operations
        let idle_timeout = tokio::time::sleep(std::time::Duration::from_secs(30));
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

                                    let counter = iteration_inner.fetch_add(1, std::sync::atomic::Ordering::AcqRel);

                                    let should_generate_report =
                                    (counter + 1) % self.iterations_threshold == 0;

                                    let exec_data_clone = self.exec_data.read().await.clone();
                                    let exec_agg_clone = self.exec_agg.read().await.clone();

                                    if should_generate_report {
                                        println!("Executed");
                                        let reportables: Vec<Box<dyn Reportable>> =
                                        vec![Box::new(exec_data_clone), Box::new(exec_agg_clone)];

                                        let report_writer_inner = self.report_writer.clone();

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

    fn should_close_connection(&self) -> bool {
        todo!()
    }
}

impl ServerReceiver {
    pub fn new(report_writer: Arc<dyn ReportWriter + 'static>) -> Self {
        Self {
            report_writer,
            ..Default::default()
        }
    }

    /// Process the data provided by the client
    pub async fn process_data(&self, parsed: ExecData) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Try to get the previous data
        let mut agg_data = {
            if let Some(prev_data) = self.exec_agg.read().await.agg_data.get(&to_safe_name(&parsed.name)) {
                prev_data.clone()
            } else {
                AggData::default()
            }
        };
        agg_data.update(&parsed).await;

        // Update the exec aggregation with the new aggregation data
        {
            let mut agg = self.exec_agg.write().await;
            agg.agg_data.insert(parsed.name.clone(), agg_data);
            info!(
                "Updated exec_agg with new data point, total: {} items",
                agg.agg_data.len()
            );
        }

        // Store the raw execution data
        self.exec_data.write().await.push(parsed);

        Ok(())
    }
}

/// Handle the data from the runner's execution
pub struct RunnerReceiver {
    pub exec_data: ExecData,
    pub diff: Option<ExecData>,
    pub report_writer: Box<dyn ReportWriter>,
    pub should_close: Arc<AtomicBool>,
    pub capturing: Arc<AtomicBool>,
}

impl DataProvider for RunnerReceiver {
    async fn main_loop(mut self, listener: TcpListener) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            match listener.accept().await {
                Ok((socket, addr)) => {
                    info!("Client connected: {}", addr);

                    self.handle_client(socket, addr).await?;
                    if self.should_close_connection() {
                        // Calculate the difference and generate the final report
                        if let Some(diff) = self.diff {
                            self.report_writer.generate_report(&diff, None)?;
                            println!("Capture finished successfully, written in {}", diff.default_path());
                        }
                        break;
                    }
                }
                Err(e) => {
                    error!(%e, "error connecting");
                    break;
                }
            }
        }
        Ok(())
    }

    /// This is executed once, without a loop. The connection is closed immediately when a
    /// request is processed
    async fn handle_client(
        &mut self,
        mut socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut buf = vec![0u8; 1024];
        match socket.read(&mut buf).await {
            Ok(0) => {
                info!(%addr, "Closing the connection");
                return Ok(());
            }
            Ok(n) => {
                debug!(%n, "Captured request");
                if buf.starts_with(b"GET /start") {
                    println!("Start signal received, capturing...");
                    // Skip if it is capturing
                    if self.capturing.swap(true, Ordering::AcqRel) {
                        warn!("Attempting to start a capture that is already in progress");
                        println!("The capture has started; skipping the request.");
                    }

                    // First capture
                    self.exec_data.system_manager.capture();
                    self.reponse_success(&mut socket).await?;
                    return Ok(());
                }

                if buf.starts_with(b"GET /stop") {
                    println!("Stop signal received, stopping the capturing...");
                    // If not capturing, request to start the capturing
                    if !self.capturing.swap(false, Ordering::AcqRel) {
                        error!("Attempting to stop a capture without start it");
                        println!("No capture has started; please start one first.");
                    }

                    let before = self.exec_data.clone();

                    // Generate the report and close the connection
                    self.exec_data.capture_elapsed_time();
                    self.exec_data.system_manager.capture();
                    self.diff = Some(before.compare(&self.exec_data));

                    self.should_close.store(true, Ordering::Release);
                    self.reponse_success(&mut socket).await?;
                    return Ok(());
                }

                // Unknown request
                println!(
                    "Unknown request: {:?}",
                    String::from_utf8_lossy(&buf).split_once("\r\n").unwrap_or(("", "")).0
                );
                self.reponse_bad_request(&mut socket).await?;
            }
            Err(e) => {
                error!("Error reading request: {}", e);
                self.reponse_bad_request(&mut socket).await?;
                return Err(Box::new(e));
            }
        }
        Ok(())
    }

    fn should_close_connection(&self) -> bool {
        self.should_close.load(Ordering::Relaxed)
    }
}

impl Default for RunnerReceiver {
    fn default() -> Self {
        let report_writer = RunnerWriter::default();
        Self::new("", Box::new(report_writer))
    }
}

impl RunnerReceiver {
    /// Create a new [`RunnerReceiver`] with the current system state
    pub fn new(name: &str, report_writer: Box<dyn ReportWriter + 'static>) -> Self {
        Self {
            exec_data: ExecData::from_system_data(name),
            report_writer,
            diff: None,
            should_close: Arc::new(AtomicBool::new(false)),
            capturing: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::reports::ReportWriter;
    use crate::analytics::server::Server;
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

    #[derive(Clone, Debug)]
    struct TestReportWriter {
        generated_flag: Arc<AtomicBool>,
        reports_count: Arc<AtomicUsize>,
    }

    impl ReportWriter for TestReportWriter {
        fn write_reports(&self, _reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()> {
            self.generated_flag.store(true, Ordering::SeqCst);
            self.reports_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn generate_report(&self, _reportable: &dyn Reportable, _path: Option<&str>) -> std::io::Result<()> {
            self.generated_flag.store(true, Ordering::SeqCst);
            self.reports_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    impl TestReportWriter {
        fn new() -> (Self, Arc<AtomicBool>, Arc<AtomicUsize>) {
            let flag = Arc::new(AtomicBool::new(false));
            let count = Arc::new(AtomicUsize::new(0));
            let writer = Self {
                generated_flag: Arc::clone(&flag),
                reports_count: Arc::clone(&count),
            };
            (writer, flag, count)
        }
    }

    struct Client {
        stream: TcpStream,
    }

    impl Client {
        fn new(server_host: &str, server_port: u16) -> Self {
            let stream =
                TcpStream::connect(format!("{}:{}", server_host, server_port)).expect("Failed to connect to server");
            Self { stream }
        }
    }

    // Test that the report generation threshold is working properly
    // for example, if the threshold is defined as 10, after 10 requests
    // the report should be generated.
    #[tokio::test]
    async fn test_generate_report() {
        let (report_writer_mock, generated_flag, reports_count) = TestReportWriter::new();
        let test_server_port = find_available_port();
        let report_writer = Arc::new(report_writer_mock);
        let data_provider = ServerReceiver::new(report_writer);

        let server: Server<ServerReceiver> = Server::new("127.0.0.1".to_string(), test_server_port, data_provider);
        let server_handle = tokio::spawn(async move {
            server.init_connection().await;
        });

        // Give server time to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Timeout for the entire test
        let test_result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut client = Client::new("127.0.0.1", test_server_port);
            let app_data_for_json = AppData::new();

            for i in 0..10 {
                let mut exec_data_to_send = app_data_for_json.exec_data.clone();
                exec_data_to_send.name = format!("test_exec_{}", i);

                let json_data = serde_json::to_string(&exec_data_to_send)?;
                client.stream.write_all(json_data.as_bytes())?;
                client.stream.flush()?;

                assert!(!generated_flag.load(Ordering::SeqCst), "Report generated early");

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            // Wait for processing
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await;

        server_handle.abort(); // Clean up server

        assert!(test_result.is_ok(), "Test timed out");
        assert_eq!(reports_count.load(Ordering::Acquire), 1);
        assert!(generated_flag.load(Ordering::SeqCst), "Report was not generated");
    }

    #[tokio::test]
    async fn test_process_data() {
        let test_server_port = find_available_port();
        let mut data = AppData::new();
        let (mock_writer, _, _) = TestReportWriter::new();
        let data_provider = ServerReceiver::new(Arc::new(mock_writer));
        let server = Server::new("127.0.0.1".to_string(), test_server_port, data_provider);

        // Expect to the server
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        data.exec_data.name = "test_name".to_string();
        data.exec_data.exec_time = 10.0;
        server.data_provider.process_data(data.exec_data.clone()).await.unwrap();

        // Should be inserted on aggregate
        assert_eq!(
            server
                .data_provider
                .exec_agg
                .read()
                .await
                .agg_data
                .get("test_name")
                .unwrap()
                .total_exec_time,
            data.exec_data.exec_time
        );

        server.data_provider.process_data(data.exec_data.clone()).await.unwrap();

        // Should be updated with the double of the value
        assert_eq!(
            server
                .data_provider
                .exec_agg
                .read()
                .await
                .agg_data
                .get("test_name")
                .unwrap()
                .total_exec_time,
            data.exec_data.exec_time * 2.0
        );
    }

    // The RunnerReceiver should:
    // - Generate a report when the start signal is received (HTTP request) and begin capturing.
    // - Stop capturing when the stop signal is received and generate the stop report.
    // - Finally, create the report using the stop report and the start report to generate the final report.
    #[tokio::test]
    async fn test_runner_connection() {
        let test_server_port = find_available_port();
        let (mock_writer, generated_flag, reports_count) = TestReportWriter::new();
        let data_provider = RunnerReceiver::new("test", Box::new(mock_writer));
        let capturing = data_provider.capturing.clone();
        let should_close = data_provider.should_close.clone();
        let server = Server::new("127.0.0.1".to_string(), test_server_port, data_provider);

        let server_handle = tokio::spawn(async move {
            server.init_connection().await;
        });

        // Expect to the server
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = Client::new("127.0.0.1", test_server_port);

        // Should not be capturing now
        assert!(!capturing.load(Ordering::Relaxed));

        let request = b"GET /start";
        client.stream.write_all(request).expect("error writing data");
        client.stream.flush().expect("error flushing");

        // Give server time to process the request
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Should BE capturing now
        assert!(capturing.load(Ordering::Relaxed), "Should be capturing after /start");
        assert!(
            !should_close.load(Ordering::Relaxed),
            "Should not be closing after /start"
        );
        assert!(!generated_flag.load(Ordering::SeqCst), "Report generated early");
        assert_eq!(reports_count.load(Ordering::Relaxed), 0);

        // The connection is closed and requires a new connection
        let mut client = Client::new("127.0.0.1", test_server_port);

        let request = b"GET /stop";
        client.stream.write_all(request).expect("error writing data");
        client.stream.flush().expect("error flushing");

        // Give server time to process the request and generate report
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Should NOT be capturing now (it was swapped to false)
        assert!(
            !capturing.load(Ordering::Relaxed),
            "Should not be capturing after /stop"
        );
        // Should close connection
        assert!(should_close.load(Ordering::Relaxed), "Should be closing after /stop");
        assert!(
            generated_flag.load(Ordering::SeqCst),
            "Report should have been generated"
        );
        assert_eq!(reports_count.load(Ordering::Relaxed), 1);

        server_handle.abort();
    }

    // Helper function to find available port
    fn find_available_port() -> u16 {
        use std::net::TcpListener;
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }
}
