use crate::analytics::reports::Reportable;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// Store a unit of execution bench
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ExecData {
    pub name: String,
    pub module: Option<String>,
    pub timestamp: i64,
    pub exec_memory_usage: f32,
    pub system_manager: SystemManager,
    pub exec_time: f32,
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
            exec_memory_usage: 0.0,
            system_manager: SystemManager::default(),
            exec_time: 0.0,
        }
    }
}

impl DataComparator for ExecData {
    type Diff = Self;
    fn compare(&self, other: &Self) -> Self::Diff {
        Self {
            name: self.name.clone(),
            module: self.module.clone(),
            timestamp: other.timestamp,
            exec_memory_usage: other.exec_memory_usage - self.exec_memory_usage,
            system_manager: self.system_manager.compare(&other.system_manager),
            exec_time: other.exec_time - self.exec_time,
        }
    }
}

impl ExecData {
    pub fn generate_start_path(&self) -> String {
        format!("{}_start.json", self.default_path())
    }

    pub fn generate_stop_path(&self) -> String {
        format!("{}_stop.json", self.default_path())
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
    pub async fn update(exec_data: &ExecData, exec_agg: Arc<RwLock<ExecAgg>>) -> Self {
        let safe_name = to_safe_name(&exec_data.name);

        let prev_data = {
            let agg = exec_agg.read().await;
            agg.agg_data.get(&exec_data.name).cloned()
        };

        let mut total_exec_time = exec_data.exec_time;
        let mut total_memory_usage = exec_data.exec_memory_usage;
        let mut total_execs = 1; // current

        // If exists data, use it
        if let Some(prev_data) = prev_data {
            total_exec_time = prev_data.total_exec_time + exec_data.exec_time;
            total_memory_usage = prev_data.total_memory_usage + exec_data.exec_memory_usage;
            total_execs = prev_data.total_execs + 1;
        }

        Self {
            name: safe_name,
            total_execs,
            total_memory_usage,
            total_exec_time,
            avg_exec_time: total_exec_time / total_execs as f32,
            avg_exec_memory: total_memory_usage / total_execs as f32,
        }
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
        format!("{}.{}", filename, extension)
    }
}

/// Transform the app name to a safe format for storage
fn to_safe_name(name: &str) -> String {
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
        let mut agg1 = AggData::default();

        let mut exec_data = ExecData::default();
        let mut exec_agg = ExecAgg::default();

        exec_data.name = "test".to_string();
        exec_data.exec_time = 10.0;
        exec_data.exec_memory_usage = 20.0;

        agg1.name = "test".to_string();
        agg1.total_exec_time = 100.0;
        agg1.total_memory_usage = 200.0;

        exec_agg.agg_data.insert("test".to_string(), agg1);

        let result = AggData::update(&exec_data, Arc::new(RwLock::new(exec_agg))).await;

        assert_eq!(result.name, "test");
        assert_eq!(result.total_memory_usage, 220.0);
        assert_eq!(result.total_exec_time, 110.0);
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

        assert_eq!(format!("{}.json", filename), with_extension);
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
}
