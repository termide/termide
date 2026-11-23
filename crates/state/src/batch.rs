//! Batch file operation types.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Local};

/// Message about background directory size calculation result
#[derive(Debug)]
pub struct DirSizeResult {
    pub size: u64,
    /// Immediate (non-recursive) child count of the directory, when known.
    pub item_count: Option<usize>,
}

/// Batch operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOperationType {
    Copy,
    Move,
}

/// Automatic conflict resolution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictMode {
    /// Ask each time
    Ask,
    /// Automatically overwrite all
    OverwriteAll,
    /// Automatically skip all
    SkipAll,
}

/// File rename pattern
#[derive(Debug, Clone)]
pub struct RenamePattern {
    template: String,
}

impl RenamePattern {
    /// Create new rename pattern
    pub fn new(template: String) -> Self {
        Self { template }
    }

    /// Apply pattern to filename.
    ///
    /// The template is read once, left to right, so text a placeholder puts
    /// in — a name holding `$I` — is never expanded again. `$0` is the whole
    /// name, `$1`..`$9` its dot-separated parts from the left, `$-1`..`$-9`
    /// from the right (empty past the last part), `$I` the counter, `$C`
    /// and `$M` the creation and modification times. Any other `$` is kept.
    pub fn apply(
        &self,
        original_name: &str,
        counter: usize,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> String {
        let parts: Vec<&str> = original_name.split('.').collect();
        let digit = |c: Option<char>| c.and_then(|c| c.to_digit(10)).map(|d| d as usize);
        let mut result = String::with_capacity(self.template.len() + original_name.len());
        let mut rest = self.template.as_str();
        while let Some(at) = rest.find('$') {
            result.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let mut chars = after.chars();
            let first = chars.next();
            // (expansion, bytes of the template consumed after `$`)
            let expanded: Option<(String, usize)> = match first {
                Some('0') => Some((original_name.to_string(), 1)),
                Some('I') => Some((counter.to_string(), 1)),
                Some('C') => Some((created.map(Self::format_time).unwrap_or_default(), 1)),
                Some('M') => Some((modified.map(Self::format_time).unwrap_or_default(), 1)),
                Some('-') => match digit(chars.next()) {
                    Some(n) if n > 0 => {
                        let part = parts
                            .len()
                            .checked_sub(n)
                            .and_then(|i| parts.get(i))
                            .copied()
                            .unwrap_or("");
                        Some((part.to_string(), 2))
                    }
                    _ => None,
                },
                c => digit(c).map(|n| (parts.get(n - 1).copied().unwrap_or("").to_string(), 1)),
            };
            match expanded {
                Some((text, used)) => {
                    result.push_str(&text);
                    rest = &after[used..];
                }
                None => {
                    result.push('$');
                    rest = after;
                }
            }
        }
        result.push_str(rest);
        result
    }

    /// Format time to YYYYMMDD_HHMMSS string
    fn format_time(time: SystemTime) -> String {
        let datetime: DateTime<Local> = time.into();
        datetime.format("%Y%m%d_%H%M%S").to_string()
    }

    /// Get preview result for example
    pub fn preview(&self, example_name: &str) -> String {
        self.apply(example_name, 1, None, None)
    }

    /// Check if result contains forbidden characters
    pub fn is_valid_result(&self, result: &str) -> bool {
        // Forbidden characters in filenames
        let forbidden = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
        !result.is_empty() && !result.chars().any(|c| forbidden.contains(&c))
    }
}

/// Pause state for batch operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseState {
    /// Operation is running
    Running,
    /// Operation is paused by user
    Paused,
}

/// Where the sources of a batch operation live.
#[derive(Debug, Clone, Default)]
pub enum SourceLocation {
    /// Not recorded, as for a clipboard paste: a source that does not exist
    /// locally is taken from the panel showing a remote location.
    #[default]
    Unknown,
    /// The local filesystem.
    Local,
    /// A VFS location (a remote host or an archive); the sources are paths
    /// on it, read through `manager`.
    Vfs {
        manager: Arc<termide_vfs::VfsManager>,
        directory: termide_vfs::VfsPath,
    },
}

