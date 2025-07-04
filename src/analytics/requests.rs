use crate::analytics::reports::REPORTS_PATH;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::time::Instant;

use crate::system::manager::{SystemCapturer, SystemManager};

pub trait DataComparator: DeserializeOwned {
    type Diff;
    fn compare(&self, other: &Self) -> Self::Diff;

    /// Compare two files and return a Result wrapping an [`ExecAggDiff`] containing the differences between them
    fn compare_files(&self, filename1: &str, filename2: &str) -> std::io::Result<Self::Diff> {
        // Ensures the extension
        let filename1 = ensure_extension(filename1, "json");
        let filename2 = ensure_extension(filename2, "json");

        let mut file1 = std::fs::File::open(filename1)?;
        let mut file2 = std::fs::File::open(filename2)?;

        let mut str1 = String::new();
        let mut str2 = String::new();

        file1.read_to_string(&mut str1)?;
        file2.read_to_string(&mut str2)?;

        let data1 = serde_json::from_str::<Self>(&str1)?;
        let data2 = serde_json::from_str::<Self>(&str2)?;

        Ok(data1.compare(&data2))
    }
}

/// Default implementation for [`Instant`]
fn instant() -> Instant {
    Instant::now()
}

/// Store a unit of execution bench
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ExecData {
    pub name: String,
    pub module: Option<String>,
    pub timestamp: i64,
    #[serde(skip, default = "instant")]
    pub init_time: Instant,
    pub system_manager: SystemManager,
    pub exec_time: f32,
}

impl Default for ExecData {
    fn default() -> Self {
        Self {
            name: "".into(),
            module: None,
            timestamp: chrono::Local::now().timestamp(),
            init_time: Instant::now(),
            system_manager: SystemManager::default(),
            exec_time: 0.0,
        }
    }
}

impl PartialEq for ExecData {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl ExecData {
    pub fn from_system_data(name: &str) -> Self {
        Self {
            name: name.into(),
            module: None,
            timestamp: chrono::Local::now().timestamp(),
            init_time: Instant::now(),
            system_manager: SystemManager::default(),
            exec_time: 0.0,
        }
    }
}

impl DataComparator for ExecData {
    type Diff = Self;
    fn compare(&self, other: &Self) -> Self::Diff {
        let name = if self.name != other.name {
            format!("{}-{}", self.name, other.name)
        } else {
            self.name.clone()
        };

        Self {
            name,
            module: self.module.clone(),
            timestamp: self.timestamp,
            init_time: self.init_time,
            system_manager: self.system_manager.compare(&other.system_manager),
            exec_time: other.exec_time - self.exec_time,
        }
    }
}

impl ExecData {
    /// Get the path for the tag. If a path is provided, return the path itself.
    /// If only a name is given, return the default path with the tag
    pub fn get_tag_path(tag: &str) -> String {
        // TODO: Make this dynamic for Windows integration.
        if tag.contains('/') {
            ensure_extension(tag, "json")
        } else {
            ensure_extension(&format!("{REPORTS_PATH}/{tag}"), "json")
        }
    }

    pub fn capture_elapsed_time(&mut self) {
        self.exec_time = self.init_time.elapsed().as_secs_f32();
    }

