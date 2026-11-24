//! Utility functions shared across termide crates.

use std::io::Read;
use std::path::Path;

/// Replace home directory prefix with `~` for display.
/// E.g. "/home/user/projects/foo" → "~/projects/foo"
pub fn shorten_home_path(path: &str) -> String {
    match dirs::home_dir() {
        Some(home) => shorten_under(path, &home.display().to_string()),
        None => path.to_string(),
    }
}

/// `path` with `home` shown as `~` when it is `home` or lies under it:
/// `/home/alice` is not under `/home/al`.
fn shorten_under(path: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', std::path::MAIN_SEPARATOR]);
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', std::path::MAIN_SEPARATOR]) => {
            format!("~{rest}")
        }
        _ => path.to_string(),
    }
}

/// Check if file appears to be binary (has null bytes in first 8KB).
///
/// This is a heuristic: text files rarely contain null bytes,
/// while binary files (executables, images, etc.) typically do.
pub fn is_binary_file(path: &Path) -> bool {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = [0u8; 8192];
    let bytes_read = match reader.read(&mut buffer) {
        Ok(n) => n,
        Err(_) => return false,
    };
    buffer[..bytes_read].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_home_directory_and_what_lies_under_it_become_tilde() {
        assert_eq!(shorten_under("/home/al", "/home/al"), "~");
        assert_eq!(shorten_under("/home/al/x", "/home/al"), "~/x");
        assert_eq!(shorten_under("/home/alice/x", "/home/al"), "/home/alice/x");
        assert_eq!(shorten_under("/etc/passwd", "/home/al"), "/etc/passwd");
        assert_eq!(shorten_under("/home/al/x", "/home/al/"), "~/x");
        assert_eq!(shorten_under("/x", "/"), "/x");
    }
}
