//! Mouse handling for the Git Status panel: dropdown wheel/click routing,
//! file-list selection with double-click staging, and selector/button hit-tests.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use termide_core::PanelEvent;

use crate::types::Section;
use crate::GitStatusPanel;

impl GitStatusPanel {
    /// Handle a mouse event. Trait `Panel::handle_mouse` delegates here.
    pub(crate) fn on_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        let col = event.column;
        let row = event.row;

        match event.kind {
            // Scroll handling. An open selector dropdown takes the wheel first;
            // otherwise the files area scrolls.
            MouseEventKind::ScrollUp => {
                if self.repo_dropdown_open || self.branch_dropdown_open {
                    self.dropdown_cursor = self.dropdown_cursor.saturating_sub(1);
                } else if self.is_in_rect(col, row, self.files_area) {
                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.repo_dropdown_open || self.branch_dropdown_open {
                    let max = self.open_dropdown_len().saturating_sub(1);
                    if self.dropdown_cursor < max {
                        self.dropdown_cursor += 1;
                    }
                } else if self.is_in_rect(col, row, self.files_area) {
                    let total_lines = self.total_virtual_lines();
                    let max_scroll = total_lines.saturating_sub(self.viewport_height);
                    self.scroll_offset = (self.scroll_offset + 3).min(max_scroll);
                }
            }

            // Click handling
            MouseEventKind::Down(MouseButton::Left) => {
                let now = std::time::Instant::now();

                // Check if click is in open repo dropdown
                if self.repo_dropdown_open {
                    if let Some(dropdown_area) = self.repo_dropdown_area {
                        if self.is_in_rect(col, row, dropdown_area) {
                            // Calculate which item was clicked (accounting for border + filter row)
                            let row_offset = if self.show_repo_filter { 3 } else { 1 };
                            let relative_row =
                                row.saturating_sub(dropdown_area.y + row_offset) as usize;
                            let clicked_idx = self.dropdown_scroll + relative_row;
                            let max_items = if self.show_repo_filter {
                                self.filtered_repo_indices().len()
                            } else {
                                self.repo_manager.len()
                            };
                            if clicked_idx < max_items
                                && relative_row
                                    < dropdown_area.height.saturating_sub(row_offset + 1) as usize
                            {
                                let repo_idx = if self.show_repo_filter {
                                    self.filtered_repo_indices()
                                        .get(clicked_idx)
                                        .copied()
                                        .unwrap_or(0)
                                } else {
                                    clicked_idx
                                };
                                self.repo_manager.select(repo_idx);
                                self.refresh();
                            }
                            self.repo_dropdown_open = false;
                            self.reset_repo_filter();
                            return vec![];
                        }
                    }
                }

                // Check if click is in open branch dropdown
                if self.branch_dropdown_open {
                    if let Some(dropdown_area) = self.branch_dropdown_area {
                        if self.is_in_rect(col, row, dropdown_area) {
                            // Calculate which item was clicked (accounting for border + filter row)
                            let row_offset = if self.show_branch_filter { 3 } else { 1 };
                            let relative_row =
                                row.saturating_sub(dropdown_area.y + row_offset) as usize;
                            let clicked_idx = self.dropdown_scroll + relative_row;
                            let max_items = if self.show_branch_filter {
                                self.filtered_branch_indices().len()
                            } else {
                                self.branches.len()
                            };
                            if clicked_idx < max_items
                                && relative_row
                                    < dropdown_area.height.saturating_sub(row_offset + 1) as usize
                            {
                                let branch_idx = if self.show_branch_filter {
                                    self.filtered_branch_indices()
                                        .get(clicked_idx)
                                        .copied()
                                        .unwrap_or(0)
                                } else {
                                    clicked_idx
                                };
                                self.switch_to_branch(branch_idx);
                            }
                            self.branch_dropdown_open = false;
                            self.reset_branch_filter();
                            return vec![];
                        }
                    }
                }

                // Check if click is in files area (unified)
                if self.is_in_rect(col, row, self.files_area) {
                    // Close any open dropdown
                    self.repo_dropdown_open = false;
                    self.branch_dropdown_open = false;

                    let relative_row = (row - self.files_area.y) as usize;
                    let relative_col = (col - self.files_area.x) as usize;
                    let vline = self.scroll_offset + relative_row;

                    // Virtual layout constants
                    let unstaged_files_start = 1;
                    let unstaged_files_end = unstaged_files_start + self.unstaged_item_count();
                    let staged_header_line = unstaged_files_end;
                    let staged_files_start = staged_header_line + 1;
                    let staged_files_end = staged_files_start + self.staged_item_count();

                    // Determine what was clicked
                    let unstaged_header_line = 0;

                    // Single click on directory icon → toggle expand/collapse
                    if let Some((is_unstaged, tree_idx)) =
                        self.check_dir_icon_click(vline, relative_col)
                    {
                        self.current_section = Section::Files;
                        self.cursor = vline;
                        self.toggle_dir_expand(is_unstaged, tree_idx);
                        self.reset_click_state();
                    } else if vline == unstaged_header_line && !self.unstaged_files.is_empty() {
                        // Clicked on unstaged header (with Stage all button)
                        self.current_section = Section::Files;
                        self.cursor = vline;
                        self.record_click(now, vline);
                    } else if vline >= unstaged_files_start && vline < unstaged_files_end {
                        // Clicked on unstaged item (file or dir name area)
                        self.current_section = Section::Files;
                        self.cursor = vline;
                        if self.check_double_click(now, vline) {
                            // Double-click: stage file
                            self.do_stage();
                            self.reset_click_state();
                        } else {
                            self.record_click(now, vline);
                        }
                    } else if vline == staged_header_line && !self.staged_files.is_empty() {
                        // Clicked on staged header (with Unstage all button)
                        self.current_section = Section::Files;
                        self.cursor = vline;
                        self.record_click(now, vline);
                    } else if vline >= staged_files_start && vline < staged_files_end {
                        // Clicked on staged item (file or dir name area)
                        self.current_section = Section::Files;
                        self.cursor = vline;
                        if self.check_double_click(now, vline) {
                            // Double-click: unstage file
                            self.do_unstage();
                            self.reset_click_state();
                        } else {
                            self.record_click(now, vline);
                        }
                    }
                    // Clicks on empty header lines are ignored
                }
                // Check if click is on selector row
                else if row == self.selector_y {
                    // Use saved branch_selector_x position for accurate detection
                    if col < self.branch_selector_x {
                        self.current_section = Section::RepoSelector;
                        // Toggle repo dropdown (close branch if open)
                        self.branch_dropdown_open = false;
                        self.repo_dropdown_open = !self.repo_dropdown_open;
                        if self.repo_dropdown_open {
                            self.dropdown_cursor = self.repo_manager.selected_index();
                        }
                    } else {
                        self.current_section = Section::BranchSelector;
                        // Toggle branch dropdown (close repo if open)
                        self.repo_dropdown_open = false;
                        self.branch_dropdown_open = !self.branch_dropdown_open;
                        if self.branch_dropdown_open {
                            self.dropdown_cursor = self
                                .branches
                                .iter()
                                .position(|b| Some(b.as_str()) == self.branch.as_deref())
                                .unwrap_or(0);
                        }
                    }
                    // Reset click state for non-file areas
                    self.reset_click_state();
                }
                // Check if click is on buttons area (may span multiple rows)
                else if row >= self.buttons_y && row < self.buttons_y + self.cached_buttons_height
                {
                    // Close any open dropdown
                    self.repo_dropdown_open = false;
                    self.branch_dropdown_open = false;

                    self.current_section = Section::Buttons;
                    // Calculate which button was clicked, accounting for wrapping
                    // Note: last_area is already content area (borders handled by ui-render)
                    let buttons = self.get_visible_buttons();
                    let content_x = self.last_area.x;
                    let content_width = self.last_area.width;
                    let mut btn_x = content_x;
                    let mut btn_y = self.buttons_y;
                    for (i, button) in buttons.iter().enumerate() {
                        let label = format!("[{}]", button.label(self.spinner_frame));
                        let btn_width = label.width() as u16;
                        if btn_x > content_x && btn_x + btn_width > content_x + content_width {
                            btn_y += 1;
                            btn_x = content_x;
                        }
                        if row == btn_y && col >= btn_x && col < btn_x + btn_width {
                            self.selected_button = i;
                            // Execute button action on click
                            return self.execute_button();
                        }
                        btn_x += btn_width + 1;
                    }
                    // Reset click state for non-file areas
                    self.reset_click_state();
                }
            }

            _ => {}
        }

        vec![]
    }
}
