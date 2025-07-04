use super::data::DataProvider;

use tokio::net::TcpListener;
use tracing::{error, info};

/// The main server of the application
#[derive(Debug, Clone)]
pub struct Server<T: DataProvider + Default> {
    pub host: String,
    pub port: u16,
    pub data_provider: T,
}

impl<T: DataProvider + Default> Default for Server<T> {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9001,
            data_provider: T::default(),
        }
    }
}

impl<T: DataProvider + Default> Server<T> {
    pub fn new(host: String, port: u16, data_provider: T) -> Self {
        Self {
            host,
            port,
            data_provider,
        }
    }

    /// Handle the main loop that receives connections.
    pub async fn init_connection(self) {
        let binding = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&binding)
            .await
            .unwrap_or_else(|_| panic!("Error binding {binding}"));

        info!("Skope listening on {}", binding);
        self.data_provider.main_loop(listener).await.unwrap_or_else(|e| {
            error!(%e, "Error wstablishing the connection");
            eprintln!("Error: {e}");
            std::process::exit(1);
        })
    }
}
