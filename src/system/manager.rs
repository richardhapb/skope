use serde::{Deserialize, Serialize};
use sysinfo::System;

/// Handle the data for system resources
pub trait SystemCapturer {
    fn capture(&mut self);
    fn compare(&self, other: &Self) -> Self;
}

/// Manage the resources of the system, like memory or CPU
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SystemManager {
    /// Memory usage in MB
    memory_usage: f32,
    /// CPU % usage
    cpu_usage: f32,
}

impl Default for SystemManager {
    fn default() -> Self {
        let mut instance = Self {
            memory_usage: 0.0,
            cpu_usage: 0.0,
        };
        instance.capture();
        instance
    }
}

impl SystemCapturer for SystemManager {
    /// Capture the system status and assign it to the struct instance
    fn capture(&mut self) {
        let sys = System::new_all();

        self.memory_usage = sys.used_memory() as f32 / 1024.0;
        self.cpu_usage = sys.global_cpu_usage();
    }

    /// Compare the current system status with the initial,
    /// return a new [`SystemManager`] with the difference
    fn compare(&self, other: &Self) -> Self {
        Self {
            memory_usage: other.memory_usage - self.memory_usage,
            cpu_usage: other.cpu_usage - self.cpu_usage,
        }
    }
}
