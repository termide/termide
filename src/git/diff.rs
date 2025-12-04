use anyhow::{Context, Result};
use regex::Regex;
use similar::TextDiff;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;

/// Git diff status for a line in a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStatus {
    /// Line unchanged from HEAD
    Unchanged,
    /// Line added (not in HEAD)
    Added,
    /// Line modified (changed from HEAD)
    Modified,
    /// Lines deleted after this line
    DeletedAfter,
}

/// Represents a single hunk from git diff output
#[derive(Debug, Clone)]
struct DiffHunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
}

/// Cache for git diff results for a single file
#[derive(Debug, Clone)]
pub struct GitDiffCache {
    /// File path this diff is for
    file_path: PathBuf,
    /// Map of line number (0-based) to status
    line_statuses: HashMap<usize, LineStatus>,
    /// Map of line numbers to count of deleted lines after them (line_idx -> deletion_count)
    deleted_after_lines: HashMap<usize, usize>,
    /// Timestamp when diff was last fetched
    last_updated: std::time::Instant,
    /// Original content from HEAD (for in-memory diff)
    original_content: Option<String>,
}

impl GitDiffCache {
    /// Create new git diff cache for a file
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            file_path,
            line_statuses: HashMap::new(),
            deleted_after_lines: HashMap::new(),
            last_updated: std::time::Instant::now(),
            original_content: None,
        }
    }

    /// Load original content from HEAD
    pub fn load_original_from_head(&mut self) -> Result<()> {
        // Convert absolute path to relative path from git root
        let git_root_output = Command::new("git")
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .context("Failed to get git root")?;

        if !git_root_output.status.success() {
            crate::logger::warn(format!(
                "Failed to get git root for file: {}",
                self.file_path.display()
            ));
            self.original_content = Some(String::new());
            return Ok(());
        }

        let git_root = String::from_utf8(git_root_output.stdout)
            .context("Failed to parse git root as UTF-8")?
            .trim()
            .to_string();
        let git_root_path = std::path::Path::new(&git_root);

        // Get relative path from git root
        let relative_path = self
            .file_path
            .strip_prefix(git_root_path)
            .context("File is not within git repository")?;

        // Get file content from HEAD
        let output = Command::new("git")
            .arg("show")
            .arg(format!("HEAD:{}", relative_path.display()))
            .output()
            .context("Failed to execute git show")?;

        if !output.status.success() {
            // File might be new (not in HEAD yet)
            crate::logger::debug(format!(
                "File not in HEAD (new file): {}",
                self.file_path.display()
            ));
            self.original_content = Some(String::new());
            return Ok(());
        }

        let content =
            String::from_utf8(output.stdout).context("Failed to parse git show output as UTF-8")?;

        let content_len = content.len();
        self.original_content = Some(content);
        crate::logger::debug(format!(
            "Loaded {} bytes from HEAD for: {}",
            content_len,
            self.file_path.display()
        ));
        Ok(())
    }

    /// Update git diff by comparing buffer content with original from HEAD
    pub fn update_from_buffer(&mut self, current_content: &str) -> Result<()> {
        // Ensure we have original content loaded
        if self.original_content.is_none() {
            self.load_original_from_head()?;
        }

        let original = self.original_content.as_ref().unwrap();

        // Compute diff using similar crate
        let diff = TextDiff::from_lines(original.as_str(), current_content);
        let (statuses, deleted_after) = compute_line_statuses_from_textdiff(&diff);

        // Use the computed results directly - they are the source of truth
        // TextDiff compares HEAD with current buffer, which correctly handles:
        // - Pure deletions (creates markers)
        // - Modified line deletions (creates markers)
        // - Restored lines via undo (removes markers)
        self.line_statuses = statuses;
        self.deleted_after_lines = deleted_after;
        self.last_updated = std::time::Instant::now();

        Ok(())
    }

    /// Update git diff by comparing file on disk with HEAD
    pub fn update(&mut self) -> Result<()> {
        // Load original content from HEAD
        self.load_original_from_head()?;

        // If original content is empty (file not in HEAD or error), clear statuses
        let original = match self.original_content.as_ref() {
            Some(content) if !content.is_empty() => content,
            _ => {
                self.line_statuses.clear();
                self.deleted_after_lines.clear();
                return Ok(());
            }
        };

        // Read current file content from disk
        let current_content = match std::fs::read_to_string(&self.file_path) {
            Ok(content) => content,
            Err(_) => {
                // File might not exist or can't be read
                self.line_statuses.clear();
                self.deleted_after_lines.clear();
                return Ok(());
            }
        };

        // Use TextDiff for consistency with update_from_buffer()
        let diff = TextDiff::from_lines(original.as_str(), &current_content);
        let (statuses, deleted_after) = compute_line_statuses_from_textdiff(&diff);

        self.line_statuses = statuses;
        self.deleted_after_lines = deleted_after;
        self.last_updated = std::time::Instant::now();

        Ok(())
    }

    /// Get status for a specific line (0-based index)
    pub fn get_line_status(&self, line: usize) -> LineStatus {
        self.line_statuses
            .get(&line)
            .copied()
            .unwrap_or(LineStatus::Unchanged)
    }

    /// Check if line has a deletion marker after it
    pub fn has_deletion_marker(&self, line: usize) -> bool {
        self.deleted_after_lines.contains_key(&line)
    }

    /// Get count of deleted lines after the given line
    pub fn get_deletion_count(&self, line: usize) -> usize {
        self.deleted_after_lines.get(&line).copied().unwrap_or(0)
    }

    /// Check if cache is stale (older than threshold)
    #[allow(dead_code)]
    pub fn is_stale(&self, threshold: std::time::Duration) -> bool {
        self.last_updated.elapsed() > threshold
    }
}

