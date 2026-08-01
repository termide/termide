use anyhow::Result;
use similar::TextDiff;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

/// Result of async git diff load operation
#[derive(Debug)]
pub struct GitDiffAsyncResult {
    /// File path this result is for
    pub file_path: PathBuf,
    /// Original content from HEAD (None if error or file not in HEAD)
    pub original_content: Option<String>,
}

/// Load original content from HEAD in a background thread
/// Returns a receiver that will receive the result
pub fn load_original_async(file_path: PathBuf) -> mpsc::Receiver<GitDiffAsyncResult> {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let original_content = load_original_from_head_sync(&file_path);
        let _ = tx.send(GitDiffAsyncResult {
            file_path,
            original_content,
        });
    });

    rx
}

/// Synchronous function to load original content from HEAD
/// Extracted for use in background thread
fn load_original_from_head_sync(file_path: &std::path::Path) -> Option<String> {
    crate::command::head_file_content(file_path)
}

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

/// Type of inline (character-level) change within a line
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineChangeType {
    /// Text unchanged
    Unchanged,
    /// Text deleted (from original, show with red background)
    Deleted,
    /// Text inserted (in current, show with green background)
    Inserted,
}

/// A segment of inline change within a modified line
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineChange {
    /// Type of change
    pub change_type: InlineChangeType,
    /// The text content
    pub text: String,
}

/// Compute character-level diff between two lines
/// Returns a list of InlineChange segments
pub fn compute_inline_diff(original: &str, current: &str) -> Vec<InlineChange> {
    use similar::ChangeTag;

    let diff = TextDiff::from_chars(original, current);
    let mut changes = Vec::new();

    // Collect all changes, merging consecutive same-type changes
    let mut current_type: Option<InlineChangeType> = None;
    let mut current_text = String::new();

    for change in diff.iter_all_changes() {
        let change_type = match change.tag() {
            ChangeTag::Equal => InlineChangeType::Unchanged,
            ChangeTag::Delete => InlineChangeType::Deleted,
            ChangeTag::Insert => InlineChangeType::Inserted,
        };

        if Some(change_type) == current_type {
            // Same type, append to current segment
            current_text.push_str(change.value());
        } else {
            // Different type, save previous segment and start new one
            if let Some(ct) = current_type {
                if !current_text.is_empty() {
                    changes.push(InlineChange {
                        change_type: ct,
                        text: std::mem::take(&mut current_text),
                    });
                }
            }
            current_type = Some(change_type);
            current_text = change.value().to_string();
        }
    }

    // Don't forget the last segment
    if let Some(ct) = current_type {
        if !current_text.is_empty() {
            changes.push(InlineChange {
                change_type: ct,
                text: current_text,
            });
        }
    }

    changes
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
    /// Mapping of current line index -> original line index (for Modified lines)
    modified_line_mapping: HashMap<usize, usize>,
    /// Timestamp when diff was last fetched
    last_updated: std::time::Instant,
    /// Original content from HEAD (for in-memory diff)
    original_content: Option<String>,
}

impl GitDiffCache {
    /// Create new git diff cache for a file
    pub fn new(file_path: PathBuf) -> Self {
        // Canonicalize to resolve symlinks — git rev-parse returns canonical paths,
        // so file_path must match for strip_prefix to work.
        let file_path = std::fs::canonicalize(&file_path).unwrap_or(file_path);
        Self {
            file_path,
            line_statuses: HashMap::new(),
            deleted_after_lines: HashMap::new(),
            modified_line_mapping: HashMap::new(),
            last_updated: std::time::Instant::now(),
            original_content: None,
        }
    }

    /// Load original content from HEAD. Never errors: a file outside a repo,
    /// gitignored, or unreadable simply yields no original (and thus no diff
    /// markers). Returns `Result` for call-site ergonomics.
    pub fn load_original_from_head(&mut self) -> Result<()> {
        self.original_content = crate::command::head_file_content(&self.file_path);
        Ok(())
    }

