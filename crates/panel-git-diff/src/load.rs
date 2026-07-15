//! Diff loading and unified-diff parsing for the Git Diff Panel.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use termide_git::{self as git};

use crate::{DiffHunk, DiffLine, FileDiff, FileStatus, GitDiffPanel, LineKind};

static HUNK_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap());

impl GitDiffPanel {
    /// Refresh diff data
    pub fn refresh(&mut self) {
        self.diffs.clear();

        if let Some(hash) = self.commit_hash.clone() {
            // Load commit diff
            self.load_commit_diff(&hash);
        } else {
            // Load working directory changes
            // Get unstaged files
            let unstaged = git::get_unstaged_files(&self.repo_path);
            for file in unstaged {
                if let Some(diff) = self.load_file_diff(&file.path, false, file.status) {
                    self.diffs.push(diff);
                }
            }

            // Get staged files
            let staged = git::get_staged_files(&self.repo_path);
            for file in staged {
                if let Some(diff) = self.load_file_diff(&file.path, true, file.status) {
                    self.diffs.push(diff);
                }
            }
        }

        // Apply file filter if set
        if let Some(ref filter) = self.file_filter {
            self.diffs.retain(|d| &d.path == filter);
        }

        // Reset selection if needed
        if self.selected_file >= self.diffs.len() && !self.diffs.is_empty() {
            self.selected_file = self.diffs.len() - 1;
        }

        self.calculate_total_lines();
    }

    /// Load diff for a specific commit or stash
    fn load_commit_diff(&mut self, hash: &str) {
        let diff_output = if self.is_stash {
            git::stash_diff(&self.repo_path, hash)
        } else {
            git::get_commit_diff(&self.repo_path, hash)
        };
        let Some(diff_output) = diff_output else {
            return;
        };

        // Parse git show output (contains multiple files)
        // Format:
        // commit <hash>
        // Author: <author>
        // Date:   <date>
        //
        //     <message>
        //
        // diff --git a/<file> b/<file>
        // --- a/<file>
        // +++ b/<file>
        // @@ ... @@
        // ...

        let mut current_file: Option<FileDiff> = None;
        let mut current_hunk: Option<DiffHunk> = None;
        let mut old_line = 0usize;
        let mut new_line = 0usize;

        for line in diff_output.lines() {
            // Start of a new file diff
            if line.starts_with("diff --git ") {
                // Save previous hunk if exists
                if let Some(hunk) = current_hunk.take() {
                    if let Some(ref mut file) = current_file {
                        file.hunks.push(hunk);
                    }
                }
                // Save previous file if exists
                if let Some(file) = current_file.take() {
                    self.diffs.push(file);
                }

                // Parse file path from "diff --git a/<path> b/<path>"
                let path = line
                    .strip_prefix("diff --git ")
                    .and_then(|s| s.split_once(' '))
                    .map(|(a, _)| a.strip_prefix("a/").unwrap_or(a))
                    .unwrap_or("")
                    .to_string();

                current_file = Some(FileDiff {
                    path,
                    status: FileStatus::Modified, // Default, will be updated
                    staged: false,                // Not applicable for commits
                    additions: 0,
                    deletions: 0,
                    hunks: Vec::new(),
                });
            } else if line.starts_with("new file mode") {
                if let Some(ref mut file) = current_file {
                    file.status = FileStatus::Added;
                }
            } else if line.starts_with("deleted file mode") {
                if let Some(ref mut file) = current_file {
                    file.status = FileStatus::Deleted;
                }
            } else if line.starts_with("rename from") || line.starts_with("rename to") {
                if let Some(ref mut file) = current_file {
                    file.status = FileStatus::Renamed;
                }
            } else if line.starts_with("@@") {
                // Save previous hunk if exists
                if let Some(hunk) = current_hunk.take() {
                    if let Some(ref mut file) = current_file {
                        file.hunks.push(hunk);
                    }
                }

                let (old_start, new_start) = Self::parse_hunk_header(line);
                old_line = old_start;
                new_line = new_start;

                current_hunk = Some(DiffHunk {
                    header: line.to_string(),
                    lines: Vec::new(),
                });
            } else if let Some(ref mut hunk) = current_hunk {
                if let Some(rest) = line.strip_prefix('+') {
                    hunk.lines.push(DiffLine {
                        kind: LineKind::Added,
                        content: rest.to_string(),
                        old_line: None,
                        new_line: Some(new_line),
                    });
                    if let Some(ref mut file) = current_file {
                        file.additions += 1;
                    }
                    new_line += 1;
                } else if let Some(rest) = line.strip_prefix('-') {
                    hunk.lines.push(DiffLine {
                        kind: LineKind::Removed,
                        content: rest.to_string(),
                        old_line: Some(old_line),
                        new_line: None,
                    });
                    if let Some(ref mut file) = current_file {
                        file.deletions += 1;
                    }
                    old_line += 1;
                } else if let Some(rest) = line.strip_prefix(' ') {
                    hunk.lines.push(DiffLine {
                        kind: LineKind::Context,
                        content: rest.to_string(),
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    });
                    old_line += 1;
                    new_line += 1;
                }
            }
        }

        // Don't forget the last hunk and file
        if let Some(hunk) = current_hunk {
            if let Some(ref mut file) = current_file {
                file.hunks.push(hunk);
            }
        }
        if let Some(file) = current_file {
            self.diffs.push(file);
        }
    }

