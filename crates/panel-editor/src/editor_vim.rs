//! Vim key-result execution for the Editor (applies `VimKeyResult` to the buffer/cursor).

use termide_core::PanelEvent;

use super::Editor;

impl Editor {
    /// Execute a Vim key result and return any panel events
    pub(super) fn execute_vim_result(
        &mut self,
        result: crate::vim::VimKeyResult,
    ) -> Option<Vec<PanelEvent>> {
        use crate::vim::{
            motions::execute_motion, operators, InsertPosition, PanelDirection, VimKeyResult,
            VimMode,
        };
        use termide_core::VimPanelDirection;

        let version_before = self.buffer.edit_version();
        let mut events = Vec::new();

        match result {
            VimKeyResult::Motion { motion, count } => {
                let viewport_height = self.viewport.height;
                let content_width = self.render_cache.content_width;
                let new_cursor = execute_motion(
                    motion,
                    &self.cursor,
                    &self.buffer,
                    count,
                    viewport_height,
                    content_width,
                    true, // use_smart_wrap
                );
                self.cursor = new_cursor;
                // Clear selection on normal mode motion
                self.selection = None;
            }
            VimKeyResult::MotionWithSelection { motion, count } => {
                let viewport_height = self.viewport.height;
                let content_width = self.render_cache.content_width;
                let new_cursor = execute_motion(
                    motion,
                    &self.cursor,
                    &self.buffer,
                    count,
                    viewport_height,
                    content_width,
                    true, // use_smart_wrap
                );

                // Update selection
                if let Some(vim) = &self.vim {
                    if let Some(anchor) = vim.visual_anchor {
                        let selection = termide_buffer::Selection::new(anchor, new_cursor);
                        self.selection = Some(selection);
                    }
                }
                self.cursor = new_cursor;
            }
            VimKeyResult::OperatorMotion {
                operator,
                motion,
                count,
            } => {
                let viewport_height = self.viewport.height;
                let content_width = self.render_cache.content_width;
                let start = self.cursor;
                let end = execute_motion(
                    motion,
                    &self.cursor,
                    &self.buffer,
                    count,
                    viewport_height,
                    content_width,
                    true, // use_smart_wrap
                );

                if let Some(vim) = self.vim.as_mut() {
                    if let Ok(op_result) = operators::execute_operator(
                        operator,
                        start,
                        end,
                        &mut self.buffer,
                        vim,
                        false,
                    ) {
                        self.cursor = op_result.cursor;
                        if op_result.enter_insert {
                            vim.enter_insert();
                        }
                    }
                }
                self.selection = None;
            }
            VimKeyResult::LinewiseOperator { operator, count } => {
                let start_line = self.cursor.line;
                let end_line =
                    (start_line + count - 1).min(self.buffer.line_count().saturating_sub(1));

                if let Some(vim) = self.vim.as_mut() {
                    if let Ok(op_result) = operators::execute_linewise_operator(
                        operator,
                        start_line,
                        end_line,
                        &mut self.buffer,
                        vim,
                    ) {
                        self.cursor = op_result.cursor;
                        if op_result.enter_insert {
                            vim.enter_insert();
                        }
                    }
                }
                self.selection = None;
            }
            VimKeyResult::VisualOperator { operator } => {
                if let (Some(selection), Some(vim)) = (self.selection.as_ref(), self.vim.as_mut()) {
                    let start = selection.start();
                    let end = selection.end();
                    let linewise = vim.mode == VimMode::VisualLine;

                    if let Ok(op_result) = operators::execute_operator(
                        operator,
                        start,
                        end,
                        &mut self.buffer,
                        vim,
                        linewise,
                    ) {
                        self.cursor = op_result.cursor;
                        if op_result.enter_insert {
                            vim.enter_insert();
                        } else {
                            vim.exit_to_normal();
                        }
                    }
                }
                self.selection = None;
            }
            VimKeyResult::EnterInsert(position) => {
                // Position cursor based on insert position
                match position {
                    InsertPosition::BeforeCursor => {
                        // Cursor stays where it is
                    }
                    InsertPosition::AfterCursor => {
                        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
                        if self.cursor.column < line_len {
                            self.cursor.column += 1;
                        }
                    }
                    InsertPosition::LineStart => {
                        // Move to first non-blank
                        if let Some(line) = self.buffer.line(self.cursor.line) {
                            use unicode_segmentation::UnicodeSegmentation;
                            let line = line.trim_end_matches('\n');
                            let first_non_blank = line
                                .graphemes(true)
                                .position(|g| !g.chars().all(|c| c.is_whitespace()))
                                .unwrap_or(0);
                            self.cursor.column = first_non_blank;
                        }
                    }
                    InsertPosition::LineEnd => {
                        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
                        self.cursor.column = line_len;
                    }
                    InsertPosition::NewLineBelow => {
                        // Insert new line below and position cursor
                        let line_len = self.buffer.line_len_graphemes(self.cursor.line);
                        self.cursor.column = line_len;
                        let _ = self.buffer.insert(&self.cursor, "\n");
                        self.cursor.line += 1;
                        self.cursor.column = 0;
                    }
                    InsertPosition::NewLineAbove => {
                        // Insert new line above and position cursor
                        self.cursor.column = 0;
                        let _ = self.buffer.insert(&self.cursor, "\n");
                        // Cursor stays on the new (now previous) line
                    }
                }
                if let Some(vim) = self.vim.as_mut() {
                    vim.enter_insert();
                }
            }
            VimKeyResult::ExitToNormal => {
                // Move cursor back one position when exiting insert mode
                if self.cursor.column > 0 {
                    self.cursor.column -= 1;
                }
                self.selection = None;
            }
            VimKeyResult::StartVisual => {
                if let Some(vim) = self.vim.as_mut() {
                    vim.enter_visual(self.cursor);
                    // Start selection at current cursor
                    self.selection = Some(termide_buffer::Selection::new(self.cursor, self.cursor));
                }
            }
            VimKeyResult::StartVisualLine => {
                if let Some(vim) = self.vim.as_mut() {
                    vim.enter_visual_line(self.cursor);
                    // Select the entire line
                    let line_start = termide_buffer::Cursor::at(self.cursor.line, 0);
                    let line_end_col = self.buffer.line_len_graphemes(self.cursor.line);
                    let line_end = termide_buffer::Cursor::at(self.cursor.line, line_end_col);
                    self.selection = Some(termide_buffer::Selection::new(line_start, line_end));
                }
            }
            VimKeyResult::DeleteChar { count } => {
                for _ in 0..count {
                    if let Some(vim) = self.vim.as_mut() {
                        if let Ok(Some(deleted)) =
                            operators::delete_char(&mut self.buffer, &self.cursor)
                        {
                            vim.yank(deleted, false);
                        }
                    }
                }
            }
            VimKeyResult::Paste { after, count } => {
                if let Some(vim) = &self.vim {
                    if let Some(text) = vim.get_register() {
                        let linewise = vim.is_register_linewise();
                        for _ in 0..count {
                            if linewise {
                                // Linewise paste - insert on new line
                                let paste_line = if after {
                                    self.cursor.line + 1
                                } else {
                                    self.cursor.line
                                };
                                let insert_cursor = termide_buffer::Cursor::at(paste_line, 0);
                                // Need to handle insertion at end of document
                                if paste_line >= self.buffer.line_count() {
                                    let last_line_len = self
                                        .buffer
                                        .line_len_graphemes(self.buffer.line_count() - 1);
                                    let end_cursor = termide_buffer::Cursor::at(
                                        self.buffer.line_count() - 1,
                                        last_line_len,
                                    );
                                    let mut text_with_newline = String::from("\n");
                                    text_with_newline.push_str(text.trim_end_matches('\n'));
                                    let _ = self.buffer.insert(&end_cursor, &text_with_newline);
                                } else {
                                    let _ = self.buffer.insert(&insert_cursor, text);
                                }
                                self.cursor.line = paste_line;
                                // Move to first non-blank
                                if let Some(line) = self.buffer.line(self.cursor.line) {
                                    use unicode_segmentation::UnicodeSegmentation;
                                    let line = line.trim_end_matches('\n');
                                    let first_non_blank = line
                                        .graphemes(true)
                                        .position(|g| !g.chars().all(|c| c.is_whitespace()))
                                        .unwrap_or(0);
                                    self.cursor.column = first_non_blank;
                                }
                            } else {
                                // Charwise paste
                                let insert_cursor = if after {
                                    termide_buffer::Cursor::at(
                                        self.cursor.line,
                                        self.cursor.column + 1,
                                    )
                                } else {
                                    self.cursor
                                };
                                if let Ok(new_cursor) = self.buffer.insert(&insert_cursor, text) {
                                    self.cursor = new_cursor;
                                    // Position cursor on last char of pasted text
                                    if self.cursor.column > 0 {
                                        self.cursor.column -= 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            VimKeyResult::Undo => {
                if let Ok(Some(new_cursor)) = self.buffer.undo() {
                    self.cursor = new_cursor;
                }
            }
            VimKeyResult::Redo => {
                if let Ok(Some(new_cursor)) = self.buffer.redo() {
                    self.cursor = new_cursor;
                }
            }
            VimKeyResult::PanelNavigation(direction) => {
                let vim_direction = match direction {
                    PanelDirection::Left => VimPanelDirection::Left,
                    PanelDirection::Down => VimPanelDirection::Down,
                    PanelDirection::Up => VimPanelDirection::Up,
                    PanelDirection::Right => VimPanelDirection::Right,
                };
                events.push(PanelEvent::VimPanelNavigation {
                    direction: vim_direction,
                });
            }
            VimKeyResult::Consumed | VimKeyResult::PassThrough | VimKeyResult::Unhandled => {
                // These are handled in handle_key
            }
        }

        // Ensure cursor is valid after operations
        self.clamp_cursor();

        // Catch-all: invalidate wrap cache if buffer was modified by any VIM operation
        if self.buffer.edit_version() != version_before {
            self.render_cache.invalidate_wrap_cache();
            self.render_cache
                .highlight
                .invalidate_range(0, self.buffer.line_count());
            self.schedule_git_diff_update();
            self.mark_lsp_changed();
        }

        if events.is_empty() {
            None
        } else {
            Some(events)
        }
    }
}
