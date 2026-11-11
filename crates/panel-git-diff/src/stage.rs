//! The line cursor, the selection of lines, and what the panel does with
//! them on the working tree's diff: stage, unstage, discard, and bring the
//! last discard back.
//!
//! The cursor walks the rows as they are drawn: a file's header, then, when
//! the file is expanded, each hunk's header and its lines. A header row
//! stands for its whole file or hunk; a line row for that line, or for the
//! selected lines around it. A selection never leaves its hunk, since a
//! patch is cut from one hunk at a time.

use std::path::{Path, PathBuf};

use termide_core::{ConfirmAction, PanelEvent};
use termide_git::{self as git, PatchAction, PatchError, PatchPart};

use crate::{FileStatus, GitDiffPanel};

/// The answer `PanelCommand::Confirmed` brings back for a discard.
pub(crate) const DISCARD_ACTION: &str = "git-diff:discard";

/// One drawn row of the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// The header of file `.0`.
    File(usize),
    /// The header of hunk `.1` of file `.0`.
    Hunk(usize, usize),
    /// Line `.2` of hunk `.1` of file `.0`.
    Line(usize, usize, usize),
}

impl Row {
    fn file(self) -> usize {
        match self {
            Row::File(f) | Row::Hunk(f, _) | Row::Line(f, _, _) => f,
        }
    }
}

/// What an action takes: a whole file, or part of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) path: PathBuf,
    /// The staged side of the file rather than its working-tree side.
    staged: bool,
    untracked: bool,
    /// `None` for the whole file.
    pub(crate) part: Option<PatchPart>,
}

/// What the last discard took, to bring it back.
#[derive(Debug, Clone)]
pub(crate) struct Discarded {
    path: PathBuf,
    /// The file's blob as it was, or `None` when there was no file.
    blob: Option<String>,
}

impl GitDiffPanel {
    /// The row drawn at `index`.
    pub(crate) fn row_at(&self, index: usize) -> Option<Row> {
        let mut at = 0;
        for (f, diff) in self.diffs.iter().enumerate() {
            if at == index {
                return Some(Row::File(f));
            }
            at += 1;
            if self.collapsed.contains(&f) {
                continue;
            }
            for (h, hunk) in diff.hunks.iter().enumerate() {
                if at == index {
                    return Some(Row::Hunk(f, h));
                }
                at += 1;
                if index < at + hunk.lines.len() {
                    return Some(Row::Line(f, h, index - at));
                }
                at += hunk.lines.len();
            }
        }
        None
    }

    /// Where file `file`'s header is drawn.
    pub(crate) fn file_row(&self, file: usize) -> usize {
        (0..file)
            .map(|i| {
                1 + if self.collapsed.contains(&i) {
                    0
                } else {
                    self.diffs[i].hunks.iter().map(|h| 1 + h.lines.len()).sum()
                }
            })
            .sum()
    }

    /// The rows of the hunk headers, in order.
    fn hunk_rows(&self) -> Vec<usize> {
        let mut rows = Vec::new();
        let mut at = 0;
        for (f, diff) in self.diffs.iter().enumerate() {
            at += 1;
            if self.collapsed.contains(&f) {
                continue;
            }
            for hunk in &diff.hunks {
                rows.push(at);
                at += 1 + hunk.lines.len();
            }
        }
        rows
    }

    /// The first and last row of the selection, when there is one.
    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    /// The rows the cursor may reach: all of them, or with a selection the
    /// lines of its hunk.
    fn reach(&self) -> (usize, usize) {
        let last = self.total_lines.saturating_sub(1);
        let Some(anchor) = self.anchor else {
            return (0, last);
        };
        match self.row_at(anchor) {
            Some(Row::Line(f, h, i)) => {
                let first = anchor - i;
                (first, first + self.diffs[f].hunks[h].lines.len() - 1)
            }
            _ => (0, last),
        }
    }

