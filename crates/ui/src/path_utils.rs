//! Utilities for resolving file operation destination paths

use std::path::{Path, PathBuf};

/// Expand leading `~` or `~/` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Whether the raw user input explicitly refers to a directory path.
pub fn has_trailing_directory_separator(input: &str) -> bool {
    input.ends_with('/') || input.ends_with(std::path::MAIN_SEPARATOR)
}

/// Resolve a local destination input string against a base directory.
///
/// Returns the normalized path and whether the user explicitly marked it as a
/// directory by using a trailing path separator.
pub fn resolve_local_destination_input(base_dir: &Path, input: &str) -> (PathBuf, bool) {
    let destination_is_directory = has_trailing_directory_separator(input);
    let destination = expand_tilde(input);
    let absolute = if destination.is_absolute() {
        destination
    } else {
        base_dir.join(destination)
    };
    (absolute, destination_is_directory)
}

fn destination_is_directory(destination: &Path, explicit_directory: bool) -> bool {
    explicit_directory || destination.is_dir()
}

/// Resolve destination path for batch operations
///
/// Handles special case where single source to non-directory destination
/// should use the destination name (rename operation).
pub fn resolve_batch_destination_path(
    source: &Path,
    destination: &Path,
    is_single_source: bool,
    explicit_directory: bool,
) -> PathBuf {
    if destination_is_directory(destination, explicit_directory) {
        // Destination is directory - append source filename
        destination.join(source.file_name().unwrap_or_default())
    } else if is_single_source {
        // Single file to non-directory - use destination as-is (rename)
        destination.to_path_buf()
    } else {
        // Multiple files to non-directory - append filename (fallback)
        destination.join(source.file_name().unwrap_or_default())
    }
}

/// Resolve destination path when applying rename pattern
///
/// If destination is a directory, joins new_name to it.
/// Otherwise, replaces filename in destination path with new_name.
pub fn resolve_rename_destination_path(
    destination: &Path,
    new_name: &str,
    explicit_directory: bool,
) -> PathBuf {
    if destination_is_directory(destination, explicit_directory) {
        destination.join(new_name)
    } else {
        destination.with_file_name(new_name)
    }
}

/// Extract file name from path as string slice
///
/// Returns "?" if the path has no file name or it's not valid UTF-8.
pub fn get_file_name_str(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
}

/// Extract file name from path as String
///
/// Returns "?" if the path has no file name or it's not valid UTF-8.
pub fn get_file_name_string(path: &Path) -> String {
    get_file_name_str(path).to_string()
}

/// Truncate `s` to at most `max_width` display columns, returning a byte slice
/// aligned to a char boundary. Respects Unicode widths (CJK = 2). No ellipsis;
/// allocation-free, suitable for render paths.
#[must_use]
pub fn truncate_to_width_str(s: &str, max_width: usize) -> &str {
    let mut end = 0;
    let mut width = 0;
    for c in s.chars() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width > max_width {
            break;
        }
        width += char_width;
        end += c.len_utf8();
    }
    &s[..end]
}

/// Truncate a string to fit within a given display width.
///
/// Respects Unicode character widths (e.g., CJK characters count as 2).
#[must_use]
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    truncate_to_width_str(s, max_width).to_string()
}

/// Truncate a string from the right with ellipsis suffix.
///
/// Keeps the leftmost (beginning) part of the string.
/// Respects Unicode character widths (e.g., CJK characters count as 2).
pub fn truncate_right(s: &str, max_width: usize) -> String {
    let current_width = s.width();
    if current_width <= max_width {
        return s.to_string();
    }

    let ellipsis = "…";
    let ellipsis_width = 1;
    let available = max_width.saturating_sub(ellipsis_width);

    // Take characters from the left until we reach available width
    let mut result = String::new();
    let mut width = 0;

    for c in s.chars() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width > available {
            break;
        }
        result.push(c);
        width += char_width;
    }

    format!("{}{}", result, ellipsis)
}

