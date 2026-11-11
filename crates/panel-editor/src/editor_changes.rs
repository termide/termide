//! The git changes the gutter marks: stepping through them, and staging,
//! unstaging and reverting the one under the cursor or the selected lines.
//!
//! Staging takes the text as the editor holds it, saved or not, as gitsigns
//! and VS Code do. A revert puts the lines back as the index has them, as an
//! edit of the buffer: the editor's undo takes it back, so it does not ask.

use std::ops::RangeInclusive;

use termide_core::PanelEvent;
use termide_i18n::t;

use super::Editor;

impl Editor {
    /// The lines the change actions take: the selection's, a last line
    /// the selection only reaches the start of left out; else the cursor's.
    fn change_lines(&self) -> RangeInclusive<usize> {
        match &self.selection {
            Some(selection) if !selection.is_empty() => {
                let (start, end) = (selection.start(), selection.end());
                let last = if end.column == 0 && end.line > start.line {
                    end.line - 1
                } else {
                    end.line
                };
                start.line..=last
            }
            _ => self.cursor.line..=self.cursor.line,
        }
    }

    /// Whether the change actions can work here: a local file git has.
    fn changes_reachable(&self) -> bool {
        !self.file_state.is_remote() && self.file_path().is_some() && self.git.diff_cache.is_some()
    }

    /// Which of the change actions the lines under the cursor offer:
    /// `(stage or revert, unstage)`.
    pub(crate) fn change_actions(&self) -> (bool, bool) {
        let Some(cache) = self
            .git
            .diff_cache
            .as_ref()
            .filter(|_| self.changes_reachable())
        else {
            return (false, false);
        };
        let lines = self.change_lines();
        (cache.has_unstaged(lines.clone()), cache.has_staged(lines))
    }

    /// Move to the start of the next change against HEAD, or the previous
    /// one, round the end of the file.
    pub(crate) fn goto_change(&mut self, forward: bool) -> Vec<PanelEvent> {
        let starts = self
            .git
            .diff_cache
            .as_ref()
            .map(|cache| cache.change_starts())
            .unwrap_or_default();
        let line = self.cursor.line;
        let target = if forward {
            starts
                .iter()
                .find(|&&start| start > line)
                .or(starts.first())
        } else {
            starts
                .iter()
                .rev()
                .find(|&&start| start < line)
                .or(starts.last())
        };
        match target {
            Some(&target) => {
                self.reveal_line(target);
                vec![PanelEvent::NeedsRedraw]
            }
            None => vec![PanelEvent::ShowMessage(t().editor_no_changes().to_string())],
        }
    }

    /// Stage the changes on the lines under the cursor.
    pub(crate) fn stage_change(&mut self) -> Vec<PanelEvent> {
        self.change_in_index(true)
    }

    /// Unstage the staged changes on the lines under the cursor.
    pub(crate) fn unstage_change(&mut self) -> Vec<PanelEvent> {
        self.change_in_index(false)
    }

    fn change_in_index(&mut self, stage: bool) -> Vec<PanelEvent> {
        if !self.changes_reachable() {
            return vec![];
        }
        let Some(path) = self.file_path().map(|path| path.to_path_buf()) else {
            return vec![];
        };
        let text = self.buffer.to_string();
        let lines = self.change_lines();
        let result = if stage {
            termide_git::stage_lines(&path, &text, lines)
        } else {
            termide_git::unstage_lines(&path, &text, lines)
        };
        match result {
            Ok(true) => {
                self.update_git_diff();
                vec![PanelEvent::NeedsRedraw]
            }
            Ok(false) => vec![PanelEvent::ShowMessage(
                t().editor_no_change_here().to_string(),
            )],
            Err(error) => {
                let t = t();
                let message = if stage {
                    t.editor_stage_failed_fmt(&error)
                } else {
                    t.editor_unstage_failed_fmt(&error)
                };
                vec![PanelEvent::ShowMessage(message)]
            }
        }
    }

    /// Put the lines under the cursor back as the index has them.
    pub(crate) fn revert_change(&mut self) -> Vec<PanelEvent> {
        if !self.changes_reachable() || self.config.read_only {
            return vec![];
        }
        let Some(path) = self.file_path().map(|path| path.to_path_buf()) else {
            return vec![];
        };
        let index = termide_git::index_file_content(&path).unwrap_or_default();
        let text = self.buffer.to_string();
        let Some(revert) = termide_git::revert_lines(&index, &text, self.change_lines()) else {
            return vec![PanelEvent::ShowMessage(
                t().editor_no_change_here().to_string(),
            )];
        };
        let start = lsp_types::Position::new(revert.lines.start as u32, 0);
        let end = if revert.lines.end < self.buffer.line_count() {
            lsp_types::Position::new(revert.lines.end as u32, 0)
        } else {
            // The text's last line has no newline: the range ends with it.
            let last = self.buffer.line_count().saturating_sub(1);
            let width = self
                .buffer
                .line(last)
                .unwrap_or_default()
                .encode_utf16()
                .count();
            lsp_types::Position::new(last as u32, width as u32)
        };
        let edit = lsp_types::TextEdit {
            range: lsp_types::Range::new(start, end),
            new_text: revert.text,
        };
        self.selection = None;
        self.buffer.begin_undo_group();
        self.apply_workspace_edits(std::slice::from_ref(&edit));
        self.buffer.end_undo_group();
        self.cursor = termide_buffer::Cursor::at(revert.lines.start, 0);
        self.clamp_cursor();
        self.schedule_git_diff_update();
        vec![PanelEvent::NeedsRedraw]
    }
}

#[cfg(test)]
#[path = "editor_changes_tests.rs"]
mod tests;