    /// Seed the HEAD-side content directly instead of reading it from git.
    /// Lets callers compute an in-memory diff via [`Self::update_from_buffer`]
    /// without invoking git (also used to set up deterministic tests).
    pub fn set_original_content(&mut self, content: Option<String>) {
        self.original_content = content;
    }

    /// Update git diff by comparing buffer content with original from HEAD
    pub fn update_from_buffer(&mut self, current_content: &str) -> Result<()> {
        // Ensure we have original content loaded
        if self.original_content.is_none() {
            self.load_original_from_head()?;
        }

        let original = match self.original_content.as_deref() {
            Some(content) => content,
            None => {
                // Not in a git repo — no git markers
                self.line_statuses.clear();
                self.deleted_after_lines.clear();
                self.modified_line_mapping.clear();
                self.last_updated = std::time::Instant::now();
                return Ok(());
            }
        };
        // original may be "" for files not yet committed — all lines show as Added

        // Compute diff using similar crate
        let diff = TextDiff::from_lines(original, current_content);
        let result = compute_line_statuses_from_textdiff(&diff);

        // Use the computed results directly - they are the source of truth
        // TextDiff compares HEAD with current buffer, which correctly handles:
        // - Pure deletions (creates markers)
        // - Modified line deletions (creates markers)
        // - Restored lines via undo (removes markers)
        self.line_statuses = result.statuses;
        self.deleted_after_lines = result.deleted_after;
        self.modified_line_mapping = result.modified_mapping;
        self.last_updated = std::time::Instant::now();

        Ok(())
    }