    pub fn print_pretty(&self, width: usize) {
        let top_bottom = "=".repeat(width);

        println!("{top_bottom}");
        let fields = vec![
            ("Name", self.name.to_owned()),
            ("Module", self.module.clone().unwrap_or("[N/A]".into())),
            ("Timestamp", self.timestamp.to_string()),
            ("Memory Usage", format!("{:.2}", self.system_manager.memory_usage)),
            ("CPU Usage (%)", format!("{:.2}", self.system_manager.cpu_usage)),
            ("Exec Time (Sec)", format!("{:.2}", self.exec_time)),
        ];

        // Determine the maximum length of the field names
        let max_field_name_len = fields.iter().map(|(name, _value)| name.len()).max().unwrap_or(0);

        // Print each field
        for (field_name, field_value) in fields {
            // Calculate padding for the field name
            let name_padding = " ".repeat(max_field_name_len - field_name.len());

            // Calculate available space for the value
            // We'll use 4 spaces for " | " and then subtract for the name and its padding
            let available_value_width = width
                .saturating_sub(max_field_name_len)
                .saturating_sub(name_padding.len())
                .saturating_sub(4); // " | "

            // Truncate or pad the value as needed
            let mut display_value = field_value;
            if display_value.len() > available_value_width {
                // Truncate and add "..."
                display_value.truncate(available_value_width.saturating_sub(3));
                display_value.push_str("...");
            }
            let value_padding = " ".repeat(available_value_width.saturating_sub(display_value.len()));

            println!("{field_name}{name_padding} | {display_value}{value_padding}");
        }

        println!("{top_bottom}");
    }
}

/// Store multiple executions grouped by name
#[derive(Deserialize, Serialize, Debug, Default, Clone)]
pub struct AggData {
    pub name: String,
    pub total_execs: i32,
    pub total_exec_time: f32,
    pub total_memory_usage: f32,
    pub avg_exec_memory: f32,
    pub avg_exec_time: f32,
}

impl AggData {
    /// Update the aggregate data with the new execution data
    pub async fn update(&mut self, exec_data: &ExecData) {
        let safe_name = to_safe_name(&exec_data.name);
        let total_memory_usage = self.total_memory_usage + exec_data.system_manager.memory_usage;
        let total_exec_time = self.total_exec_time + exec_data.exec_time;
        let total_execs = self.total_execs + 1;

        self.name = safe_name;
        self.total_execs = total_execs;
        self.total_memory_usage = total_memory_usage;
        self.total_exec_time = total_exec_time;
        self.avg_exec_time = total_exec_time / total_execs as f32;
        self.avg_exec_memory = total_memory_usage / total_execs as f32;
    }

    /// Compare two aggregates and return a new one with the differences
    pub fn difference(&self, other: &Self) -> Self {
        let safe_name = to_safe_name(&self.name);

        let total_execs = other.total_execs - self.total_execs;
        let total_memory_usage = other.total_memory_usage - self.total_memory_usage;
        let total_exec_time = other.total_exec_time - self.total_exec_time;
        let avg_exec_time = other.avg_exec_time - self.avg_exec_time;
        let avg_exec_memory = other.avg_exec_memory - self.avg_exec_memory;

        Self {
            name: safe_name,
            total_execs,
            total_memory_usage,
            total_exec_time,
            avg_exec_time,
            avg_exec_memory,
        }
    }
}

/// A map containing all the aggregated data by name
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ExecAgg {
    pub agg_data: HashMap<String, AggData>,
}

impl DataComparator for ExecAgg {
    type Diff = ExecAggDiff;
    /// Compare two maps and return an [`ExecAggDiff`] representing the difference.
    fn compare(&self, other: &Self) -> Self::Diff {
        let mut exec_agg = ExecAgg::default();
        let mut missed: Vec<String> = vec![];
        let mut exclusive: Vec<String> = vec![];
        let mut reviewed: Vec<&str> = vec![];

        for (name, agg_data) in &self.agg_data {
            if let Some(other_data) = other.agg_data.get(name) {
                exec_agg
                    .agg_data
                    .insert(name.to_string(), agg_data.difference(other_data));
            } else {
                exclusive.push(name.to_string());
            }

            reviewed.push(name);
        }

        let mut other_names = other.agg_data.keys();
        while reviewed.len() - exclusive.len() < other.agg_data.len() {
            if let Some(name) = other_names.next() {
                if !reviewed.contains(&name.as_str()) {
                    missed.push(name.to_string());
                    reviewed.push(name);
                }
            }
        }

        ExecAggDiff {
            exec_agg,
            exclusive,
            missed,
        }
    }
}

/// Represnts the difference between two [`ExecAgg`]
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct ExecAggDiff {
    /// The difference between both data sets.
    pub exec_agg: ExecAgg,
    /// The apps that only has the first [`ExecAgg`] instance
    pub exclusive: Vec<String>,
    /// The apps missing for the first [`ExecAgg`]
    pub missed: Vec<String>,
}

/// Ensure the file has the expected extension; if not, add it.
fn ensure_extension(filename: &str, extension: &str) -> String {
    if filename.ends_with(extension) {
        filename.to_string()
    } else {
        format!("{filename}.{extension}")
    }
}

/// Transform the app name to a safe format for storage
pub fn to_safe_name(name: &str) -> String {
    name.replace("/", "-")
        .replace("?", "x")
        .replace("\\", "_")
        .replace(":", "_")
        .replace("*", "_")
        .replace("\"", "_")
        .replace("<", "_")
        .replace(">", "_")
        .replace("|", "_")
        .replace(" ", "_")
}

#[cfg(test)]
mod tests {
    use crate::analytics::reports::{ReportWriter, ServerWriter};

    use super::*;

