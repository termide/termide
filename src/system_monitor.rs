use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// System resource monitor for CPU and RAM usage
#[derive(Debug)]
pub struct SystemMonitor {
    system: System,
}

impl SystemMonitor {
    /// Create new system monitor
    pub fn new() -> Self {
        // Configure what to refresh
        let refresh_kind = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram());

        let mut system = System::new_with_specifics(refresh_kind);

        // Initial refresh to populate data
        system.refresh_specifics(refresh_kind);

        Self { system }
    }

    /// Update system statistics
    /// Should be called periodically (e.g., every second)
    pub fn update(&mut self) {
        let refresh_kind = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram());

        self.system.refresh_specifics(refresh_kind);
    }

    /// Get CPU usage as integer percentage (0-100)
    pub fn cpu_usage(&self) -> u8 {
        self.system.global_cpu_usage().round() as u8
    }

    /// Get RAM info: (used_gb, total_gb)
    /// Returns values in gigabytes rounded to nearest integer
    pub fn ram_info_gb(&self) -> (u64, u64) {
        let used_bytes = self.system.used_memory();
        let total_bytes = self.system.total_memory();

        // Convert bytes to GB and round
        let used_gb = (used_bytes as f64 / 1_073_741_824.0).round() as u64;
        let total_gb = (total_bytes as f64 / 1_073_741_824.0).round() as u64;

        (used_gb, total_gb)
    }

    /// Get RAM info: (used_mb, total_mb)
    /// Returns values in megabytes rounded to nearest integer
    pub fn ram_info_mb(&self) -> (u64, u64) {
        let used_bytes = self.system.used_memory();
        let total_bytes = self.system.total_memory();

        // Convert bytes to MB and round
        let used_mb = (used_bytes as f64 / 1_048_576.0).round() as u64;
        let total_mb = (total_bytes as f64 / 1_048_576.0).round() as u64;

        (used_mb, total_mb)
    }

    /// Get RAM usage as integer percentage (0-100)
    pub fn ram_usage_percent(&self) -> u8 {
        let used = self.system.used_memory();
        let total = self.system.total_memory();
        if total > 0 {
            ((used as f64 / total as f64) * 100.0).round() as u8
        } else {
            0
        }
    }

    /// Format RAM info with automatic unit selection (GB/MB)
    /// Returns formatted string like "4/16" and unit like "GB" or "MB"
    pub fn format_ram(&self) -> (String, RamUnit) {
        let (used_gb, total_gb) = self.ram_info_gb();

        // Use GB if total memory is >= 1GB, otherwise use MB
        if total_gb >= 1 {
            (format!("{}/{}", used_gb, total_gb), RamUnit::Gigabytes)
        } else {
            let (used_mb, total_mb) = self.ram_info_mb();
            (format!("{}/{}", used_mb, total_mb), RamUnit::Megabytes)
        }
    }
}

/// RAM unit for formatting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamUnit {
    Gigabytes,
    Megabytes,
}

/// Disk space information
#[derive(Clone, Debug)]
pub struct DiskSpaceInfo {
    pub device: Option<String>, // Device name (e.g., "NVME0N1", "SDA1")
    pub available: u64,
    pub total: u64,
}

impl DiskSpaceInfo {
    /// Get disk usage percentage (0-100)
    /// Returns percentage of USED space (not available)
    pub fn usage_percent(&self) -> u8 {
        if self.total > 0 {
            let used = self.total.saturating_sub(self.available);
            ((used * 100) / self.total).min(100) as u8
        } else {
            0
        }
    }

    /// Format disk information: "NVME0N1P2 386/467Гб (83%)"
    pub fn format_space(&self) -> String {
        let t = crate::i18n::t();

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
                "{} {}/{}{} ({}%)",
                device_name,
                used_gb,
                total_gb,
                t.size_gigabytes(),
                percent
            )
        } else {
            // Fallback to old format if device name is not available
            format!(
                "{}/{}{} ({}%)",
                used_gb,
                total_gb,
                t.size_gigabytes(),
                percent
            )
        }
    }
}
