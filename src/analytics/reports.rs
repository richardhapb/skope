use super::requests::{ExecAgg, ExecData};
use std::fmt::Debug;
use std::fs::{self, File};
use std::io::Write;
use std::ops::Deref;
use tracing::info;

pub const REPORTS_PATH: &str = ".skope";

/// A reportable data
pub trait Reportable: Sync + Send {
    fn default_path(&self) -> String;
    fn report_data(&self) -> Result<String, serde_json::Error>;
}

impl Reportable for ExecAgg {
    fn default_path(&self) -> String {
        format!("{}/report_agg.json",REPORTS_PATH)
    }

    fn report_data(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Reportable for ExecData {
    fn default_path(&self) -> String {
        format!("{}/{}.json", REPORTS_PATH, self.name)
    }

    fn report_data(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Reportable for Vec<ExecData> {
    fn default_path(&self) -> String {
        format!("{}/report_apps.json", REPORTS_PATH)
    }

    fn report_data(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Generate json reports for the [`Reportable`] data
pub trait ReportWriter: Send + Sync + Debug {
    fn write_reports(&self, reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()>;

    fn generate_report(
        &self,
        reportable: &dyn Reportable,
        path: Option<&str>,
    ) -> std::io::Result<()> {
        if !fs::metadata(REPORTS_PATH).is_ok() {
            fs::create_dir(REPORTS_PATH)?;
        }
        let default_path = &reportable.default_path();
        let report_path = path.unwrap_or(default_path);
        let mut file = File::create(report_path)?;

        let app_data = reportable
            .report_data()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        file.write_all(app_data.as_bytes())?;
        info!(%report_path, "Report written");

        Ok(())
    }
}

#[derive(Copy, Clone, Debug)]
#[allow(dead_code)]
pub struct ServerWriter {
    iterations_threshold: usize,
}

impl ReportWriter for ServerWriter {
    fn write_reports(&self, reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()> {
        for reportable in reportables {
            self.generate_report(reportable.deref(), None)?;
        }

        Ok(())
    }
}

impl ServerWriter {
    pub fn new() -> Self {
        Self {
            iterations_threshold: 10,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RunnerWriter {
    report_name: String,
}

impl ReportWriter for RunnerWriter {
    fn write_reports(&self, reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()> {
        for reportable in reportables {
            self.generate_report(reportable.deref(), Some(&self.report_name))?;
        }

        Ok(())
    }
}

impl RunnerWriter {
    pub fn new(name: &str) -> Self {
        Self { report_name: name.to_string() }
    }
}