    fn generate_diff_data() -> (ExecAgg, ExecAgg) {
        let mut data1 = ExecAgg::default();
        let mut data2 = ExecAgg::default();

        let mut agg1 = AggData::default();
        let mut agg2 = AggData::default();

        agg1.total_exec_time = 100.0;
        agg2.total_exec_time = 200.0;

        agg1.total_memory_usage = 200.0;
        agg2.total_memory_usage = 100.0;

        data1.agg_data.insert("test_1".to_string(), agg1.clone());
        data2.agg_data.insert("test_1".to_string(), agg2.clone());

        // An exclusive data is inserted
        data1.agg_data.insert("exclusive_data".to_string(), agg2);
        // An missed data is inserted to "other"
        data2.agg_data.insert("missed_data".to_string(), agg1);

        (data1, data2)
    }

    #[tokio::test]
    async fn test_update() {
        let mut agg = AggData::default();
        let mut exec_data = ExecData::default();

        exec_data.name = "test".to_string();
        exec_data.exec_time = 10.0;
        exec_data.system_manager.memory_usage = 20.0;

        agg.name = "test".to_string();
        agg.total_exec_time = 100.0;
        agg.total_memory_usage = 200.0;

        agg.update(&exec_data).await;

        assert_eq!(agg.name, "test");
        assert_eq!(agg.total_memory_usage, 220.0);
        assert_eq!(agg.total_exec_time, 110.0);
    }

    #[test]
    fn test_compare() {
        let (data1, data2) = generate_diff_data();

        let diff = data1.compare(&data2);

        let app_data = diff.exec_agg.agg_data.get("test_1").unwrap();

        assert_eq!(app_data.total_exec_time, 100.0);
        assert_eq!(app_data.total_memory_usage, -100.0);

        assert!(diff.exclusive.contains(&"exclusive_data".to_string()));
        assert!(diff.missed.contains(&"missed_data".to_string()));
    }

    #[test]
    fn test_difference() {
        let mut agg1 = AggData::default();
        let mut agg2 = AggData::default();

        agg1.total_exec_time = 100.0;
        agg2.total_exec_time = 200.0;

        agg1.total_memory_usage = 200.0;
        agg2.total_memory_usage = 100.0;

        let diff = agg1.difference(&agg2);

        assert_eq!(diff.total_exec_time, 100.0);
        assert_eq!(diff.total_memory_usage, -100.0);
    }

    #[test]
    fn test_ensure_extension() {
        let filename = "somefile";
        let with_extension = ensure_extension(filename, "json");

        assert_eq!(format!("{filename}.json"), with_extension);
    }

    #[test]
    fn test_compare_files() {
        let report_writer = ServerWriter::new();

        let temp_dir = std::env::temp_dir();
        let path1 = temp_dir.join("test_file1.json");
        let path2 = temp_dir.join("test_file2.json");

        let (data1, data2) = generate_diff_data();

        report_writer
            .generate_report(&data1, Some(path1.to_str().unwrap()))
            .unwrap();
        report_writer
            .generate_report(&data2, Some(path2.to_str().unwrap()))
            .unwrap();

        let diff = data1
            .compare_files(path1.to_str().unwrap(), path2.to_str().unwrap())
            .unwrap();

        let app_data = diff.exec_agg.agg_data.get("test_1").unwrap();

        assert_eq!(app_data.total_exec_time, 100.0);
        assert_eq!(app_data.total_memory_usage, -100.0);

        assert!(diff.exclusive.contains(&"exclusive_data".to_string()));
        assert!(diff.missed.contains(&"missed_data".to_string()));
    }

    #[test]
    fn test_exec_data_compare() {
        let mut data1 = ExecData::default();
        let mut data2 = ExecData::default();

        data1.system_manager.memory_usage = 10.0;
        data2.system_manager.memory_usage = 20.0;

        data1.system_manager.cpu_usage = 50.0;
        data2.system_manager.cpu_usage = 80.0;

        let expected_time = chrono::DateTime::parse_from_str("2025-05-01 11:00 +0000", "%Y-%m-%d %H:%M %z")
            .unwrap()
            .timestamp();

        data1.timestamp = expected_time;
        data2.timestamp = chrono::DateTime::parse_from_str("2025-05-02 9:00 +0000", "%Y-%m-%d %H:%M %z")
            .unwrap()
            .timestamp();

        let diff = data1.compare(&data2);

        assert_eq!(diff.system_manager.memory_usage, 10.0);
        assert_eq!(diff.system_manager.cpu_usage, 30.0);
        assert_eq!(diff.timestamp, expected_time);
    }
}