/// Batch file operation with conflict support
#[derive(Debug, Clone)]
pub struct BatchOperation {
    /// Operation type
    pub operation_type: BatchOperationType,
    /// List of files to process
    pub sources: Vec<PathBuf>,
    /// Target directory
    pub destination: PathBuf,
    /// Whether the original user input explicitly marked destination as a directory
    pub destination_is_directory: bool,
    /// Current index of file being processed
    pub current_index: usize,
    /// Conflict resolution mode
    pub conflict_mode: ConflictMode,
    /// Rename pattern for RenameAll
    pub rename_pattern: Option<RenamePattern>,
    /// Counter for $I variable in pattern
    pub rename_counter: usize,
    /// The name picked with Rename for the current item when it is taken
    /// too: the conflict dialog then asks about it, and Overwrite replaces
    /// the file under it. Dropped when the batch moves on.
    pub picked_name: Option<String>,
    /// Statistics: successfully processed
    pub success_count: usize,
    /// Statistics: errors
    pub error_count: usize,
    /// Statistics: skipped
    pub skipped_count: usize,
    /// Pause state for batch operation
    pub pause_state: PauseState,
    /// What a copy batch created and finished — what "Delete copied" removes
    /// after a cancel. Only entries the batch brought into being: nothing
    /// that existed before, nothing a move left (its source is gone).
    pub completed_destinations: Vec<PathBuf>,
    /// Where the item in flight lands, when the batch creates it there;
    /// recorded into `completed_destinations` once it completes.
    pub item_destination: Option<PathBuf>,
    /// Cumulative files completed from previous batch items (for multi-folder downloads)
    pub cumulative_files_completed: usize,
    /// Cumulative bytes completed from previous batch items
    pub cumulative_bytes_completed: u64,
    /// Total files across all batch items (when known)
    pub cumulative_total_files: usize,
    /// Total bytes across all batch items (when known)
    pub cumulative_total_bytes: u64,
    /// Where `sources` live.
    pub source_location: SourceLocation,
}

impl BatchOperation {
    /// Create new batch operation
    pub fn new(
        operation_type: BatchOperationType,
        sources: Vec<PathBuf>,
        destination: PathBuf,
    ) -> Self {
        Self {
            operation_type,
            sources,
            destination,
            destination_is_directory: false,
            current_index: 0,
            conflict_mode: ConflictMode::Ask,
            rename_pattern: None,
            rename_counter: 1,
            picked_name: None,
            success_count: 0,
            error_count: 0,
            skipped_count: 0,
            pause_state: PauseState::Running,
            completed_destinations: Vec::new(),
            item_destination: None,
            cumulative_files_completed: 0,
            cumulative_bytes_completed: 0,
            cumulative_total_files: 0,
            cumulative_total_bytes: 0,
            source_location: SourceLocation::Unknown,
        }
    }

    /// Record where the sources live.
    pub fn with_source_location(mut self, source_location: SourceLocation) -> Self {
        self.source_location = source_location;
        self
    }

    /// Whether `source` has to be read through the VFS. Only an unrecorded
    /// location falls back to guessing from local existence, which misreads
    /// a remote or archive path that also exists locally (`/etc`, `/usr`).
    pub fn source_is_remote(&self, source: &Path) -> bool {
        match self.source_location {
            // Not `exists`: a dangling local symlink is still local.
            SourceLocation::Unknown => std::fs::symlink_metadata(source).is_err(),
            SourceLocation::Local => false,
            SourceLocation::Vfs { .. } => true,
        }
    }

    /// Mark that the original destination input explicitly referred to a directory.
    pub fn with_destination_directory(mut self, destination_is_directory: bool) -> Self {
        self.destination_is_directory = destination_is_directory;
        self
    }

    /// Add a successfully completed destination path
    pub fn add_completed_destination(&mut self, path: PathBuf) {
        self.completed_destinations.push(path);
    }

    /// Note the exact local path the current item is written to, as the
    /// sub-operation is started. It is kept for cancel cleanup only when the
    /// batch copies and nothing is there yet: an overwrite or a merge into
    /// an existing directory would otherwise offer pre-existing data for
    /// deletion, and a move has no copy to delete.
    pub fn note_item_destination(&mut self, dest: &Path) {
        let fresh = self.operation_type == BatchOperationType::Copy
            && std::fs::symlink_metadata(dest).is_err();
        self.item_destination = fresh.then(|| dest.to_path_buf());
    }

    /// The current item completed: keep what `note_item_destination` noted.
    pub fn record_item_destination(&mut self) {
        if let Some(dest) = self.item_destination.take() {
            self.completed_destinations.push(dest);
        }
    }

    /// Set rename pattern
    pub fn set_rename_pattern(&mut self, pattern: RenamePattern) {
        self.rename_pattern = Some(pattern);
    }

    /// Get and increment rename counter
    pub fn get_and_increment_rename_counter(&mut self) -> usize {
        let counter = self.rename_counter;
        self.rename_counter += 1;
        counter
    }

    /// Get current file being processed
    pub fn current_source(&self) -> Option<&PathBuf> {
        self.sources.get(self.current_index)
    }

