use std::error::Error;
use tokio::{process::Command, task::JoinHandle};
use tracing::{error, info, warn};

pub trait Executable {
    fn get_location(&self) -> &str;
}

pub struct BashScript {
    pub path: String,
}

impl BashScript {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

impl Executable for BashScript {
    fn get_location(&self) -> &str {
        &self.path
    }
}

pub trait ScriptExecutor {
    // The `execute` method will return `Ok(())` if the task is successfully spawned.
    // Errors from the script's execution will be logged internally but not propagated
    // via this return value.
    async fn execute(executable: impl Executable + Send + Sync + 'static) -> Result<JoinHandle<()>, Box<dyn Error>>;
}

pub struct BashExecutor;

impl ScriptExecutor for BashExecutor {
    async fn execute(executable: impl Executable + Send + Sync + 'static) -> Result<JoinHandle<()>, Box<dyn Error>> {
        let location = executable.get_location();

        let content = tokio::fs::read_to_string(location).await?;

        // Spawn a new asynchronous task that will execute the bash command.
        // This task will run independently in the background.
        Ok(tokio::spawn(async move {
            let output = Command::new("bash").arg("-c").arg(&content).output().await; // Await the script's completion within this spawned task

            match output {
                Ok(output) => {
                    if output.status.success() {
                        info!("Script finished successfully.");
                        if !output.stdout.is_empty() {
                            info!("Script stdout:\n{}", String::from_utf8_lossy(&output.stdout));
                        }
                        if !output.stderr.is_empty() {
                            warn!("Script stderr:\n{}", String::from_utf8_lossy(&output.stderr));
                        }
                    } else {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        error!(
                            "Script execution failed with status: {:?}\nStdout: {}\nStderr: {}",
                            output.status, stdout, stderr
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to launch bash command: {}", e);
                }
            }
        }))
    }
}
