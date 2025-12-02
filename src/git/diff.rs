use anyhow::{Context, Result};
use regex::Regex;
use similar::TextDiff;
use std::collections::HashMap;
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
            last_updated: std::time::Instant::now(),
            original_content: None,
        }
    }

    /// Load original content from HEAD
    pub fn load_original_from_head(&mut self) -> Result<()> {
        // Get file path relative to git root
        let output = Command::new("git")
            .arg("show")
            .arg(format!("HEAD:{}", self.file_path.display()))
            .output()
            .context("Failed to execute git show")?;

        if !output.status.success() {
            // File might be new (not in HEAD yet)
            self.original_content = Some(String::new());
            return Ok(());
        }

        let content =
            String::from_utf8(output.stdout).context("Failed to parse git show output as UTF-8")?;

        self.original_content = Some(content);
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
        self.line_statuses = compute_line_statuses_from_textdiff(&diff);
        self.last_updated = std::time::Instant::now();

        Ok(())
    }

    /// Update git diff by running `git diff HEAD --unified=0 -- <file>`
    pub fn update(&mut self) -> Result<()> {
        // Run git diff against HEAD
        let output = Command::new("git")
            .arg("diff")
            .arg("HEAD")
            .arg("--unified=0")
            .arg("--")
            .arg(&self.file_path)
            .output()
            .context("Failed to execute git diff")?;

        // If git command failed, file might not be in a repo or not tracked
        if !output.status.success() {
            self.line_statuses.clear();
            return Ok(());
        }

        // Parse diff output
        let diff_text =
            String::from_utf8(output.stdout).context("Failed to parse git diff output as UTF-8")?;

        let hunks = parse_diff_hunks(&diff_text)?;
        self.line_statuses = compute_line_statuses(hunks);
        self.last_updated = std::time::Instant::now();

        // Also load original content for future in-memory diffs
        let _ = self.load_original_from_head();

        Ok(())
    }

    /// Get status for a specific line (0-based index)
    pub fn get_line_status(&self, line: usize) -> LineStatus {
        self.line_statuses
            .get(&line)
            .copied()
            .unwrap_or(LineStatus::Unchanged)
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
fn compute_line_statuses(hunks: Vec<DiffHunk>) -> HashMap<usize, LineStatus> {
    let mut statuses = HashMap::new();

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
            // Show marker on the line before deletion point
            // If deleting at start of file (old_start == 1), mark line 0
            let marker_line = hunk.new_start.saturating_sub(1);
            statuses.insert(marker_line, LineStatus::DeletedAfter);
        }
    }

    statuses
}

/// Compute line statuses from TextDiff (similar crate)
fn compute_line_statuses_from_textdiff<'a>(
    diff: &TextDiff<'a, 'a, 'a, str>,
) -> HashMap<usize, LineStatus> {
    let mut statuses = HashMap::new();
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
                // Deleted line - mark the current position
                // If we're at the beginning, mark line 0
                // Otherwise mark the previous line
                if new_line_idx > 0 {
                    statuses.insert(new_line_idx - 1, LineStatus::DeletedAfter);
                } else {
                    statuses.insert(0, LineStatus::DeletedAfter);
                }
            }
        }
    }

    // Second pass: identify modified lines
    // A modified line is when we have Delete followed by Insert at same position
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
                // Check if next change is Insert (indicating modification)
                let mut delete_count = 0;
                let mut j = i;
                while j < changes.len() && changes[j].tag() == ChangeTag::Delete {
                    delete_count += 1;
                    j += 1;
                }

                let mut insert_count = 0;
                let mut k = j;
                while k < changes.len() && changes[k].tag() == ChangeTag::Insert {
                    insert_count += 1;
                    k += 1;
                }

                if insert_count > 0 && delete_count > 0 {
                    // This is a modification
                    // Mark the inserted lines as Modified instead of Added
                    for offset in 0..insert_count {
                        statuses.insert(new_idx + offset, LineStatus::Modified);
                    }
                    new_idx += insert_count;
                    i = k;
                } else if insert_count > 0 {
                    // Pure insertion (already marked as Added)
                    new_idx += insert_count;
                    i = k;
                } else {
                    // Pure deletion (already marked)
                    i = j;
                }
            }
            ChangeTag::Insert => {
                new_idx += 1;
                i += 1;
            }
        }
    }

    statuses
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

    #[test]
    fn test_compute_added_status() {
        let hunks = vec![DiffHunk {
            old_start: 3,
            old_count: 0,
            new_start: 4,
            new_count: 2,
        }];

        let statuses = compute_line_statuses(hunks);
        assert_eq!(statuses.get(&3), Some(&LineStatus::Added)); // 0-based line 3
        assert_eq!(statuses.get(&4), Some(&LineStatus::Added)); // 0-based line 4
        assert_eq!(statuses.get(&5), None); // No status
    }

    #[test]
    fn test_compute_modified_status() {
        let hunks = vec![DiffHunk {
            old_start: 2,
            old_count: 1,
            new_start: 2,
            new_count: 1,
        }];

        let statuses = compute_line_statuses(hunks);
        assert_eq!(statuses.get(&1), Some(&LineStatus::Modified)); // 0-based line 1
    }

    #[test]
    fn test_compute_deleted_status() {
        let hunks = vec![DiffHunk {
            old_start: 5,
            old_count: 2,
            new_start: 5,
            new_count: 0,
        }];

        let statuses = compute_line_statuses(hunks);
        // Marker should be on line before deletion (0-based line 4)
        assert_eq!(statuses.get(&4), Some(&LineStatus::DeletedAfter));
    }

    #[test]
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

        let statuses = compute_line_statuses(hunks);
        assert_eq!(statuses.get(&1), Some(&LineStatus::Modified)); // Line 2 (0-based 1)
        assert_eq!(statuses.get(&5), Some(&LineStatus::Added)); // Line 6 (0-based 5)
        assert_eq!(statuses.get(&6), Some(&LineStatus::Added)); // Line 7 (0-based 6)
        assert_eq!(statuses.get(&12), Some(&LineStatus::DeletedAfter)); // Line 13 (0-based 12)
    }
}
