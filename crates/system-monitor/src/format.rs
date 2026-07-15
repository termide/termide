//! Human-readable formatting helpers for network speed, byte sizes, and disk space.

use crate::{DiskSpaceInfo, BYTES_PER_GB};

/// Format network speed as compact human-readable string.
///
/// Returns strings like "0B/s", "4kB/s", "1.2MB/s", "2.5GB/s".
pub fn format_net_speed(bytes_per_sec: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    let s = if bytes_per_sec >= 1000 * MB {
        format!("{}GB/s", bytes_per_sec.div_ceil(GB))
    } else if bytes_per_sec >= 1000 * KB {
        format!("{}MB/s", bytes_per_sec.div_ceil(MB))
    } else {
        format!("{}kB/s", bytes_per_sec.div_ceil(KB))
    };

    format!("{s:<7}")
}

/// Format bytes as human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
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
        let percent = (used * 100)
            .checked_div(self.total)
            .map(|v| v.min(100))
            .unwrap_or(0);

        // Convert to GB (rounded to nearest integer)
        let used_gb = (used as f64 / BYTES_PER_GB).round() as u64;
        let total_gb = (self.total as f64 / BYTES_PER_GB).round() as u64;

        if let Some(device) = &self.device {
            // Extract device name from path like "/dev/nvme0n1p2" -> "NVME0N1P2"
            let device_name = device
                .strip_prefix("/dev/")
                .unwrap_or(device)
                .to_uppercase();
            format!(
                "{}: {}/{}{} ({}%)",
                device_name,
                used_gb,
                total_gb,
                t.size_gigabytes(),
                percent
            )
        } else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes_bytes() {
        assert_eq!(format_bytes(500), "500B");
    }

    #[test]
    fn test_format_bytes_kb() {
        assert_eq!(format_bytes(2048), "2.0KB");
    }

    #[test]
    fn test_format_bytes_mb() {
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0MB");
    }

    #[test]
    fn test_format_bytes_gb() {
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0GB");
    }

    #[test]
    fn test_format_bytes_zero() {
        assert_eq!(format_bytes(0), "0B");
    }
}
