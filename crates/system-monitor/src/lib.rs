//! System resource monitoring for termide.
//!
//! Provides CPU and memory usage information.

use std::sync::{Arc, Mutex};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// System resource statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemStats {
    /// CPU usage percentage (0-100).
    pub cpu_usage: f32,
    /// Memory usage in bytes.
    pub memory_used: u64,
    /// Total memory in bytes.
    pub memory_total: u64,
}

impl SystemStats {
    /// Get memory usage as percentage.
    pub fn memory_percent(&self) -> f32 {
        if self.memory_total == 0 {
            0.0
        } else {
            (self.memory_used as f32 / self.memory_total as f32) * 100.0
        }
    }
}

/// RAM unit for formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamUnit {
    Gigabytes,
    Megabytes,
}

/// System monitor for tracking resource usage.
#[derive(Debug)]
pub struct SystemMonitor {
    system: Arc<Mutex<System>>,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMonitor {
    /// Create a new system monitor.
    pub fn new() -> Self {
        let refresh_kind = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram());

        let mut system = System::new_with_specifics(refresh_kind);
        system.refresh_specifics(refresh_kind);

        Self {
            system: Arc::new(Mutex::new(system)),
        }
    }

    /// Refresh system information.
    pub fn refresh(&self) {
        if let Ok(mut sys) = self.system.lock() {
            let refresh_kind = RefreshKind::new()
                .with_cpu(CpuRefreshKind::new().with_cpu_usage())
                .with_memory(MemoryRefreshKind::new().with_ram());
            sys.refresh_specifics(refresh_kind);
        }
    }

    /// Alias for refresh() - backward compatibility.
    #[inline]
    pub fn update(&mut self) {
        self.refresh();
    }

    /// Get current system stats.
    pub fn stats(&self) -> SystemStats {
        if let Ok(sys) = self.system.lock() {
            SystemStats {
                cpu_usage: sys.global_cpu_usage(),
                memory_used: sys.used_memory(),
                memory_total: sys.total_memory(),
            }
        } else {
            SystemStats::default()
        }
    }

    /// Get CPU usage as integer percentage (0-100).
    pub fn cpu_usage(&self) -> u8 {
        if let Ok(sys) = self.system.lock() {
            sys.global_cpu_usage().round() as u8
        } else {
            0
        }
    }

    /// Get CPU usage as float percentage.
    pub fn cpu_usage_float(&self) -> f32 {
        self.stats().cpu_usage
    }

    /// Get memory usage percentage.
    pub fn memory_percent(&self) -> f32 {
        self.stats().memory_percent()
    }

    /// Get RAM info: (used_gb, total_gb).
    pub fn ram_info_gb(&self) -> (u64, u64) {
        if let Ok(sys) = self.system.lock() {
            let used = (sys.used_memory() as f64 / 1_073_741_824.0).round() as u64;
            let total = (sys.total_memory() as f64 / 1_073_741_824.0).round() as u64;
            (used, total)
        } else {
            (0, 0)
        }
    }

    /// Get RAM info: (used_mb, total_mb).
    pub fn ram_info_mb(&self) -> (u64, u64) {
        if let Ok(sys) = self.system.lock() {
            let used = (sys.used_memory() as f64 / 1_048_576.0).round() as u64;
            let total = (sys.total_memory() as f64 / 1_048_576.0).round() as u64;
            (used, total)
        } else {
            (0, 0)
        }
    }

    /// Get RAM usage as integer percentage (0-100).
    pub fn ram_usage_percent(&self) -> u8 {
        if let Ok(sys) = self.system.lock() {
            let used = sys.used_memory();
            let total = sys.total_memory();
            if total > 0 {
                ((used as f64 / total as f64) * 100.0).round() as u8
            } else {
                0
            }
        } else {
            0
        }
    }

    /// Format RAM info with automatic unit selection.
    pub fn format_ram(&self) -> (String, RamUnit) {
        let (used_gb, total_gb) = self.ram_info_gb();
        if total_gb >= 1 {
            (format!("{}/{}", used_gb, total_gb), RamUnit::Gigabytes)
        } else {
            let (used_mb, total_mb) = self.ram_info_mb();
            (format!("{}/{}", used_mb, total_mb), RamUnit::Megabytes)
        }
    }
}

/// Disk space information.
#[derive(Clone, Debug)]
pub struct DiskSpaceInfo {
    /// Device name (e.g., "NVME0N1", "SDA1").
    pub device: Option<String>,
    /// Available space in bytes.
    pub available: u64,
    /// Total space in bytes.
    pub total: u64,
}

impl DiskSpaceInfo {
    /// Get disk usage percentage (0-100).
    pub fn usage_percent(&self) -> u8 {
        if self.total > 0 {
            let used = self.total.saturating_sub(self.available);
            ((used * 100) / self.total).min(100) as u8
        } else {
            0
        }
    }

    /// Get used space in bytes.
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Get used space in GB.
    pub fn used_gb(&self) -> u64 {
        (self.used() as f64 / 1_073_741_824.0).round() as u64
    }

    /// Get total space in GB.
    pub fn total_gb(&self) -> u64 {
        (self.total as f64 / 1_073_741_824.0).round() as u64
    }

    /// Get device name (extracted from path).
    pub fn device_name(&self) -> Option<String> {
        self.device
            .as_ref()
            .map(|d| d.strip_prefix("/dev/").unwrap_or(d).to_uppercase())
    }
}

/// Format bytes as human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Extension trait for DiskSpaceInfo with i18n support.
pub trait DiskSpaceInfoExt {
    /// Format disk space with device name and usage.
    fn format_space(&self) -> String;
}

impl DiskSpaceInfoExt for DiskSpaceInfo {
    fn format_space(&self) -> String {
        let t = termide_i18n::t();

        // Calculate used space and percentage
        let used = self.total.saturating_sub(self.available);
        let percent = if self.total > 0 {
            ((used * 100) / self.total).min(100)
        } else {
            0
        };

        // Convert to GB (rounded to nearest integer)
        let used_gb = (used as f64 / 1_073_741_824.0).round() as u64;
        let total_gb = (self.total as f64 / 1_073_741_824.0).round() as u64;

        if let Some(device) = &self.device {
            // Extract device name from path like "/dev/nvme0n1p2" -> "NVME0N1P2"
            let device_name = device
                .strip_prefix("/dev/")
                .unwrap_or(device)
                .to_uppercase();
            format!(
                "{}: {}/{} {} ({}%)",
                device_name,
                used_gb,
                total_gb,
                t.size_gigabytes(),
                percent
            )
        } else {
            format!(
                "{}/{} {} ({}%)",
                used_gb,
                total_gb,
                t.size_gigabytes(),
                percent
            )
        }
    }
}