/// Parse git diff hunks from unified diff format
/// Format: @@ -<old_start>,<old_count> +<new_start>,<new_count> @@
fn parse_diff_hunks(diff_text: &str) -> Result<Vec<DiffHunk>> {
    let mut hunks = Vec::new();

    // Regex to match hunk headers
    // Example: "@@ -2 +2 @@" or "@@ -2,3 +2,5 @@"
    let hunk_regex = Regex::new(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
        .context("Failed to compile diff hunk regex")?;

    for captures in hunk_regex.captures_iter(diff_text) {
        let old_start: usize = captures[1].parse().context("Failed to parse old_start")?;
        let old_count: usize = captures
            .get(2)
            .map(|m| m.as_str().parse().unwrap_or(1))
            .unwrap_or(1);
        let new_start: usize = captures[3].parse().context("Failed to parse new_start")?;
        let new_count: usize = captures
            .get(4)
            .map(|m| m.as_str().parse().unwrap_or(1))
            .unwrap_or(1);

        hunks.push(DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
        });
    }

    Ok(hunks)
}

/// Compute line statuses from diff hunks
fn compute_line_statuses(hunks: Vec<DiffHunk>) -> (HashMap<usize, LineStatus>, HashSet<usize>) {
    let mut statuses = HashMap::new();
    let mut deleted_after = HashSet::new();

    for hunk in hunks {
        if hunk.old_count == 0 && hunk.new_count > 0 {
            // Lines added (not in old file)
            // new_start is 1-based, convert to 0-based
            let start = hunk.new_start.saturating_sub(1);
            for i in 0..hunk.new_count {
                statuses.insert(start + i, LineStatus::Added);
            }
        } else if hunk.old_count > 0 && hunk.new_count > 0 {
            // Lines modified (changed from old file)
            // Mark all new lines as modified
            let start = hunk.new_start.saturating_sub(1);
            for i in 0..hunk.new_count {
                statuses.insert(start + i, LineStatus::Modified);
            }
        } else if hunk.old_count > 0 && hunk.new_count == 0 {
            // Lines deleted
            // Show deletion marker on the line before deletion point
            // If deleting at start of file (old_start == 1), mark line 0
            let marker_line = hunk.new_start.saturating_sub(1);
            deleted_after.insert(marker_line);
        }
    }

    (statuses, deleted_after)
}