    /// Load diff for a single file
    fn load_file_diff(&self, path: &Path, staged: bool, status_char: char) -> Option<FileDiff> {
        let status = match status_char {
            'A' => FileStatus::Added,
            'D' => FileStatus::Deleted,
            'R' => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };

        let diff_text = git::get_file_diff(&self.repo_path, path, staged)?;
        let stats = git::get_file_diff_stats(&self.repo_path, path, staged);

        let hunks = Self::parse_diff(&diff_text);

        Some(FileDiff {
            path: path.to_string_lossy().into_owned(),
            status,
            staged,
            additions: stats.additions,
            deletions: stats.deletions,
            hunks,
        })
    }

    /// Parse unified diff output into hunks
    fn parse_diff(diff_text: &str) -> Vec<DiffHunk> {
        let mut hunks = Vec::new();
        let mut current_hunk: Option<DiffHunk> = None;
        let mut old_line = 0usize;
        let mut new_line = 0usize;

        for line in diff_text.lines() {
            if line.starts_with("@@") {
                // Save previous hunk if exists
                if let Some(hunk) = current_hunk.take() {
                    hunks.push(hunk);
                }

                // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
                let (old_start, new_start) = Self::parse_hunk_header(line);
                old_line = old_start;
                new_line = new_start;

                current_hunk = Some(DiffHunk {
                    header: line.to_string(),
                    lines: Vec::new(),
                });
            } else if let Some(ref mut hunk) = current_hunk {
                let (kind, content) = if let Some(rest) = line.strip_prefix('+') {
                    let diff_line = DiffLine {
                        kind: LineKind::Added,
                        content: rest.to_string(),
                        old_line: None,
                        new_line: Some(new_line),
                    };
                    new_line += 1;
                    (LineKind::Added, diff_line)
                } else if let Some(rest) = line.strip_prefix('-') {
                    let diff_line = DiffLine {
                        kind: LineKind::Removed,
                        content: rest.to_string(),
                        old_line: Some(old_line),
                        new_line: None,
                    };
                    old_line += 1;
                    (LineKind::Removed, diff_line)
                } else if let Some(rest) = line.strip_prefix(' ') {
                    let diff_line = DiffLine {
                        kind: LineKind::Context,
                        content: rest.to_string(),
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    };
                    old_line += 1;
                    new_line += 1;
                    (LineKind::Context, diff_line)
                } else {
                    // Line without prefix (shouldn't happen in unified diff)
                    continue;
                };

                let _ = kind; // silence unused warning
                hunk.lines.push(content);
            }
        }

        // Don't forget the last hunk
        if let Some(hunk) = current_hunk {
            hunks.push(hunk);
        }

        hunks
    }

    /// Parse hunk header to get start line numbers
    fn parse_hunk_header(header: &str) -> (usize, usize) {
        if let Some(caps) = HUNK_HEADER_RE.captures(header) {
            let old_start: usize = caps
                .get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
            let new_start: usize = caps
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
            return (old_start, new_start);
        }
        (1, 1)
    }

    /// Calculate total number of renderable lines
    pub(crate) fn calculate_total_lines(&mut self) {
        let mut total = 0;
        for (i, diff) in self.diffs.iter().enumerate() {
            // File header line
            total += 1;
            // If not collapsed, add hunk lines
            if !self.collapsed.contains(&i) {
                for hunk in &diff.hunks {
                    total += 1; // Hunk header
                    total += hunk.lines.len();
                }
            }
        }
        self.total_lines = total;
    }
}