    /// Check if operation is complete
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.sources.len()
    }

    /// Advance to next file
    pub fn advance(&mut self) {
        // A note left by an item that failed or was skipped must not be
        // taken for the next item's destination.
        self.item_destination = None;
        self.picked_name = None;
        self.current_index += 1;
    }

    /// Total number of files
    pub fn total_count(&self) -> usize {
        self.sources.len()
    }

    /// Set conflict resolution mode
    pub fn set_conflict_mode(&mut self, mode: ConflictMode) {
        self.conflict_mode = mode;
    }

    /// Increment success counter
    pub fn increment_success(&mut self) {
        self.success_count += 1;
    }

    /// Increment error counter
    pub fn increment_error(&mut self) {
        self.error_count += 1;
    }

    /// Increment skipped counter
    pub fn increment_skipped(&mut self) {
        self.skipped_count += 1;
    }

    /// Get the last successfully processed source filename
    /// Returns the filename of the file at current_index - 1 if available
    pub fn last_successful_filename(&self) -> Option<String> {
        if self.current_index == 0 || self.success_count == 0 {
            return None;
        }

        // Get the file that was just processed (current_index - 1)
        self.sources
            .get(self.current_index.saturating_sub(1))
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|s| s.to_string())
    }

    /// Get destination path reference
    pub fn destination_path(&self) -> &PathBuf {
        &self.destination
    }

    /// Whether the destination should be treated as a directory.
    pub fn destination_is_directory(&self) -> bool {
        self.destination_is_directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path inside an archive or on a server can also exist locally
    /// (`/etc` in a rootfs tarball); a recorded VFS location must win over
    /// that, or the local directory would be copied instead.
    #[test]
    fn a_recorded_location_decides_where_a_source_is_read_from() {
        let exists_locally = Path::new("/");
        let missing_locally = Path::new("/definitely/not/here/42");
        let op = |location| {
            BatchOperation::new(BatchOperationType::Copy, vec![], PathBuf::from("/tmp"))
                .with_source_location(location)
        };

        let vfs = op(SourceLocation::Vfs {
            manager: Arc::new(termide_vfs::VfsManager::new()),
            directory: termide_vfs::VfsPath::archive(
                termide_vfs::VfsPath::local("/x/rootfs.tar"),
                "/",
            ),
        });
        assert!(vfs.source_is_remote(exists_locally));

        let local = op(SourceLocation::Local);
        assert!(!local.source_is_remote(missing_locally));

        let unknown = op(SourceLocation::Unknown);
        assert!(!unknown.source_is_remote(exists_locally));
        assert!(unknown.source_is_remote(missing_locally));
    }

    #[test]
    fn test_basic_replacement() {
        let pattern = RenamePattern::new("$0".to_string());
        assert_eq!(pattern.preview("file.txt"), "file.txt");
    }

    #[test]
    fn test_parts_from_left() {
        let pattern = RenamePattern::new("$1_copy.$2".to_string());
        assert_eq!(pattern.preview("document.txt"), "document_copy.txt");
    }

    #[test]
    fn test_parts_from_right() {
        let pattern = RenamePattern::new("$1_backup.$-1".to_string());
        assert_eq!(pattern.preview("archive.tar.gz"), "archive_backup.gz");
    }

    #[test]
    fn test_counter() {
        let pattern = RenamePattern::new("$1_$I.$-1".to_string());
        assert_eq!(pattern.apply("file.txt", 5, None, None), "file_5.txt");
    }

    #[test]
    fn test_complex_pattern() {
        let pattern = RenamePattern::new("$1_$I.$2.$3".to_string());
        assert_eq!(pattern.preview("document.tar.gz"), "document_1.tar.gz");
    }

    #[test]
    fn test_missing_parts() {
        let pattern = RenamePattern::new("$1.$5".to_string());
        assert_eq!(pattern.preview("file.txt"), "file.");
    }

    #[test]
    fn test_validation() {
        let pattern = RenamePattern::new("$1_copy.$-1".to_string());
        assert!(pattern.is_valid_result("file_copy.txt"));
        assert!(!pattern.is_valid_result("file/copy.txt"));
        assert!(!pattern.is_valid_result("file:copy.txt"));
        assert!(!pattern.is_valid_result(""));
    }

    #[test]
    fn test_batch_operation_new() {
        let op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a"), PathBuf::from("/b")],
            PathBuf::from("/dest"),
        );
        assert_eq!(op.total_count(), 2);
        assert!(!op.is_complete());
        assert!(!op.destination_is_directory());
    }

    #[test]
    fn test_batch_operation_lifecycle() {
        let mut op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            PathBuf::from("/dest"),
        );

        assert_eq!(op.total_count(), 3);
        assert!(!op.is_complete());
        assert_eq!(op.current_source(), Some(&PathBuf::from("/a")));

        op.advance();
        op.increment_success();
        assert_eq!(op.current_index, 1);
        assert_eq!(op.success_count, 1);

        op.advance();
        op.increment_error();

        op.advance();
        op.increment_skipped();

        assert!(op.is_complete());
        assert_eq!(op.success_count, 1);
        assert_eq!(op.error_count, 1);
        assert_eq!(op.skipped_count, 1);
    }

    #[test]
    fn test_batch_operation_completed_destinations() {
        let mut op = BatchOperation::new(
            BatchOperationType::Move,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        );

        op.add_completed_destination(PathBuf::from("/dest/a"));
        assert_eq!(op.completed_destinations.len(), 1);
    }

    /// "Delete copied" after a cancel removes only what the batch created:
    /// the path actually written (a rename included), never something that
    /// was there before, and nothing for a move.
    #[test]
    fn cancel_cleanup_records_only_what_a_copy_created() {
        let fresh = Path::new("/definitely/not/here/42/renamed (1).txt");
        let existing = Path::new(env!("CARGO_MANIFEST_DIR"));
        let batch = |kind| {
            BatchOperation::new(
                kind,
                vec![
                    PathBuf::from("/a"),
                    PathBuf::from("/b"),
                    PathBuf::from("/c"),
                ],
                PathBuf::from("/dest"),
            )
        };

        let mut copy = batch(BatchOperationType::Copy);
        copy.note_item_destination(fresh);
        copy.record_item_destination();
        copy.advance();
        // An overwrite or a merge into an existing directory.
        copy.note_item_destination(existing);
        copy.record_item_destination();
        copy.advance();
        // A note from an item that failed does not carry over.
        copy.note_item_destination(fresh);
        copy.advance();
        copy.record_item_destination();
        assert_eq!(copy.completed_destinations, vec![fresh.to_path_buf()]);

        let mut moved = batch(BatchOperationType::Move);
        moved.note_item_destination(fresh);
        moved.record_item_destination();
        assert!(moved.completed_destinations.is_empty());
    }

    #[test]
    fn test_batch_operation_rename_counter() {
        let mut op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        );

        assert_eq!(op.get_and_increment_rename_counter(), 1);
        assert_eq!(op.get_and_increment_rename_counter(), 2);
        assert_eq!(op.get_and_increment_rename_counter(), 3);
    }

    #[test]
    fn test_batch_operation_last_successful_filename() {
        let mut op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a/file.txt"), PathBuf::from("/b/other.rs")],
            PathBuf::from("/dest"),
        );

        // Before any processing
        assert_eq!(op.last_successful_filename(), None);

        // After processing first file
        op.advance();
        op.increment_success();
        assert_eq!(op.last_successful_filename(), Some("file.txt".to_string()));
    }

    #[test]
    fn test_batch_operation_pause_state() {
        let op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        );
        assert_eq!(op.pause_state, PauseState::Running);
    }

    #[test]
    fn test_batch_operation_destination_directory_intent() {
        let op = BatchOperation::new(
            BatchOperationType::Copy,
            vec![PathBuf::from("/a")],
            PathBuf::from("/dest"),
        )
        .with_destination_directory(true);
        assert!(op.destination_is_directory());
    }

    #[test]
    fn test_rename_pattern_no_extension() {
        let pattern = RenamePattern::new("$1_backup".to_string());
        assert_eq!(pattern.preview("README"), "README_backup");
    }

    #[test]
    fn inserted_text_is_not_expanded_again() {
        let pattern = RenamePattern::new("$0".to_string());
        assert_eq!(pattern.preview("report$I.txt"), "report$I.txt");
        assert_eq!(pattern.preview("a$1$-1$C$M$0.txt"), "a$1$-1$C$M$0.txt");
        let pattern = RenamePattern::new("$1-$I.$-1".to_string());
        assert_eq!(
            pattern.apply("pay$Iroll.$0", 7, None, None),
            "pay$Iroll-7.$0"
        );
    }

    #[test]
    fn parts_past_the_last_are_empty() {
        let pattern = RenamePattern::new("[$-3][$-2][$-1]".to_string());
        assert_eq!(pattern.preview("file.txt"), "[][file][txt]");
        let pattern = RenamePattern::new("[$-9]".to_string());
        assert_eq!(pattern.preview("a.b.c"), "[]");
    }

    #[test]
    fn unknown_and_trailing_dollars_are_kept() {
        let pattern = RenamePattern::new("$x_$-_$-0_$".to_string());
        assert_eq!(pattern.preview("f.txt"), "$x_$-_$-0_$");
        // `$12` is part 1 followed by "2", as before.
        let pattern = RenamePattern::new("$12".to_string());
        assert_eq!(pattern.preview("f.txt"), "f2");
    }

    #[test]
    fn test_rename_pattern_multiple_extensions() {
        let pattern = RenamePattern::new("$1.$2.$-1".to_string());
        assert_eq!(pattern.preview("file.tar.gz"), "file.tar.gz");
    }
}