    /// Update git diff by comparing file on disk with HEAD
    pub fn update(&mut self) -> Result<()> {
        // Load original content from HEAD
        self.load_original_from_head()?;

        // If not in a git repo, clear statuses; empty original means new file (show all as Added)
        let original = match self.original_content.as_deref() {
            Some(content) => content,
            None => {
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
        let diff = TextDiff::from_lines(original, &current_content);
        let result = compute_line_statuses_from_textdiff(&diff);

        self.line_statuses = result.statuses;
        self.deleted_after_lines = result.deleted_after;
        self.modified_line_mapping = result.modified_mapping;
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

    /// Get total number of lines with deletion markers.
    ///
    /// This is O(1) - simply returns the number of entries in the map.
    /// Used for virtual line count calculation.
    pub fn deletion_marker_count(&self) -> usize {
        self.deleted_after_lines.len()
    }

    /// Get original line content from HEAD by original line index
    /// Returns None if original content is not loaded or line doesn't exist
    fn get_original_line_by_idx(&self, original_idx: usize) -> Option<&str> {
        let original = self.original_content.as_ref()?;
        original.lines().nth(original_idx)
    }

    /// Compute inline diff for a modified line
    /// Returns None if line is not modified or original is not available
    pub fn get_inline_diff(
        &self,
        line_idx: usize,
        current_text: &str,
    ) -> Option<Vec<InlineChange>> {
        // Only compute for modified lines
        if self.get_line_status(line_idx) != LineStatus::Modified {
            return None;
        }

        // Get the original line index from the mapping
        let original_idx = self.modified_line_mapping.get(&line_idx)?;
        let original_line = self.get_original_line_by_idx(*original_idx)?;
        Some(compute_inline_diff(original_line, current_text))
    }

    /// Apply async result and recompute diff
    /// Called when background thread completes loading original content
    pub fn apply_async_result(&mut self, async_result: GitDiffAsyncResult) {
        // Store original content
        self.original_content = async_result.original_content;

        // Recompute diff if we have original content
        let original = match self.original_content.as_deref() {
            Some(content) => content,
            None => {
                self.line_statuses.clear();
                self.deleted_after_lines.clear();
                self.modified_line_mapping.clear();
                return;
            }
        };
        // original may be "" for files not yet committed — all lines show as Added

        // Read current file content from disk
        let current_content = match std::fs::read_to_string(&self.file_path) {
            Ok(content) => content,
            Err(_) => {
                self.line_statuses.clear();
                self.deleted_after_lines.clear();
                self.modified_line_mapping.clear();
                return;
            }
        };

        // Compute diff
        let diff = TextDiff::from_lines(original, &current_content);
        let result = compute_line_statuses_from_textdiff(&diff);

        self.line_statuses = result.statuses;
        self.deleted_after_lines = result.deleted_after;
        self.modified_line_mapping = result.modified_mapping;
        self.last_updated = std::time::Instant::now();
    }
}

/// Result of computing line statuses from diff
struct DiffResult {
    statuses: HashMap<usize, LineStatus>,
    deleted_after: HashMap<usize, usize>,
    modified_mapping: HashMap<usize, usize>,
}

/// Compute line statuses from TextDiff (similar crate)
fn compute_line_statuses_from_textdiff<'a>(diff: &TextDiff<'a, 'a, 'a, str>) -> DiffResult {
    let mut statuses = HashMap::new();
    let mut deleted_after = HashMap::new();
    let mut modified_mapping = HashMap::new();

    // Process changes tracking both old and new line indices
    let changes: Vec<_> = diff.iter_all_changes().collect();
    let mut i = 0;
    let mut new_idx = 0;
    let mut old_idx = 0;

    while i < changes.len() {
        use similar::ChangeTag;

        match changes[i].tag() {
            ChangeTag::Equal => {
                // Unchanged line - increment both counters
                new_idx += 1;
                old_idx += 1;
                i += 1;
            }
            ChangeTag::Delete => {
                // Count consecutive deletions
                let mut delete_count = 0;
                let delete_start_old = old_idx;
                while i + delete_count < changes.len()
                    && changes[i + delete_count].tag() == ChangeTag::Delete
                {
                    delete_count += 1;
                }

                // Count consecutive inserts immediately after the deletions
                let mut insert_count = 0;
                while i + delete_count + insert_count < changes.len()
                    && changes[i + delete_count + insert_count].tag() == ChangeTag::Insert
                {
                    insert_count += 1;
                }

                // Pair deletes with inserts as Modified lines
                let paired = delete_count.min(insert_count);
                for p in 0..paired {
                    statuses.insert(new_idx, LineStatus::Modified);
                    modified_mapping.insert(new_idx, delete_start_old + p);
                    new_idx += 1;
                    old_idx += 1;
                }

                // Remaining unpaired deletes → deletion marker
                let unpaired_deletes = delete_count - paired;
                if unpaired_deletes > 0 {
                    old_idx += unpaired_deletes;
                    let marker_line_idx = if new_idx > 0 { new_idx - 1 } else { 0 };
                    deleted_after.insert(marker_line_idx, unpaired_deletes);
                }

                // Remaining unpaired inserts → Added lines
                let unpaired_inserts = insert_count - paired;
                for _ in 0..unpaired_inserts {
                    statuses.insert(new_idx, LineStatus::Added);
                    new_idx += 1;
                }

                i += delete_count + insert_count;
            }
            ChangeTag::Insert => {
                // Pure insertion - only new index increments
                statuses.insert(new_idx, LineStatus::Added);
                new_idx += 1;
                i += 1;
            }
        }
    }

    DiffResult {
        statuses,
        deleted_after,
        modified_mapping,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_line_statuses_from_textdiff_added() {
        let original = "line1\nline2\nline3\n";
        let current = "line1\nline2\nnew line\nline3\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // Line at index 2 should be Added (0-indexed: "new line")
        assert_eq!(result.statuses.get(&2), Some(&LineStatus::Added));
        // No deletions
        assert!(result.deleted_after.is_empty());
    }

    #[test]
    fn test_compute_line_statuses_from_textdiff_modified() {
        let original = "line1\noriginal line\nline3\n";
        let current = "line1\nmodified line\nline3\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // Line at index 1 should be Modified (0-indexed)
        assert_eq!(result.statuses.get(&1), Some(&LineStatus::Modified));
        assert!(result.deleted_after.is_empty());
        // Mapping should show new_idx 1 -> old_idx 1
        assert_eq!(result.modified_mapping.get(&1), Some(&1));
    }

    #[test]
    fn test_compute_line_statuses_from_textdiff_deleted() {
        let original = "line1\ndeleted line\nline3\n";
        let current = "line1\nline3\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // Deletion marker should be after line 0 (where line1 is)
        assert!(result.deleted_after.contains_key(&0));
        assert_eq!(result.deleted_after.get(&0), Some(&1)); // 1 line deleted
        assert!(result.statuses.is_empty()); // No modifications or additions
    }

    #[test]
    fn test_compute_line_statuses_from_textdiff_multiple_deletions() {
        let original = "line1\ndeleted1\ndeleted2\ndeleted3\nline5\n";
        let current = "line1\nline5\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // 3 lines deleted after line 0
        assert!(result.deleted_after.contains_key(&0));
        assert_eq!(result.deleted_after.get(&0), Some(&3));
        assert!(result.statuses.is_empty()); // No modifications or additions
    }

    #[test]
    fn test_compute_line_statuses_from_textdiff_mixed_changes() {
        let original = "line1\nold\nline3\n";
        let current = "line1\nnew\nnew2\nline3\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // Line 1 modified (old -> new), line 2 added (new2)
        assert_eq!(result.statuses.get(&1), Some(&LineStatus::Modified));
        assert_eq!(result.statuses.get(&2), Some(&LineStatus::Added));
        assert!(result.deleted_after.is_empty());
        // Modified line mapping: new_idx 1 -> old_idx 1
        assert_eq!(result.modified_mapping.get(&1), Some(&1));
    }

    #[test]
    fn test_modified_line_mapping_preserves_index() {
        // Test that mapping works correctly when lines are modified in place
        let original = "line1\noriginal line 2\nline3\noriginal line 4\n";
        let current = "line1\nmodified line 2\nline3\nmodified line 4\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // Lines 1 and 3 are modified (0-indexed)
        assert_eq!(result.statuses.get(&1), Some(&LineStatus::Modified));
        assert_eq!(result.statuses.get(&3), Some(&LineStatus::Modified));
        // Mapping should preserve indices since no lines added/deleted
        assert_eq!(result.modified_mapping.get(&1), Some(&1));
        assert_eq!(result.modified_mapping.get(&3), Some(&3));
    }

    #[test]
    fn test_compute_inline_diff_insertion() {
        let original = "Hello world";
        let current = "Hello beautiful world";

        let changes = compute_inline_diff(original, current);

        // Should be: Unchanged("Hello "), Inserted("beautiful "), Unchanged("world")
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].change_type, InlineChangeType::Unchanged);
        assert_eq!(changes[0].text, "Hello ");
        assert_eq!(changes[1].change_type, InlineChangeType::Inserted);
        assert_eq!(changes[1].text, "beautiful ");
        assert_eq!(changes[2].change_type, InlineChangeType::Unchanged);
        assert_eq!(changes[2].text, "world");
    }

    #[test]
    fn test_compute_inline_diff_deletion() {
        let original = "Hello beautiful world";
        let current = "Hello world";

        let changes = compute_inline_diff(original, current);

        // Should be: Unchanged("Hello "), Deleted("beautiful "), Unchanged("world")
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].change_type, InlineChangeType::Unchanged);
        assert_eq!(changes[0].text, "Hello ");
        assert_eq!(changes[1].change_type, InlineChangeType::Deleted);
        assert_eq!(changes[1].text, "beautiful ");
        assert_eq!(changes[2].change_type, InlineChangeType::Unchanged);
        assert_eq!(changes[2].text, "world");
    }

    #[test]
    fn test_compute_inline_diff_replacement() {
        let original = "let x = 5";
        let current = "let x = 10";

        let changes = compute_inline_diff(original, current);

        // Should have: Unchanged("let x = "), Deleted("5"), Inserted("10")
        assert!(changes.len() >= 3);
        // Find the deleted and inserted parts
        let deleted: Vec<_> = changes
            .iter()
            .filter(|c| c.change_type == InlineChangeType::Deleted)
            .collect();
        let inserted: Vec<_> = changes
            .iter()
            .filter(|c| c.change_type == InlineChangeType::Inserted)
            .collect();

        assert!(!deleted.is_empty());
        assert!(!inserted.is_empty());
    }

    #[test]
    fn test_compute_inline_diff_same_text() {
        let text = "Hello world";

        let changes = compute_inline_diff(text, text);

        // Should be single unchanged segment
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, InlineChangeType::Unchanged);
        assert_eq!(changes[0].text, text);
    }

    #[test]
    fn test_multiple_consecutive_modified_lines() {
        let original = "line1\noriginal A\noriginal B\noriginal C\nline5\n";
        let current = "line1\nmodified A\nmodified B\nmodified C\nline5\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        // All 3 lines should be Modified, not DeletedAfter + Added
        assert_eq!(result.statuses.get(&1), Some(&LineStatus::Modified));
        assert_eq!(result.statuses.get(&2), Some(&LineStatus::Modified));
        assert_eq!(result.statuses.get(&3), Some(&LineStatus::Modified));
        assert!(result.deleted_after.is_empty());
    }

    #[test]
    fn test_more_deletes_than_inserts() {
        // 3 lines replaced by 1 line: 3 Del + 1 Ins → 1 Modified + 2 deleted
        let original = "line1\nold A\nold B\nold C\nline5\n";
        let current = "line1\nnew A\nline5\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        assert_eq!(result.statuses.get(&1), Some(&LineStatus::Modified));
        // 2 unpaired deletions after Modified line (index 1)
        assert_eq!(result.deleted_after.get(&1), Some(&2));
    }

    #[test]
    fn test_more_inserts_than_deletes() {
        // 1 line replaced by 3 lines: 1 Del + 3 Ins → 1 Modified + 2 Added
        let original = "line1\nold A\nline3\n";
        let current = "line1\nnew A\nnew B\nnew C\nline3\n";

        let diff = similar::TextDiff::from_lines(original, current);
        let result = compute_line_statuses_from_textdiff(&diff);

        assert_eq!(result.statuses.get(&1), Some(&LineStatus::Modified));
        assert_eq!(result.statuses.get(&2), Some(&LineStatus::Added));
        assert_eq!(result.statuses.get(&3), Some(&LineStatus::Added));
        assert!(result.deleted_after.is_empty());
    }

    #[test]
    fn test_update_from_buffer_empty_original_marks_all_added() {
        use std::path::PathBuf;

        // Simulate a new/untracked file: original_content is empty string
        let mut cache = GitDiffCache {
            file_path: PathBuf::from("/tmp/test_new_file.rs"),
            original_content: Some(String::new()),
            line_statuses: HashMap::new(),
            deleted_after_lines: HashMap::new(),
            modified_line_mapping: HashMap::new(),
            last_updated: std::time::Instant::now(),
        };

        let result = cache.update_from_buffer("line1\nline2\n");
        assert!(result.is_ok());
        // All lines in an untracked file should show as Added
        assert_eq!(
            cache.line_statuses.get(&0),
            Some(&LineStatus::Added),
            "First line should be Added for untracked file"
        );
        assert_eq!(
            cache.line_statuses.get(&1),
            Some(&LineStatus::Added),
            "Second line should be Added for untracked file"
        );
        assert!(cache.deleted_after_lines.is_empty());
        assert!(cache.modified_line_mapping.is_empty());
    }
}