    /// Put the cursor on `row`, within its reach, and scroll it into view.
    pub(crate) fn set_cursor(&mut self, row: usize) {
        let (first, last) = self.reach();
        self.cursor = row.clamp(first, last);
        if let Some(row) = self.row_at(self.cursor) {
            self.selected_file = row.file();
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.visible_height > 0 && self.cursor >= self.scroll + self.visible_height {
            self.scroll = self.cursor + 1 - self.visible_height;
        }
    }

    /// The rows changed under the cursor (a reload): it stays on the same
    /// row number within them, so a staged hunk's place goes to the next
    /// one, and the selection stays only while both its ends are lines of
    /// one hunk.
    pub(crate) fn settle_cursor(&mut self) {
        self.cursor = self.cursor.min(self.total_lines.saturating_sub(1));
        if let Some(anchor) = self.anchor {
            let one_hunk = match (self.row_at(anchor), self.row_at(self.cursor)) {
                (Some(Row::Line(f, h, _)), Some(Row::Line(g, k, _))) => (f, h) == (g, k),
                _ => false,
            };
            if !one_hunk {
                self.anchor = None;
            }
        }
        self.selected_file = self.row_at(self.cursor).map_or(0, Row::file);
    }

    /// Move the cursor `delta` rows.
    pub(crate) fn move_cursor(&mut self, delta: isize) {
        self.set_cursor(self.cursor.saturating_add_signed(delta));
    }

    /// Move the cursor `delta` rows, starting a selection on a line row
    /// that has none (`Shift+↑↓`).
    pub(crate) fn extend_selection(&mut self, delta: isize) {
        if self.anchor.is_none() && self.selectable() {
            self.anchor = Some(self.cursor);
        }
        self.move_cursor(delta);
    }

    /// Start a selection at the cursor, or drop the one there is.
    pub(crate) fn toggle_selection(&mut self) {
        if self.anchor.take().is_none() && self.selectable() {
            self.anchor = Some(self.cursor);
        }
    }

    /// Whether lines can be selected at the cursor: a line of a file whose
    /// changes can be taken in part.
    fn selectable(&self) -> bool {
        (self.can_stage() || self.review.is_some())
            && matches!(self.row_at(self.cursor), Some(Row::Line(f, _, _)) if self.partial(f))
    }

    /// Jump to the next hunk's header, or with `back` the previous one.
    pub(crate) fn jump_hunk(&mut self, back: bool) {
        self.anchor = None;
        let rows = self.hunk_rows();
        let target = if back {
            rows.iter().rev().find(|&&r| r < self.cursor)
        } else {
            rows.iter().find(|&&r| r > self.cursor)
        };
        if let Some(&row) = target {
            self.set_cursor(row);
        }
    }

    /// Whether the panel shows the working tree, whose changes it can
    /// stage, unstage and discard.
    pub(crate) fn can_stage(&self) -> bool {
        self.given.is_none() && self.commit_hash.is_none() && self.review.is_none()
    }

    /// Whether file `file`'s changes can be taken a hunk or a line at a
    /// time: untracked, renamed and binary files go whole, and in a review
    /// files the agent created or deleted.
    fn partial(&self, file: usize) -> bool {
        let diff = &self.diffs[file];
        let whole = match diff.status {
            FileStatus::Untracked | FileStatus::Renamed => true,
            FileStatus::Added | FileStatus::Deleted => self.review.is_some(),
            FileStatus::Modified => false,
        };
        !whole && !diff.hunks.is_empty()
    }

    /// What the cursor and the selection stand for.
    pub(crate) fn target(&self) -> Option<Target> {
        let row = self.row_at(self.cursor)?;
        let file = row.file();
        let diff = &self.diffs[file];
        let part = match row {
            _ if !self.partial(file) => None,
            Row::File(_) => None,
            Row::Hunk(_, h) => Some((h, None)),
            Row::Line(_, h, i) => {
                let lines = match self.selection() {
                    Some((first, last)) => (i + first - self.cursor, i + last - self.cursor),
                    None => (i, i),
                };
                Some((h, Some(lines)))
            }
        };
        Some(Target {
            path: PathBuf::from(&diff.path),
            staged: diff.staged,
            untracked: diff.status == FileStatus::Untracked,
            part: part.map(|(hunk, lines)| PatchPart {
                hunk,
                header: diff.hunks[hunk].header.clone(),
                lines,
            }),
        })
    }

    /// Stage what the cursor stands for, or unstage it on the staged side.
    pub(crate) fn toggle_stage(&mut self) -> Vec<PanelEvent> {
        let Some(target) = self.target() else {
            return vec![];
        };
        let action = if target.staged {
            PatchAction::Unstage
        } else {
            PatchAction::Stage
        };
        self.act(&target, action)
    }

    /// Ask before discarding what the cursor stands for; on the staged side
    /// there is nothing to discard, so it unstages.
    pub(crate) fn ask_discard(&mut self) -> Vec<PanelEvent> {
        let Some(target) = self.target() else {
            return vec![];
        };
        if target.staged {
            return self.act(&target, PatchAction::Unstage);
        }
        let t = termide_i18n::t();
        let path = target.path.to_string_lossy();
        let message = if target.untracked {
            t.git_diff_delete_untracked_fmt(&path)
        } else {
            t.git_diff_discard_confirm_fmt(&path)
        };
        self.pending_discard = Some(target);
        vec![PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(DISCARD_ACTION.to_string()),
        }]
    }

