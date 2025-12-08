use std::fs;
use std::path::Path;
use std::time::SystemTime;

use crate::constants::{GIGABYTE, KILOBYTE, MEGABYTE};
use crate::git::GitStatus;

use super::FileEntry;

/// Get icon for file/directory (1 character)
pub fn get_icon(entry: &FileEntry) -> &'static str {
    // Git deleted
    if entry.git_status == GitStatus::Deleted {
        return "✗";
    }

    // Parent directory
    if entry.name == ".." {
        return "↑";
    }

    // Directory
    if entry.is_dir {
        return if entry.is_symlink { "▷" } else { "▶" };
    }

    // Determine file type by extension
    let path = Path::new(&entry.name);
    let highlighter = crate::syntax_highlighter::global_highlighter();

    // File with syntax highlighting
    if highlighter.language_for_file(path).is_some() {
        return if entry.is_symlink { "○" } else { "●" };
    }

    // Known text extensions without highlighting
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext.to_lowercase().as_str() {
            "txt" | "log" | "conf" | "cfg" | "ini" | "xml" | "properties" | "env" => {
                return if entry.is_symlink { "▫" } else { "▪" };
            }
            _ => {}
        }
    }

    // Binary / unknown files
    if entry.is_symlink {
        "◇"
    } else {
        "◆"
    }
}

/// Get attribute character (R/X flag or selection checkmark)
/// Returns 1 character
pub fn get_attribute(entry: &FileEntry, is_selected: bool) -> &'static str {
    if is_selected {
        return "✓";
    }

    // Executable has priority over read-only (but not for directories)
    if entry.is_executable && !entry.is_dir {
        return "X";
    }

    if entry.is_readonly {
        return "R";
    }

    " "
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

/// Format file size in human-readable format (rounded to whole units)
pub fn format_size(bytes: u64) -> String {
    let t = crate::i18n::t();
    if bytes >= GIGABYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / GIGABYTE as f64,
            t.size_gigabytes()
        )
    } else if bytes >= MEGABYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / MEGABYTE as f64,
            t.size_megabytes()
        )
    } else if bytes >= KILOBYTE {
        format!(
            "{:.0} {}",
            bytes as f64 / KILOBYTE as f64,
            t.size_kilobytes()
        )
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
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "                   ".to_string())
}
