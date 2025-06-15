use serde::{Deserialize, Serialize};
use std::{collections::HashMap, hash::Hash};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default)]
pub enum RequestType {
    HttpRequest,
    #[default]
    Function,
    CodeBlock,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ExecData {
    pub exec_type: RequestType,
    pub name: String,
    pub module: String,
    pub timestamp: f32,
    pub exec_memory_usage: f32,
    pub total_memory_global: f32,
    pub exec_time: f32,
}

impl Hash for ExecData {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq for ExecData {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ExecData {}

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
    pub async fn update(exec_data: &ExecData, exec_agg: Arc<RwLock<ExecAgg>>) -> Self {
        let name = exec_data.name.clone();
        let safe_name = name
            .replace("/", "-")
            .replace("?", "x")
            .replace("\\", "_")
            .replace(":", "_")
            .replace("*", "_")
            .replace("\"", "_")
            .replace("<", "_")
            .replace(">", "_")
            .replace("|", "_")
            .replace(" ", "_");

        let prev_data = {
            let agg = exec_agg.read().await;
            agg.exec_data.get(&name).cloned()
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
            total_execs: total_execs,
            total_memory_usage: total_memory_usage,
            total_exec_time: total_exec_time,
            avg_exec_time: total_exec_time / total_execs as f32,
            avg_exec_memory: total_memory_usage / total_execs as f32,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ExecAgg {
    pub exec_data: HashMap<String, AggData>,
}

impl Serialize for ExecAgg {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // Create a map serializer with the number of items in exec_data
        let mut map = serializer.serialize_map(Some(self.exec_data.len()))?;

        // Iterate through the hashmap entries
        for (name, agg_data) in &self.exec_data {
            // Use the name as the key and the agg_data as the value
            map.serialize_entry(name, agg_data)?;
        }

        // Finalize and return the map
        map.end()
    }
}