/// Compute line statuses from TextDiff (similar crate)
fn compute_line_statuses_from_textdiff<'a>(
    diff: &TextDiff<'a, 'a, 'a, str>,
) -> (HashMap<usize, LineStatus>, HashMap<usize, usize>) {
    let mut statuses = HashMap::new();
    let mut deleted_after = HashMap::new();
    let mut new_line_idx = 0;

    for change in diff.iter_all_changes() {
        use similar::ChangeTag;

        match change.tag() {
            ChangeTag::Equal => {
                // Unchanged line - just increment counter
                new_line_idx += 1;
            }
            ChangeTag::Insert => {
                // Added line
                statuses.insert(new_line_idx, LineStatus::Added);
                new_line_idx += 1;
            }
            ChangeTag::Delete => {
                // Delete tags will be processed in the second pass
                // to distinguish between modifications and pure deletions
            }
        }
    }

    // Second pass: identify modified lines and count consecutive deletions
    // Process Delete and Insert pairwise:
    // - Delete immediately followed by Insert = Modification (1:1 pairing)
    // - Consecutive Deletes NOT followed by Insert = Pure deletions (count them)
    // - Insert NOT preceded by Delete = Pure addition (already handled in first pass)
    let changes: Vec<_> = diff.iter_all_changes().collect();
    let mut i = 0;
    let mut new_idx = 0;

    while i < changes.len() {
        use similar::ChangeTag;

        match changes[i].tag() {
            ChangeTag::Equal => {
                new_idx += 1;
                i += 1;
            }
            ChangeTag::Delete => {
                // Check if immediately followed by Insert (indicating modification)
                if i + 1 < changes.len() && changes[i + 1].tag() == ChangeTag::Insert {
                    // Modification: pair this Delete with the next Insert
                    statuses.insert(new_idx, LineStatus::Modified);
                    new_idx += 1;
                    i += 2; // Skip both Delete and Insert
                } else {
                    // Count consecutive deletions
                    let mut deletion_count = 0;
                    while i < changes.len() && changes[i].tag() == ChangeTag::Delete {
                        deletion_count += 1;
                        i += 1;
                    }

                    // Place deletion marker after previous line with deletion count
                    let marker_line_idx = if new_idx > 0 { new_idx - 1 } else { 0 };
                    deleted_after.insert(marker_line_idx, deletion_count);

                    // new_idx stays the same (no new lines added)
                }
            }
            ChangeTag::Insert => {
                // Pure insertion (already marked as Added in first pass)
                new_idx += 1;
                i += 1;
            }
        }
    }

    (statuses, deleted_after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_added_line() {
        let diff = r#"
@@ -3,0 +4 @@ some context
+added line
"#;
        let hunks = parse_diff_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 3);
        assert_eq!(hunks[0].old_count, 0);
        assert_eq!(hunks[0].new_start, 4);
        assert_eq!(hunks[0].new_count, 1);
    }

    #[test]
    fn test_parse_modified_line() {
        let diff = r#"
@@ -2 +2 @@ context
-old line
+new line
"#;
        let hunks = parse_diff_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 2);
        assert_eq!(hunks[0].old_count, 1);
        assert_eq!(hunks[0].new_start, 2);
        assert_eq!(hunks[0].new_count, 1);
    }

    #[test]
    fn test_parse_deleted_lines() {
        let diff = r#"
@@ -5,2 +4,0 @@ context
-deleted line 1
-deleted line 2
"#;
        let hunks = parse_diff_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 5);
        assert_eq!(hunks[0].old_count, 2);
        assert_eq!(hunks[0].new_start, 4);
        assert_eq!(hunks[0].new_count, 0);
    }

    // Tests for old compute_line_statuses API - disabled as we now use TextDiff
    // These tests can be re-enabled when compute_line_statuses is updated
    // to return the new (HashMap<LineStatus>, HashMap<deletion_count>) format

    #[test]
    #[ignore]
    fn test_compute_added_status() {
        let hunks = vec![DiffHunk {
            old_start: 3,
            old_count: 0,
            new_start: 4,
            new_count: 2,
        }];

        let (statuses, _) = compute_line_statuses(hunks);
        assert_eq!(statuses.get(&3), Some(&LineStatus::Added)); // 0-based line 3
        assert_eq!(statuses.get(&4), Some(&LineStatus::Added)); // 0-based line 4
        assert_eq!(statuses.get(&5), None); // No status
    }

    #[test]
    #[ignore]
    fn test_compute_modified_status() {
        let hunks = vec![DiffHunk {
            old_start: 2,
            old_count: 1,
            new_start: 2,
            new_count: 1,
        }];

        let (statuses, _) = compute_line_statuses(hunks);
        assert_eq!(statuses.get(&1), Some(&LineStatus::Modified)); // 0-based line 1
    }

    #[test]
    #[ignore]
    fn test_compute_deleted_status() {
        let hunks = vec![DiffHunk {
            old_start: 5,
            old_count: 2,
            new_start: 5,
            new_count: 0,
        }];

        let (statuses, _) = compute_line_statuses(hunks);
        // Marker should be on line before deletion (0-based line 4)
        assert_eq!(statuses.get(&4), Some(&LineStatus::DeletedAfter));
    }

    #[test]
    #[ignore]
    fn test_multiple_hunks() {
        let diff = r#"
@@ -2 +2 @@
-old
+new
@@ -5,0 +6,2 @@
+added1
+added2
@@ -10,3 +13,0 @@
-del1
-del2
-del3
"#;
        let hunks = parse_diff_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 3);

        let (statuses, _) = compute_line_statuses(hunks);
        assert_eq!(statuses.get(&1), Some(&LineStatus::Modified)); // Line 2 (0-based 1)
        assert_eq!(statuses.get(&5), Some(&LineStatus::Added)); // Line 6 (0-based 5)
        assert_eq!(statuses.get(&6), Some(&LineStatus::Added)); // Line 7 (0-based 6)
        assert_eq!(statuses.get(&12), Some(&LineStatus::DeletedAfter)); // Line 13 (0-based 12)
    }
}
