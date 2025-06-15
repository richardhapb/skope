use super::requests::{ExecAgg, ExecData};
use std::fs::{self, File};
use std::io::Write;
use tracing::info;

pub trait Reportable: Sync + Send {
    fn default_path(&self) -> String;
    fn report_data(&self) -> Result<String, serde_json::Error>;

    fn generate_report(&self, path: Option<&str>) -> std::io::Result<()> {
        if !fs::metadata("reports").is_ok() {
            fs::create_dir("reports")?;
        }
        let default_path = &self.default_path();
        let report_path = path.unwrap_or(default_path);
        let mut file = File::create(report_path)?;

        let app_data = self
            .report_data()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        file.write_all(app_data.as_bytes())?;
        info!(%report_path, "Aggregate report written");

        Ok(())
    }
}

impl Reportable for ExecAgg {
    fn default_path(&self) -> String {
        "reports/report_agg.json".into()
    }

    fn report_data(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Reportable for Vec<ExecData> {
    fn default_path(&self) -> String {
        "reports/report_apps.json".into()
    }

    fn report_data(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

pub trait ReportWriter: Send + Sync + 'static {
    fn write_reports(&self, reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()>;
    fn get_iterations_threshold(&self) -> usize;
    #[allow(dead_code)]
    fn set_iterations_threshold(&mut self, iterations: usize);
}

#[derive(Copy, Clone)]
pub struct DefaultWriter {
    iterations_threshold: usize
}

impl ReportWriter for DefaultWriter {
    fn write_reports(&self, reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()> {
        for reportable in reportables {
            reportable.generate_report(None)?;
        }

        Ok(())
    }

    fn get_iterations_threshold(&self) -> usize {
        self.iterations_threshold
    }

    fn set_iterations_threshold(&mut self, iterations: usize) {
        self.iterations_threshold = iterations;
    }
}

impl DefaultWriter {
    pub fn new(iterations_threshold: usize) -> Self {
        Self {
           iterations_threshold 
        }
    }
}