/// Truncate a string from the left with ellipsis prefix.
///
/// Keeps the rightmost (most relevant) part of the string.
/// Respects Unicode character widths (e.g., CJK characters count as 2).
pub fn truncate_left(s: &str, max_width: usize) -> String {
    let current_width = s.width();
    if current_width <= max_width {
        return s.to_string();
    }

    let ellipsis = "…";
    let ellipsis_width = 1;
    let available = max_width.saturating_sub(ellipsis_width);

    // Take characters from the right until we reach available width
    let mut chars_rev = Vec::new();
    let mut width = 0;

    for c in s.chars().rev() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width > available {
            break;
        }
        chars_rev.push(c);
        width += char_width;
    }

    let mut result = String::with_capacity(ellipsis.len() + chars_rev.len() * 4);
    result.push_str(ellipsis);
    for &c in chars_rev.iter().rev() {
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("termide-path-utils-{name}-{unique}"))
    }

    #[test]
    fn trailing_slash_marks_directory_intent() {
        assert!(has_trailing_directory_separator("dest/"));
        assert!(!has_trailing_directory_separator("dest"));
    }

    #[test]
    fn resolve_local_destination_preserves_directory_intent() {
        let base = PathBuf::from("/tmp/base");
        let (path, is_dir) = resolve_local_destination_input(&base, "nested/");
        assert_eq!(path, PathBuf::from("/tmp/base/nested"));
        assert!(is_dir);
    }

    #[test]
    fn single_source_nonexisting_trailing_slash_is_directory() {
        let source = PathBuf::from("/tmp/source.txt");
        let destination = PathBuf::from("/tmp/newdir");
        let final_dest = resolve_batch_destination_path(&source, &destination, true, true);
        assert_eq!(final_dest, PathBuf::from("/tmp/newdir/source.txt"));
    }

    #[test]
    fn single_source_nonexisting_without_trailing_slash_is_exact_path() {
        let source = PathBuf::from("/tmp/source.txt");
        let destination = PathBuf::from("/tmp/renamed.txt");
        let final_dest = resolve_batch_destination_path(&source, &destination, true, false);
        assert_eq!(final_dest, destination);
    }

    #[test]
    fn existing_directory_is_used_even_without_explicit_trailing_slash() {
        let temp_dir = temp_path("existing-dir");
        fs::create_dir_all(&temp_dir).unwrap();
        let source = PathBuf::from("/tmp/source.txt");
        let final_dest = resolve_batch_destination_path(&source, &temp_dir, true, false);
        assert_eq!(final_dest, temp_dir.join("source.txt"));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn rename_destination_honors_explicit_directory_intent() {
        let destination = PathBuf::from("/tmp/newdir");
        let final_dest = resolve_rename_destination_path(&destination, "renamed.txt", true);
        assert_eq!(final_dest, PathBuf::from("/tmp/newdir/renamed.txt"));
    }

    #[test]
    fn single_source_file_like_destination_stays_exact_path() {
        let base_dir = PathBuf::from("/repo");
        let source = PathBuf::from("/repo/AGENTS.md");
        let (destination, is_dir) = resolve_local_destination_input(&base_dir, ".claude/CLAUDE.md");
        let final_dest = resolve_batch_destination_path(&source, &destination, true, is_dir);
        assert_eq!(final_dest, PathBuf::from("/repo/.claude/CLAUDE.md"));
    }

    #[test]
    fn single_source_directory_like_destination_places_file_inside() {
        let base_dir = PathBuf::from("/repo");
        let source = PathBuf::from("/repo/AGENTS.md");
        let (destination, is_dir) = resolve_local_destination_input(&base_dir, ".claude/");
        let final_dest = resolve_batch_destination_path(&source, &destination, true, is_dir);
        assert_eq!(final_dest, PathBuf::from("/repo/.claude/AGENTS.md"));
    }
}
