use sysinfo::System;

pub trait MemoryCapturer {
    fn capture(&mut self);
}

/// Manage the resources of the system, like memory or CPU
pub struct SystemManager {
    /// Memory usage in MB
    memory_usage: f32,
    /// CPU % usage
    cpu_usage: f32,
}

impl MemoryCapturer for SystemManager {
    fn capture(&mut self) {
        let sys = System::new_all();

        self.memory_usage = sys.used_memory() as f32 / 1024.0;
        self.cpu_usage = sys.global_cpu_usage();
    }
}
