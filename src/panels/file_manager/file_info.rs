use std::fs;
use std::sync::mpsc;

use super::{utils, FileManager};
use crate::state::{ActiveModal, DirSizeResult, PendingAction};

/// File information for display
#[derive(Clone, Debug)]
pub struct FileInfo {
    pub name: String,
    pub file_type: String,
    pub size: String,
    pub owner: String,
    pub group: String,
    #[allow(dead_code)]
    pub modified: String,
    pub mode: String, // Access permissions in format "0755"
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

impl FileManager {
    /// Get information about the currently selected file
    pub fn get_current_file_info(&self) -> Option<FileInfo> {
        use std::os::unix::fs::MetadataExt;
        use std::time::SystemTime;

        let entry = self.entries.get(self.selected)?;

        let file_path = if entry.name == ".." {
            self.current_path
                .parent()
                .unwrap_or(&self.current_path)
                .to_path_buf()
        } else {
            self.current_path.join(&entry.name)
        };

        let metadata = fs::metadata(&file_path).ok()?;

        let file_type = if metadata.is_dir() {
            "Directory"
        } else if metadata.is_symlink() {
            "Symlink"
        } else {
            "File"
        };

        let size = if metadata.is_dir() {
            "DIR".to_string()
        } else {
            utils::format_size(metadata.len())
        };

        let owner = utils::get_user_name(metadata.uid());
        let group = utils::get_group_name(metadata.gid());

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            })
            .unwrap_or_else(|| "Unknown".to_string());

        // Format access permissions in octal format (e.g. "0755")
        let mode = format!("{:04o}", metadata.mode() & 0o7777);

