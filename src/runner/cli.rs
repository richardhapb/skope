use std::error::Error;

pub trait Executable {
    fn get_location(&self) -> &str;
}

pub struct BashScript {
    path: String,
}

impl BashScript {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }
}

impl Executable for BashScript {
    fn get_location(&self) -> &str {
        &self.path
    }
}

pub trait ScriptExecutor {
    async fn execute(executable: impl Executable) -> Result<(), Box<dyn Error>>;
}

pub struct BashExecutor;

impl ScriptExecutor for BashExecutor {
    async fn execute(executable: impl Executable) -> Result<(), Box<dyn Error>> {
        let location = executable.get_location();
        let content = tokio::fs::read_to_string(location).await?;

        tokio::spawn(async move {
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&content)
                .output()
                .await
                .unwrap_or_else(|_| panic!("Cannot execute the command"));
        });

        Ok(())
    }
}
