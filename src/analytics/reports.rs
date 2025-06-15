use super::requests::{ExecAgg, ExecData};
use serde::Serialize;
use std::fs::{self, File};
use std::io::Write;
use tracing::info;

pub trait Reportable: Serialize {
    fn default_path(&self) -> String;

    fn generate_report(&self, path: Option<&str>) -> std::io::Result<()>
    {
        if !fs::exists("reports")? {
            fs::create_dir("reports")?;
        }
        let default_path = &self.default_path();

        let report_path = path.unwrap_or(default_path);
        let mut file = File::create(report_path)?;

        let app_data = serde_json::to_string(&self)?;

        file.write_all(app_data.as_bytes())?;
        info!(%report_path, "Aggregate report written");

        Ok(())
    }
}

impl Reportable for ExecAgg {
    fn default_path(&self) -> String {
        "reports/report_agg.json".into()
    }
}

impl Reportable for Vec<ExecData> {
    fn default_path(&self) -> String {
         "reports/report_apps.json".into()
    }
}

