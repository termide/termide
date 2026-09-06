//! System resource monitoring for termide.
//!
//! Provides CPU, memory, and network usage information.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, Networks, ProcessRefreshKind, RefreshKind, System,
    UpdateKind,
};

mod disk;
mod format;
mod net_processes;

pub use disk::{get_all_disk_space_info, get_disk_space_info};
pub use format::{format_bytes, format_net_speed, DiskSpaceInfoExt};

#[cfg(unix)]
use std::path::Path;

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

/// Battery information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryInfo {
    /// State of charge, percentage in 0..=100.
    pub percent: u8,
    /// Whether AC power is connected (charging or full).
    pub charging: bool,
}

/// Read battery information from the first available battery.
/// Returns `None` if no battery is present or the platform backend fails.
pub fn get_battery_info() -> Option<BatteryInfo> {
    use starship_battery::{Manager, State};
    let manager = Manager::new().ok()?;
    let mut batteries = manager.batteries().ok()?;
    let battery = batteries.next()?.ok()?;
    let percent = (battery.state_of_charge().value * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let charging = !matches!(battery.state(), State::Discharging);
    Some(BatteryInfo { percent, charging })
}

/// Network throughput state.
struct NetworkState {
    networks: Networks,
    download_rate: u64,
    upload_rate: u64,
    last_refresh: Instant,
}

struct MountCacheEntry {
    canonical_path: std::path::PathBuf,
    device: Option<String>,
}

struct ProcessCacheEntry {
    cpu_top: Vec<ProcessInfo>,
    mem_top: Vec<ProcessInfo>,
}

/// System monitor for tracking resource usage.
pub struct SystemMonitor {
    system: Arc<Mutex<System>>,
    net_state: Mutex<NetworkState>,
    /// Cached battery reading + when it was taken.
    battery_cache: Mutex<(Option<BatteryInfo>, Instant)>,
    /// Cached network process list, refreshed at most every `NET_PROCESS_CACHE_TTL`.
    net_process_cache: Mutex<(Vec<NetworkProcessInfo>, Instant)>,
    /// Cached mount-device resolution. The mount table rarely changes.
    #[cfg(unix)]
    mount_cache: Mutex<(Option<MountCacheEntry>, Instant)>,
    /// Cached process lists for CPU/RAM modals.
    process_cache: Mutex<(Option<ProcessCacheEntry>, Instant)>,
    /// Cached all-disk info for Disk modal.
    all_disk_cache: Mutex<(Vec<DiskSpaceInfo>, Instant)>,
}

/// How often the cached battery reading is refreshed.
const BATTERY_REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

/// How often the mount table cache is refreshed.
#[cfg(unix)]
const MOUNT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// How often the cached process lists are refreshed.
const PROCESS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the cached network process list is refreshed.
const NET_PROCESS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

/// How often the cached all-disk list is refreshed.
const ALL_DISK_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

// Manual Debug impl because Networks doesn't implement Debug
impl std::fmt::Debug for SystemMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemMonitor")
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Bytes per gigabyte.
pub(crate) const BYTES_PER_GB: f64 = 1_073_741_824.0;
/// Bytes per megabyte.
const BYTES_PER_MB: f64 = 1_048_576.0;

impl SystemMonitor {
    /// Create a new system monitor.
    pub fn new() -> Self {
        let refresh_kind = Self::refresh_kind();

        let mut system = System::new_with_specifics(refresh_kind);
        system.refresh_specifics(refresh_kind);

        let networks = Networks::new_with_refreshed_list();

        // Seed the battery cache so the first render doesn't block waiting
        // for `/sys`. The `Instant::now() - BATTERY_REFRESH` trick forces a
        // refresh on the next `battery_cached()` call.
        let battery_cache = Mutex::new((
            None,
            Instant::now()
                .checked_sub(BATTERY_REFRESH)
                .unwrap_or_else(Instant::now),
        ));

        Self {
            system: Arc::new(Mutex::new(system)),
            net_state: Mutex::new(NetworkState {
                networks,
                download_rate: 0,
                upload_rate: 0,
                last_refresh: Instant::now(),
            }),
            battery_cache,
            #[cfg(unix)]
            mount_cache: Mutex::new((
                None,
                Instant::now()
                    .checked_sub(MOUNT_CACHE_TTL)
                    .unwrap_or_else(Instant::now),
            )),
            process_cache: Mutex::new((
                None,
                Instant::now()
                    .checked_sub(PROCESS_CACHE_TTL)
                    .unwrap_or_else(Instant::now),
            )),
            net_process_cache: Mutex::new((
                Vec::new(),
                Instant::now()
                    .checked_sub(NET_PROCESS_CACHE_TTL)
                    .unwrap_or_else(Instant::now),
            )),
            all_disk_cache: Mutex::new((
                Vec::new(),
                Instant::now()
                    .checked_sub(ALL_DISK_CACHE_TTL)
                    .unwrap_or_else(Instant::now),
            )),
        }
    }

    /// Get the battery reading, refreshed at most every `BATTERY_REFRESH`.
    ///
    /// Intended for the hot path (status bar / menu render). Use
    /// [`get_battery_info`] directly for a forced read.
    pub fn battery_cached(&self) -> Option<BatteryInfo> {
        let mut guard = match self.battery_cache.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.1.elapsed() >= BATTERY_REFRESH {
            *guard = (get_battery_info(), Instant::now());
        }
        guard.0
    }

    /// Get refresh kind configuration.
    fn refresh_kind() -> RefreshKind {
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram())
    }

    /// Execute a function with locked system, returning default on lock failure.
    fn with_system<T: Default>(&self, f: impl FnOnce(&System) -> T) -> T {
        self.system.lock().map(|sys| f(&sys)).unwrap_or_default()
    }

    /// Refresh system information.
    pub fn refresh(&self) {
        if let Ok(mut sys) = self.system.lock() {
            sys.refresh_specifics(Self::refresh_kind());
        }
        self.refresh_networks();
    }

    /// Refresh network statistics and compute throughput rates.
    fn refresh_networks(&self) {
        if let Ok(mut state) = self.net_state.lock() {
            let elapsed = state.last_refresh.elapsed();
            let elapsed_secs = elapsed.as_secs_f64();

            state.networks.refresh();

            let mut total_rx: u64 = 0;
            let mut total_tx: u64 = 0;
            for (_name, data) in &state.networks {
                total_rx += data.received();
                total_tx += data.transmitted();
            }

            if elapsed_secs > 0.0 {
                state.download_rate = (total_rx as f64 / elapsed_secs) as u64;
                state.upload_rate = (total_tx as f64 / elapsed_secs) as u64;
            }

            state.last_refresh = Instant::now();
        }
    }

    /// Alias for refresh() - backward compatibility.
    #[inline]
    pub fn update(&mut self) {
        self.refresh();
    }

    /// Get current system stats.
    pub fn stats(&self) -> SystemStats {
        self.with_system(|sys| SystemStats {
            cpu_usage: sys.global_cpu_usage(),
            memory_used: sys.used_memory(),
            memory_total: sys.total_memory(),
        })
    }

    /// Get CPU usage as integer percentage (0-100).
    pub fn cpu_usage(&self) -> u8 {
        self.with_system(|sys| sys.global_cpu_usage().round() as u8)
    }

    /// Get memory usage percentage.
    pub fn memory_percent(&self) -> f32 {
        self.stats().memory_percent()
    }

    /// Get RAM info in specified unit: (used, total).
    fn ram_info(&self, divisor: f64) -> (u64, u64) {
        self.with_system(|sys| {
            let used = (sys.used_memory() as f64 / divisor).round() as u64;
            let total = (sys.total_memory() as f64 / divisor).round() as u64;
            (used, total)
        })
    }

    /// Get RAM info: (used_gb, total_gb).
    pub fn ram_info_gb(&self) -> (u64, u64) {
        self.ram_info(BYTES_PER_GB)
    }

    /// Get RAM info: (used_mb, total_mb).
    pub fn ram_info_mb(&self) -> (u64, u64) {
        self.ram_info(BYTES_PER_MB)
    }

    /// Get RAM usage as integer percentage (0-100).
    pub fn ram_usage_percent(&self) -> u8 {
        self.with_system(|sys| {
            let used = sys.used_memory();
            let total = sys.total_memory();
            if total > 0 {
                ((used as f64 / total as f64) * 100.0).round() as u8
            } else {
                0
            }
        })
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

    /// Get network download rate in bytes per second.
    pub fn net_download_rate(&self) -> u64 {
        self.net_state.lock().map(|s| s.download_rate).unwrap_or(0)
    }

    /// Get network upload rate in bytes per second.
    pub fn net_upload_rate(&self) -> u64 {
        self.net_state.lock().map(|s| s.upload_rate).unwrap_or(0)
    }

    /// Get top N processes by CPU usage, grouped by binary name.
    ///
    /// Performs an on-demand process refresh (not part of periodic refresh).
    pub fn top_cpu_processes(&self, n: usize) -> Vec<ProcessInfo> {
        self.grouped_processes(n, |p| p.cpu_percent, true)
    }

    /// Get top N processes by memory usage, grouped by binary name.
    ///
    /// Performs an on-demand process refresh (not part of periodic refresh).
    pub fn top_memory_processes(&self, n: usize) -> Vec<ProcessInfo> {
        self.grouped_processes(n, |p| p.memory_bytes as f32, false)
    }

    /// Get cached top N processes by CPU and memory, refreshing at most every
    /// `PROCESS_CACHE_TTL`. Avoids a full process refresh on every resource tick.
    pub fn top_processes_cached(&self, n: usize) -> (Vec<ProcessInfo>, Vec<ProcessInfo>) {
        let mut guard = match self.process_cache.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.1.elapsed() >= PROCESS_CACHE_TTL {
            let cpu = self.grouped_processes(n, |p| p.cpu_percent, true);
            let mem = self.grouped_processes(n, |p| p.memory_bytes as f32, false);
            *guard = (
                Some(ProcessCacheEntry {
                    cpu_top: cpu.clone(),
                    mem_top: mem.clone(),
                }),
                Instant::now(),
            );
            (cpu, mem)
        } else {
            guard
                .0
                .as_ref()
                .map(|e| (e.cpu_top.clone(), e.mem_top.clone()))
                .unwrap_or_default()
        }
    }

    /// Get top N processes by network activity, cached to avoid a full process
    /// scan on every resource tick. The cache refreshes every `NET_PROCESS_CACHE_TTL`.
    pub fn top_network_processes_cached(&self, n: usize) -> Vec<NetworkProcessInfo> {
        let mut guard = match self.net_process_cache.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.1.elapsed() >= NET_PROCESS_CACHE_TTL {
            let mut result = net_processes::collect();
            result.sort_by_key(|p| std::cmp::Reverse(p.connections));
            result.truncate(n);
            *guard = (result.clone(), Instant::now());
            result
        } else {
            guard.0.clone()
        }
    }

    /// Get disk space info with cached mount-device resolution.
    ///
    /// Same as `get_disk_space_info()` but avoids re-reading the mount table
    /// and calling `canonicalize()` on every mount entry when the path hasn't
    /// changed and the cache is fresh.
    #[cfg(unix)]
    pub fn get_disk_space_info_cached(&self, path: &Path) -> Option<DiskSpaceInfo> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let canonical = path.canonicalize().ok()?;

        let device = {
            let mut guard = match self.mount_cache.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let needs_refresh = guard.1.elapsed() >= MOUNT_CACHE_TTL
                || guard
                    .0
                    .as_ref()
                    .is_none_or(|e| e.canonical_path != canonical);
            if needs_refresh {
                let dev = disk::get_device_for_path(path);
                *guard = (
                    Some(MountCacheEntry {
                        canonical_path: canonical,
                        device: dev.clone(),
                    }),
                    Instant::now(),
                );
                dev
            } else {
                guard.0.as_ref().unwrap().device.clone()
            }
        };

        let path_cstr = CString::new(path.as_os_str().as_bytes()).ok()?;
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                #[cfg(target_os = "macos")]
                let available = (stat.f_bavail as u64) * stat.f_bsize;
                #[cfg(not(target_os = "macos"))]
                let available = stat.f_bavail * stat.f_bsize;

                #[cfg(target_os = "macos")]
                let total = (stat.f_blocks as u64) * stat.f_bsize;
                #[cfg(not(target_os = "macos"))]
                let total = stat.f_blocks * stat.f_bsize;

                Some(DiskSpaceInfo {
                    device,
                    available,
                    total,
                })
            } else {
                None
            }
        }
    }

    /// Refresh processes and return top N grouped by name, sorted by the given key.
    fn grouped_processes(
        &self,
        n: usize,
        sort_key: impl Fn(&ProcessInfo) -> f32,
        is_cpu: bool,
    ) -> Vec<ProcessInfo> {
        let Ok(mut sys) = self.system.lock() else {
            return Vec::new();
        };

        // One-shot process refresh with CPU, memory, and exe path (to filter kernel threads)
        let process_refresh = ProcessRefreshKind::new()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, process_refresh);

        // Normalize CPU usage to total system capacity (0-100%)
        let num_cpus = sys.cpus().len().max(1) as f32;

        // Group by process name
        let mut grouped: HashMap<String, ProcessInfo> = HashMap::new();
        for process in sys.processes().values() {
            // Skip threads (they share memory with the main process) and
            // kernel threads (no executable on disk)
            if process.thread_kind().is_some() {
                continue;
            }
            if process.exe().is_none_or(|p| p.as_os_str().is_empty()) {
                continue;
            }
            let name = process.name().to_string_lossy().into_owned();
            if name.is_empty() {
                continue;
            }
            let cpu = process.cpu_usage() / num_cpus;
            let mem = process.memory();
            let entry = grouped.entry(name.clone()).or_insert_with(|| ProcessInfo {
                name,
                cpu_percent: 0.0,
                memory_bytes: 0,
                count: 0,
            });
            if is_cpu {
                entry.cpu_percent += cpu;
            } else {
                entry.cpu_percent = entry.cpu_percent.max(cpu);
            }
            entry.memory_bytes += mem;
            entry.count += 1;
        }

        let mut processes: Vec<ProcessInfo> = grouped.into_values().collect();
        processes.sort_by(|a, b| {
            sort_key(b)
                .partial_cmp(&sort_key(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        processes.truncate(n);
        processes
    }

    /// Windows fallback: no mount table to cache, so just call the
    /// underlying `get_disk_space_info()` directly. Mirrors the cfg-gating
    /// of the free function below.
    #[cfg(windows)]
    pub fn get_disk_space_info_cached(&self, path: &std::path::Path) -> Option<DiskSpaceInfo> {
        get_disk_space_info(path)
    }

    /// Cached version of [`get_all_disk_space_info()`] with 5s TTL.
    pub fn get_all_disk_space_info_cached(&self) -> Vec<DiskSpaceInfo> {
        let mut guard = match self.all_disk_cache.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.1.elapsed() >= ALL_DISK_CACHE_TTL {
            *guard = (get_all_disk_space_info(), Instant::now());
        }
        guard.0.clone()
    }
}

/// Network activity information for a process or group of processes with the same name.
#[derive(Debug, Clone)]
pub struct NetworkProcessInfo {
    /// Process binary name.
    pub name: String,
    /// Sorted listening TCP ports.
    pub listening_ports: Vec<u16>,
    /// Number of established TCP connections.
    pub connections: usize,
}

/// Information about a process (or group of processes with the same name).
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process binary name.
    pub name: String,
    /// CPU usage percentage (summed across all processes with this name).
    pub cpu_percent: f32,
    /// Memory usage in bytes (summed across all processes with this name).
    pub memory_bytes: u64,
    /// Number of processes with this name.
    pub count: usize,
}

/// Disk space information.
#[derive(Clone, Debug, PartialEq)]
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
        let used = self.total.saturating_sub(self.available);
        (used * 100)
            .checked_div(self.total)
            .map(|v| v.min(100) as u8)
            .unwrap_or(0)
    }

    /// Get used space in bytes.
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Get used space in GB.
    pub fn used_gb(&self) -> u64 {
        (self.used() as f64 / BYTES_PER_GB).round() as u64
    }

    /// Get total space in GB.
    pub fn total_gb(&self) -> u64 {
        (self.total as f64 / BYTES_PER_GB).round() as u64
    }

    /// Get device name (extracted from path).
    pub fn device_name(&self) -> Option<String> {
        self.device
            .as_ref()
            .map(|d| d.strip_prefix("/dev/").unwrap_or(d).to_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // SystemStats::memory_percent()
    // =========================================================================

    #[test]
    fn test_memory_percent_normal() {
        let stats = SystemStats {
            cpu_usage: 50.0,
            memory_used: 4_000_000_000,
            memory_total: 16_000_000_000,
        };
        let percent = stats.memory_percent();
        assert!((percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_memory_percent_zero_total() {
        let stats = SystemStats {
            cpu_usage: 0.0,
            memory_used: 0,
            memory_total: 0,
        };
        assert_eq!(stats.memory_percent(), 0.0);
    }

    #[test]
    fn test_memory_percent_full() {
        let stats = SystemStats {
            cpu_usage: 0.0,
            memory_used: 16_000_000_000,
            memory_total: 16_000_000_000,
        };
        assert!((stats.memory_percent() - 100.0).abs() < 0.01);
    }

    // =========================================================================
    // DiskSpaceInfo
    // =========================================================================

    #[test]
    fn test_disk_usage_percent() {
        let info = DiskSpaceInfo {
            device: Some("/dev/sda1".to_string()),
            available: 200_000_000_000,
            total: 1_000_000_000_000,
        };
        // used = 800GB, total = 1TB, percent = 80%
        assert_eq!(info.usage_percent(), 80);
    }

    #[test]
    fn test_disk_usage_percent_zero_total() {
        let info = DiskSpaceInfo {
            device: None,
            available: 0,
            total: 0,
        };
        assert_eq!(info.usage_percent(), 0);
    }

    #[test]
    fn test_disk_used_bytes() {
        let info = DiskSpaceInfo {
            device: None,
            available: 300,
            total: 1000,
        };
        assert_eq!(info.used(), 700);
    }

    #[test]
    fn test_disk_device_name() {
        let info = DiskSpaceInfo {
            device: Some("/dev/nvme0n1p2".to_string()),
            available: 0,
            total: 0,
        };
        assert_eq!(info.device_name(), Some("NVME0N1P2".to_string()));
    }

    #[test]
    fn test_disk_device_name_no_prefix() {
        let info = DiskSpaceInfo {
            device: Some("sda1".to_string()),
            available: 0,
            total: 0,
        };
        assert_eq!(info.device_name(), Some("SDA1".to_string()));
    }

    #[test]
    fn test_disk_device_name_none() {
        let info = DiskSpaceInfo {
            device: None,
            available: 0,
            total: 0,
        };
        assert_eq!(info.device_name(), None);
    }

    // =========================================================================
    // Process info
    // =========================================================================

    #[test]
    fn test_top_cpu_processes_sorted() {
        let monitor = SystemMonitor::new();
        let procs = monitor.top_cpu_processes(10);
        // Verify descending CPU order
        for window in procs.windows(2) {
            assert!(window[0].cpu_percent >= window[1].cpu_percent);
        }
    }

    #[test]
    fn test_top_memory_processes_sorted() {
        let monitor = SystemMonitor::new();
        let procs = monitor.top_memory_processes(10);
        // Verify descending memory order
        for window in procs.windows(2) {
            assert!(window[0].memory_bytes >= window[1].memory_bytes);
        }
    }

    #[test]
    fn test_processes_grouped_by_name() {
        let monitor = SystemMonitor::new();
        let procs = monitor.top_cpu_processes(100);
        // All names should be unique (grouped)
        let mut names: Vec<&str> = procs.iter().map(|p| p.name.as_str()).collect();
        let len_before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), len_before);
    }

    // =========================================================================
    // DiskSpaceInfo GB calculations
    // =========================================================================

    #[test]
    fn test_disk_used_gb() {
        let info = DiskSpaceInfo {
            device: None,
            available: 500 * 1_073_741_824, // 500 GB available
            total: 1000 * 1_073_741_824,    // 1 TB total
        };
        assert_eq!(info.used_gb(), 500);
        assert_eq!(info.total_gb(), 1000);
    }
}
