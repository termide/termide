//! Folding for the Editor: fold and unfold at the cursor or everywhere, with
//! the ranges from the language server when it answers in time and from the
//! indentation otherwise.

use std::sync::mpsc::TryRecvError;
use std::time::Instant;

use crate::folding::{self, FoldAction, FoldRange};

use super::{Editor, LspManager};

impl Editor {
    /// Fold the innermost unfolded block around the cursor; again, the block
    /// around that one.
    pub(crate) fn fold(&mut self) {
        self.run_fold(FoldAction::Fold);
    }

    /// Fold every block.
    pub(crate) fn fold_all(&mut self) {
        self.run_fold(FoldAction::FoldAll);
    }

    /// Unfold the fold the cursor's line heads; `false` when it heads none.
    pub(crate) fn unfold(&mut self) -> bool {
        self.buffer.remove_fold(self.cursor.line)
    }

    /// Unfold everything; `false` when nothing was folded.
    pub(crate) fn unfold_all(&mut self) -> bool {
        self.buffer.clear_folds()
    }

    /// Unfold the fold the cursor's line heads, or fold around the cursor.
    pub(crate) fn toggle_fold(&mut self) {
        if !self.unfold() {
            self.fold();
        }
    }

    /// Whether anything is folded.
    pub(crate) fn has_folds(&self) -> bool {
        !self.buffer.folds().is_empty()
    }

    fn run_fold(&mut self, action: FoldAction) {
        let version = self.buffer.edit_version();
        let known = match &self.folding.server_ranges {
            Some((v, ranges)) if *v == version => Some(ranges.clone()),
            _ => None,
        };
        if let Some(ranges) = known {
            self.apply_fold(action, &ranges);
        } else if self.lsp.enabled && self.lsp.language_id.is_some() && self.file_path().is_some() {
            // The app layer, which holds the servers, asks for the ranges.
            self.folding.pending = Some(action);
            self.folding.request_wanted = true;
        } else {
            self.finish_pending_fold(Some(action), None);
        }
    }

    /// Consume the request a fold left for the app layer.
    pub fn take_folding_request(&mut self) -> bool {
        std::mem::take(&mut self.folding.request_wanted)
    }

    /// Ask the language server for the ranges a pending fold waits on; with
    /// no server that answers them, fold by the indentation at once.
    pub fn request_folding_ranges(&mut self, lsp_manager: Option<&LspManager>) {
        let rx = lsp_manager.and_then(|manager| {
            self.flush_lsp_changes(manager);
            let lang = self.lsp.language_id.clone()?;
            let path = self.file_path()?.to_path_buf();
            manager.folding_range(&lang, &path)
        });
        match rx {
            Some(rx) => {
                self.folding.rx = Some((rx, self.buffer.edit_version(), Instant::now()));
            }
            None => {
                let action = self.folding.pending.take();
                self.finish_pending_fold(action, None);
            }
        }
    }

    /// Take the server's ranges once they came, or give up on them after a
    /// while and fold by the indentation. Returns whether the screen changed.
    pub fn poll_folding_ranges(&mut self) -> bool {
        let Some((rx, version, asked)) = &self.folding.rx else {
            return false;
        };
        let answer = match rx.try_recv() {
            Ok(Some(ranges)) => Some(folding::server_ranges(&ranges)),
            Ok(None) | Err(TryRecvError::Disconnected) => None,
            Err(TryRecvError::Empty) if asked.elapsed() < folding::SERVER_WAIT => return false,
            // The fold does not wait any longer, but a server still busy
            // indexing may answer later: keep its ranges for the next one.
            Err(TryRecvError::Empty) if asked.elapsed() < folding::LATE_ANSWER_WAIT => {
                let action = self.folding.pending.take();
                return self.finish_pending_fold(action, None);
            }
            Err(TryRecvError::Empty) => None,
        };
        let version = *version;
        self.folding.rx = None;
        // A server with nothing to say leaves it to the indentation.
        let answer = answer.filter(|ranges| !ranges.is_empty());
        if let Some(ranges) = &answer {
            self.folding.server_ranges = Some((version, ranges.clone()));
        }
        let current = answer.filter(|_| version == self.buffer.edit_version());
        let action = self.folding.pending.take();
        self.finish_pending_fold(action, current)
    }

    /// Carry out `action` with `ranges`, or with the indentation's.
    fn finish_pending_fold(
        &mut self,
        action: Option<FoldAction>,
        ranges: Option<Vec<FoldRange>>,
    ) -> bool {
        let Some(action) = action else {
            return false;
        };
        let ranges =
            ranges.unwrap_or_else(|| folding::indent_ranges(&self.buffer, self.config.tab_size));
        self.apply_fold(action, &ranges)
    }

    fn apply_fold(&mut self, action: FoldAction, ranges: &[FoldRange]) -> bool {
        let changed = match action {
            FoldAction::Fold => {
                let buffer = &self.buffer;
                match folding::range_to_fold(ranges, self.cursor.line, |start| {
                    buffer.fold_at(start).is_some()
                }) {
                    Some((start, end)) => self.buffer.add_fold(start, end),
                    None => false,
                }
            }
            FoldAction::FoldAll => ranges.iter().fold(false, |changed, &(start, end)| {
                self.buffer.add_fold(start, end) | changed
            }),
        };
        // The cursor stays on screen: on the header of the fold that hid it.
        if self.buffer.is_line_hidden(self.cursor.line) {
            self.cursor.line = self.buffer.visible_at_or_before(self.cursor.line);
            self.clamp_cursor();
            self.input.preferred_column = None;
        }
        changed
    }

    /// Show what folds hide that must show: the cursor's line, which a
    /// search, a jump or a click may have put inside a fold, and the top of
    /// the viewport, which an edit may have left there.
    pub(crate) fn settle_folds(&mut self) {
        if self.buffer.is_line_hidden(self.cursor.line) {
            self.buffer.reveal_line(self.cursor.line);
        }
        let top = self.viewport.top_line;
        if self.buffer.is_line_hidden(top) {
            self.viewport.top_line = self.buffer.visible_at_or_before(top);
            self.viewport.top_visual_row_offset = 0;
        }
        self.render_cache.sync_folds(self.buffer.folds_version());
    }

    /// Run `f` on the viewport with its top line counted in lines that show,
    /// the unit scrolling without word wrap works in.
    pub(crate) fn in_visible_lines<R>(
        &mut self,
        f: impl FnOnce(&mut termide_buffer::Viewport) -> R,
    ) -> R {
        let top = self.viewport.top_line;
        self.viewport.top_line = top - self.buffer.hidden_before(top);
        let result = f(&mut self.viewport);
        self.viewport.top_line = self
            .buffer
            .line_at_visible_index(self.viewport.top_line)
            .min(self.buffer.line_count().saturating_sub(1));
        result
    }

    /// `line` counted in lines that show.
    pub(crate) fn visible_index(&self, line: usize) -> usize {
        line - self.buffer.hidden_before(line)
    }
}

#[cfg(test)]
#[path = "editor_folding_tests.rs"]
mod tests;