    /// The discard the user confirmed.
    pub(crate) fn confirmed_discard(&mut self) {
        if let Some(target) = self.pending_discard.take() {
            self.act(&target, PatchAction::Discard);
        }
    }

    /// Bring back what the last discard took.
    pub(crate) fn undo_discard(&mut self) -> Vec<PanelEvent> {
        let Some(discarded) = self.last_discard.take() else {
            self.status_message = Some(termide_i18n::t().git_diff_nothing_to_undo().to_string());
            return vec![];
        };
        let result = match &discarded.blob {
            Some(blob) => git::restore_blob(&self.repo_path, &discarded.path, blob),
            None => std::fs::remove_file(self.repo_path.join(&discarded.path))
                .map_err(|e| e.to_string()),
        };
        self.status_message = Some(match result {
            Ok(()) => termide_i18n::t().git_diff_restored_fmt(&discarded.path.to_string_lossy()),
            Err(error) => first_line(&error),
        });
        self.refresh();
        vec![PanelEvent::NeedsRedraw]
    }

    /// Run `action` on `target`, keeping the file first when it discards.
    fn act(&mut self, target: &Target, action: PatchAction) -> Vec<PanelEvent> {
        let repo = self.repo_path.clone();
        if action == PatchAction::Discard {
            match keep(&repo, &target.path) {
                Ok(discarded) => self.last_discard = Some(discarded),
                Err(error) => {
                    self.status_message = Some(first_line(&error));
                    return vec![];
                }
            }
        }
        let result = match &target.part {
            Some(part) => git::apply_part(&repo, &target.path, action, part),
            None => whole_file(&repo, target, action).map_err(PatchError::Git),
        };
        self.status_message = result.as_ref().err().map(error_text);
        if result.is_err() && action == PatchAction::Discard {
            self.last_discard = None;
        }
        self.anchor = None;
        self.refresh();
        vec![PanelEvent::NeedsRedraw]
    }
}

/// Keep `path` as it is now, before a discard changes it.
fn keep(repo: &Path, path: &Path) -> Result<Discarded, String> {
    let blob = if repo.join(path).exists() {
        Some(git::save_blob(repo, path)?)
    } else {
        None
    };
    Ok(Discarded {
        path: path.to_path_buf(),
        blob,
    })
}

/// `action` on the whole of `target`'s file.
fn whole_file(repo: &Path, target: &Target, action: PatchAction) -> Result<(), String> {
    match action {
        PatchAction::Stage => git::stage_files(repo, std::slice::from_ref(&target.path)),
        PatchAction::Unstage => git::unstage_file(repo, &target.path),
        PatchAction::Discard if target.untracked => {
            std::fs::remove_file(repo.join(&target.path)).map_err(|e| e.to_string())
        }
        PatchAction::Discard => git::revert_file(repo, &target.path),
    }
}

/// What the status line says of an action on part of a file that failed.
pub(crate) fn error_text(error: &PatchError) -> String {
    let t = termide_i18n::t();
    match error {
        PatchError::Stale => t.git_diff_stale().to_string(),
        PatchError::NothingChosen => t.git_diff_nothing_chosen().to_string(),
        PatchError::Git(error) => first_line(error),
    }
}

/// Git's words fit the status line by their first line.
pub(crate) fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests;
