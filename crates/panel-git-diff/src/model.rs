//! Diff data model: line/hunk/file types for the Git Diff Panel.

/// Type of diff line
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Context line (unchanged)
    Context,
    /// Added line
    Added,
    /// Removed line
    Removed,
    /// Hunk header (@@ ... @@)
    HunkHeader,
}

/// A single line in the diff
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// Type of line
    pub kind: LineKind,
    /// Line content (without +/- prefix)
    pub content: String,
    /// Old line number (if applicable)
    pub old_line: Option<usize>,
    /// New line number (if applicable)
    pub new_line: Option<usize>,
}

/// A hunk (block of changes)
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// Header line (@@ -x,y +a,b @@)
    pub header: String,
    /// Lines in this hunk
    pub lines: Vec<DiffLine>,
}

/// File status in diff
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// Diff information for a single file
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// File path
    pub path: String,
    /// File status
    pub status: FileStatus,
    /// Is this a staged change
    pub staged: bool,
    /// Number of additions
    pub additions: usize,
    /// Number of deletions
    pub deletions: usize,
    /// Hunks
    pub hunks: Vec<DiffHunk>,
}
