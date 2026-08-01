//! Git blame operations — async inline annotations.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::command::git_command_stdout;

/// A single blame entry for one or more consecutive lines.
#[derive(Debug, Clone)]
pub struct BlameEntry {
    /// Full 40-character commit hash.
    pub commit_hash: String,
    /// Author name.
    pub author: String,
    /// Unix timestamp of the author date.
    pub timestamp: i64,
    /// First line of the commit message.
    pub summary: String,
}

impl BlameEntry {
    /// Format for inline display: "  author, age • abcdef1 summary"
    pub fn inline_text(&self) -> String {
        let age = format_age(self.timestamp);
        let short = &self.commit_hash[..7.min(self.commit_hash.len())];
        format!("  {}, {} • {} {}", self.author, age, short, self.summary)
    }
}

/// Human-readable, localized age from a Unix timestamp.
fn format_age(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now - timestamp).max(0) as u64;
    termide_i18n::relative_age(secs)
}

/// Run `git blame --porcelain` in a background thread.
///
/// Returns a receiver that yields `Vec<BlameEntry>` indexed by 0-based line
/// (entry at index 0 = line 1 in the file).  Returns an empty vec on failure.
pub fn get_blame_async(repo: PathBuf, file: PathBuf) -> mpsc::Receiver<Vec<BlameEntry>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = run_blame(&repo, &file).unwrap_or_default();
        let _ = tx.send(result);
    });
    rx
}

/// Parse `git blame --porcelain -- <file>` output into per-line blame entries.
fn run_blame(repo: &Path, file: &Path) -> Option<Vec<BlameEntry>> {
    let file_str = file.to_string_lossy();
    let output = git_command_stdout(repo, &["blame", "--porcelain", "--", &file_str])?;
    if output.trim().is_empty() {
        return None;
    }

    // Porcelain format: each hunk starts with a header line:
    //   "<hash> <orig_line> <final_line> <n_lines>"
    // Followed by optional tag lines like "author ...", "author-time ...", "summary ..."
    // Followed by a line of code (prefixed with '\t').
    // A hash that was already described earlier in the output omits the tag lines.

    // First pass: collect commit metadata keyed by hash.
    use std::collections::HashMap;
    let mut meta: HashMap<String, (String, i64, String)> = HashMap::new(); // hash -> (author, ts, summary)

    // Second pass will expand entries per final line.
    // We do a single-pass parse instead.

    let mut entries: Vec<BlameEntry> = Vec::new();

    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let header = lines[i];
        // Header: "<40-char-hash> <orig_line> <final_line> <n_lines>"
        let mut parts = header.splitn(4, ' ');
        let hash = match parts.next() {
            Some(h) if h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()) => h,
            _ => {
                i += 1;
                continue;
            }
        };
        let _orig_line = parts.next();
        let final_line: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        let n_lines: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);

        i += 1;

        // Collect optional tag lines until the '\t' code line.
        let mut author = String::new();
        let mut timestamp: i64 = 0;
        let mut summary = String::new();

        while i < lines.len() && !lines[i].starts_with('\t') {
            let tag_line = lines[i];
            if let Some(rest) = tag_line.strip_prefix("author ") {
                author = rest.to_string();
            } else if let Some(rest) = tag_line.strip_prefix("author-time ") {
                timestamp = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = tag_line.strip_prefix("summary ") {
                summary = rest.to_string();
            }
            i += 1;
        }

        // Skip the '\t' code line.
        if i < lines.len() && lines[i].starts_with('\t') {
            i += 1;
        }

        // If this hash was already seen, reuse cached metadata.
        if author.is_empty() {
            if let Some((a, ts, s)) = meta.get(hash) {
                author = a.clone();
                timestamp = *ts;
                summary = s.clone();
            }
        } else {
            meta.insert(
                hash.to_string(),
                (author.clone(), timestamp, summary.clone()),
            );
        }

        let entry = BlameEntry {
            commit_hash: hash.to_string(),
            author,
            timestamp,
            summary,
        };

        // Expand: this hunk covers n_lines starting at final_line (1-based).
        // Ensure the entries Vec is large enough.
        let end_line = final_line + n_lines; // exclusive, 1-based
        if entries.len() < end_line - 1 {
            entries.resize(end_line - 1, entry.clone());
        }
        for line_idx in (final_line - 1)..(end_line - 1) {
            if line_idx < entries.len() {
                entries[line_idx] = entry.clone();
            }
        }
    }

    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_age_just_now() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(format_age(now - 30), "just now");
    }

    #[test]
    fn test_format_age_minutes() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // Default (uninitialized) translation falls back to English.
        assert_eq!(format_age(now - 300), "5 minutes ago");
    }

    #[test]
    fn test_format_age_hours() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(format_age(now - 7200), "2 hours ago");
    }

    #[test]
    fn test_inline_text() {
        let entry = BlameEntry {
            commit_hash: "abc1234567890abc1234567890abc1234567890ab".to_string(),
            author: "nvn".to_string(),
            timestamp: 0,
            summary: "Fix bug".to_string(),
        };
        let text = entry.inline_text();
        assert!(text.contains("nvn"));
        assert!(text.contains("abc1234"));
        assert!(text.contains("Fix bug"));
    }
}
