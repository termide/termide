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
    pub modified: String,
    pub mode: String, // Access permissions in format "0755"
}

/// Disk space information
#[derive(Clone, Debug)]
pub struct DiskSpaceInfo {
    pub available: u64,
    pub total: u64,
}

impl DiskSpaceInfo {
    /// Format disk information: "100GB / 512GB (20%)"
    pub fn format_space(&self) -> String {
        let percent = if self.total > 0 {
            (self.available * 100) / self.total
        } else {
            0
        };

        format!(
            "{} / {} ({}%)",
            utils::format_size(self.available),
            utils::format_size(self.total),
            percent
        )
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
                let data = vec![
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

    /// Get disk space information for the current directory
    pub fn get_disk_space_info(&self) -> Option<DiskSpaceInfo> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        // Convert path to CString for passing to statvfs
        let path_cstr = CString::new(self.current_path.as_os_str().as_bytes()).ok()?;

        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                // f_bavail - available blocks for non-privileged users
                // f_blocks - total blocks in the filesystem
                // f_bsize - block size in bytes
                let available = (stat.f_bavail as u64) * (stat.f_bsize as u64);
                let total = (stat.f_blocks as u64) * (stat.f_bsize as u64);

                Some(DiskSpaceInfo { available, total })
            } else {
                None
            }
        }
    }
}