        Some(FileInfo {
            name: entry.name.clone(),
            file_type: file_type.to_string(),
            size,
            owner,
            group,
            modified,
            mode,
        })
    }

    /// Show file/directory information (Space)
    pub(super) fn show_file_info(&mut self) {
        use std::os::unix::fs::MetadataExt;
        use std::time::SystemTime;

        if let Some(entry) = self.entries.get(self.selected) {
            let file_path = if entry.name == ".." {
                self.current_path
                    .parent()
                    .unwrap_or(&self.current_path)
                    .to_path_buf()
            } else {
                self.current_path.join(&entry.name)
            };

            if let Ok(metadata) = fs::metadata(&file_path) {
                let t = crate::i18n::t();

                // Determine type and title
                let (modal_title, is_dir) = if metadata.is_dir() {
                    (t.file_info_title_directory(&entry.name), true)
                } else if metadata.is_symlink() {
                    (t.file_info_title_symlink(&entry.name), false)
                } else {
                    (t.file_info_title_file(&entry.name), false)
                };

                let size = if is_dir {
                    format!("{}...", t.file_info_calculating())
                } else {
                    utils::format_size(metadata.len())
                };

                let owner = utils::get_user_name(metadata.uid());
                let group = utils::get_group_name(metadata.gid());

                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| {
                        chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                            .unwrap_or_else(|| "Unknown".to_string())
                    })
                    .unwrap_or_else(|| "Unknown".to_string());

                let created = metadata
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| {
                        chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                            .unwrap_or_else(|| "Unknown".to_string())
                    })
                    .unwrap_or_else(|| "Unknown".to_string());

                // Collect data without Name and Type
                let mut data = vec![
                    (
                        t.file_info_path().to_string(),
                        file_path.display().to_string(),
                    ),
                    (t.file_info_size().to_string(), size),
                    (t.file_info_owner().to_string(), owner),
                    (t.file_info_group().to_string(), group),
                    (t.file_info_created().to_string(), created),
                    (t.file_info_modified().to_string(), modified),
                ];

                // Add git status if in repository (filtered by specific file/directory)
                if let Some(git_status) =
                    crate::git::get_repo_status(&self.current_path, &file_path)
                {
                    if git_status.is_ignored {
                        // If file is ignored, show only one line
                        data.push((
                            t.file_info_git().to_string(),
                            t.file_info_git_ignored().to_string(),
                        ));
                    } else {
                        // Otherwise show three lines for uncommitted, ahead, behind
                        data.push((
                            t.file_info_git().to_string(),
                            t.file_info_git_uncommitted(git_status.uncommitted_changes),
                        ));
                        data.push((
                            String::new(), // Empty key - aligns with first line's value
                            t.file_info_git_ahead(git_status.ahead),
                        ));
                        data.push((
                            String::new(), // Empty key
                            t.file_info_git_behind(git_status.behind),
                        ));
                    }
                }

                let modal = crate::ui::modal::InfoModal::new(modal_title, data);
                self.modal_request = Some((
                    PendingAction::ClosePanel { panel_index: 0 },
                    ActiveModal::Info(Box::new(modal)),
                ));

                if is_dir {
                    let path = file_path.clone();
                    let (tx, rx) = mpsc::channel();

                    std::thread::spawn(move || {
                        let size = utils::calculate_dir_size(&path);
                        let _ = tx.send(DirSizeResult { size });
                    });

                    self.dir_size_receiver = Some(rx);
                }
            }
        }
    }

    /// Resolve dm-X device to physical partition
    /// e.g., /dev/dm-0 -> /dev/nvme0n1p2
    fn resolve_dm_device(device: &str) -> Option<String> {
        // Extract dm number (e.g., "dm-0" from "/dev/dm-0")
        let dm_name = device.strip_prefix("/dev/")?;
        if !dm_name.starts_with("dm-") {
            return None;
        }

        // Read /sys/block/dm-X/slaves/ to find physical partition
        let slaves_path = format!("/sys/block/{}/slaves", dm_name);
        let slaves_dir = std::fs::read_dir(&slaves_path).ok()?;

        // Get first slave (physical partition)
        for entry in slaves_dir.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                return Some(format!("/dev/{}", name));
            }
        }

        None
    }

    /// Get device name from /proc/mounts for a given path
    fn get_device_for_path(path: &std::path::Path) -> Option<String> {
        let mounts_content = std::fs::read_to_string("/proc/mounts").ok()?;
        let mut best_match: Option<(String, usize)> = None;

        for line in mounts_content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }

            let device = parts[0];
            let mount_point = parts[1];

            // Check if this mount point is a prefix of our path
            if let Ok(canonical_path) = path.canonicalize() {
                if let Ok(canonical_mount) = std::path::Path::new(mount_point).canonicalize() {
                    if canonical_path.starts_with(&canonical_mount) {
                        let mount_len = canonical_mount.as_os_str().len();
                        // Keep track of the longest matching mount point
                        if best_match.is_none() || mount_len > best_match.as_ref().unwrap().1 {
                            best_match = Some((device.to_string(), mount_len));
                        }
                    }
                }
            }
        }

        best_match.and_then(|(device, _)| {
            // First try to resolve symlink (e.g., /dev/disk/by-uuid/... -> /dev/nvme0n1p2)
            let resolved = std::path::Path::new(&device)
                .canonicalize()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| device.clone());

            // If it's a dm device, resolve to physical partition
            if resolved.contains("/dm-") {
                Self::resolve_dm_device(&resolved).or(Some(resolved))
            } else {
                Some(resolved)
            }
        })
    }

    /// Get disk space information for the current directory
    pub fn get_disk_space_info(&self) -> Option<DiskSpaceInfo> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        // Convert path to CString for passing to statvfs
        let path_cstr = CString::new(self.current_path.as_os_str().as_bytes()).ok()?;

        // Get device name for this path
        let device = Self::get_device_for_path(&self.current_path);

        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                // f_bavail - available blocks for non-privileged users
                // f_blocks - total blocks in the filesystem
                // f_bsize - block size in bytes
                let available = stat.f_bavail * stat.f_bsize;
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
}
