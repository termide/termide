use std::fs;
use std::path::Path;
use std::time::SystemTime;

use unicode_width::UnicodeWidthStr;

use crate::constants::{GIGABYTE, KILOBYTE, MEGABYTE};
use crate::git::GitStatus;

use super::FileEntry;

/// Get icon for file/directory
pub fn get_icon(entry: &FileEntry) -> &'static str {
    // For deleted files, show red cross
    if entry.git_status == GitStatus::Deleted {
        return "✗";
    }

    if entry.name == ".." {
        return "↑";
    }

    if entry.is_dir {
        return "📁";
    }

    // Determine icon by extension
    if let Some(ext) = entry.name.split('.').last() {
        match ext.to_lowercase().as_str() {
            "rs" => "🦀",
            "toml" => "⚙",
            "md" => "📝",
            "txt" => "📄",
            "json" => "{}",
            "yaml" | "yml" => "📋",
            "sh" | "bash" => "🔧",
            "py" => "🐍",
            "js" | "ts" => "📜",
            _ => "📄",
        }
    } else {
        "📄"
    }
}

/// Truncate file name to specified length (in characters, not bytes)
pub fn truncate_name(name: &str, max_len: usize) -> String {
    let char_count = name.chars().count();
    if char_count <= max_len {
        name.to_string()
    } else {
        // Take first (max_len - 1) characters and add ellipsis
        let truncated: String = name.chars().take(max_len.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

/// Format file size in human-readable format
pub fn format_size(bytes: u64) -> String {
    let t = crate::i18n::t();
    if bytes >= GIGABYTE {
        format!("{:.2} {}", bytes as f64 / GIGABYTE as f64, t.size_gigabytes())
    } else if bytes >= MEGABYTE {
        format!("{:.2} {}", bytes as f64 / MEGABYTE as f64, t.size_megabytes())
    } else if bytes >= KILOBYTE {
        format!("{:.2} {}", bytes as f64 / KILOBYTE as f64, t.size_kilobytes())
    } else {
        format!("{} {}", bytes, t.size_bytes())
    }
}

/// Iteratively calculate directory size (without recursion, protected from stack overflow)
pub fn calculate_dir_size(path: &Path) -> u64 {
    use std::collections::VecDeque;

    let mut total_size = 0u64;
    let mut dirs_to_process = VecDeque::new();
    dirs_to_process.push_back(path.to_path_buf());

    // Iterative traversal with explicit stack
    while let Some(current_dir) = dirs_to_process.pop_front() {
        if let Ok(entries) = fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                // Use symlink_metadata to not follow symlinks
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        total_size += metadata.len();
                    } else if metadata.is_dir() {
                        // Add directory to queue for processing
                        dirs_to_process.push_back(entry.path());
                    }
                    // Ignore symlinks to avoid cycles
                }
            }
        }
    }

    total_size
}

/// Get user name by UID
/// Returns symbolic name if available, otherwise numeric ID
pub fn get_user_name(uid: u32) -> String {
    unsafe {
        let pwd = libc::getpwuid(uid);
        if !pwd.is_null() {
            let name_ptr = (*pwd).pw_name;
            if !name_ptr.is_null() {
                if let Ok(name) = std::ffi::CStr::from_ptr(name_ptr).to_str() {
                    return name.to_string();
                }
            }
        }
    }
    uid.to_string()
}

/// Get group name by GID
/// Returns symbolic name if available, otherwise numeric ID
pub fn get_group_name(gid: u32) -> String {
    unsafe {
        let grp = libc::getgrgid(gid);
        if !grp.is_null() {
            let name_ptr = (*grp).gr_name;
            if !name_ptr.is_null() {
                if let Ok(name) = std::ffi::CStr::from_ptr(name_ptr).to_str() {
                    return name.to_string();
                }
            }
        }
    }
    gid.to_string()
}

/// Format modification time in YYYY-MM-DD HH:MM:SS format
/// Returns 19 characters (time string or spaces)
pub fn format_modified_time(time: Option<SystemTime>) -> String {
    time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "                   ".to_string())
}

/// Normalize icon to fixed visual width (2 terminal columns)
/// Adds spaces so all icons occupy the same visual width
pub fn normalize_icon(icon: &str) -> String {
    const TARGET_WIDTH: usize = 2;
    let current_width = icon.width();

    if current_width < TARGET_WIDTH {
        let padding = " ".repeat(TARGET_WIDTH - current_width);
        format!("{}{}", icon, padding)
    } else {
        icon.to_string()
    }
}
