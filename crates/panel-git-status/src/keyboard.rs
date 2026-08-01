//! Keyboard handling for the Git Status panel: dropdown-filter typing
//! interception, configurable hotkeys, and cursor/section navigation.

use crossterm::event::KeyCode;

use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up};
use termide_core::{KeyChord, PanelEvent};

use crate::types::{Section, Selection};
use crate::{tree, GitStatusPanel};

impl GitStatusPanel {
    /// Handle a key chord. Trait `Panel::handle_key` delegates here.
    pub(crate) fn on_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        // Clear status message on any key
        self.status_message = None;

        // Repo dropdown filter: intercept printable keys while the dropdown is
        // open so typing narrows the list instead of triggering hotkeys.
        if self.repo_dropdown_open {
            match key.code {
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                        && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    self.show_repo_filter = true;
                    self.repo_filter.push(c);
                    self.dropdown_cursor = 0;
                    return vec![];
                }
                KeyCode::Backspace if self.show_repo_filter && !self.repo_filter.is_empty() => {
                    self.repo_filter.pop();
                    if self.repo_filter.is_empty() {
                        self.show_repo_filter = false;
                    }
                    self.dropdown_cursor = 0;
                    return vec![];
                }
                KeyCode::Esc if self.show_repo_filter && !self.repo_filter.is_empty() => {
                    self.repo_filter.clear();
                    self.show_repo_filter = false;
                    self.dropdown_cursor = 0;
                    return vec![];
                }
                _ => {}
            }
        }

        // Branch dropdown filter: intercept printable keys while the dropdown is
        // open so typing narrows the list instead of triggering hotkeys.
        if self.branch_dropdown_open {
            match key.code {
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                        && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    self.show_branch_filter = true;
                    self.branch_filter.push(c);
                    self.dropdown_cursor = 0;
                    return vec![];
                }
                KeyCode::Backspace if self.show_branch_filter && !self.branch_filter.is_empty() => {
                    self.branch_filter.pop();
                    if self.branch_filter.is_empty() {
                        self.show_branch_filter = false;
                    }
                    self.dropdown_cursor = 0;
                    return vec![];
                }
                KeyCode::Esc if self.show_branch_filter && !self.branch_filter.is_empty() => {
                    self.branch_filter.clear();
                    self.show_branch_filter = false;
                    self.dropdown_cursor = 0;
                    return vec![];
                }
                _ => {}
            }
        }

        // Configurable actions via HotkeyTable
        if self.hotkeys.matches("stage", &key) {
            if self.current_section == Section::Files
                && matches!(
                    self.get_selection(),
                    Some(Selection::UnstagedFile(_)) | Some(Selection::UnstagedDir(_))
                )
            {
                self.do_stage();
            }
            return vec![];
        }

        if self.hotkeys.matches("unstage", &key) {
            if self.current_section == Section::Files
                && matches!(
                    self.get_selection(),
                    Some(Selection::StagedFile(_)) | Some(Selection::StagedDir(_))
                )
            {
                self.do_unstage();
            }
            return vec![];
        }

        if self.hotkeys.matches("view", &key) {
            if self.current_section == Section::Files
                && matches!(
                    self.get_selection(),
                    Some(Selection::UnstagedFile(_)) | Some(Selection::StagedFile(_))
                )
            {
                return self.open_file(false);
            }
            return vec![];
        }
        if self.hotkeys.matches("edit", &key) {
            if self.current_section == Section::Files
                && matches!(
                    self.get_selection(),
                    Some(Selection::UnstagedFile(_)) | Some(Selection::StagedFile(_))
                )
            {
                return self.open_file(true);
            }
            return vec![];
        }
        if self.hotkeys.matches("info", &key) {
            if self.current_section == Section::Files
                && matches!(
                    self.get_selection(),
                    Some(Selection::UnstagedFile(_)) | Some(Selection::StagedFile(_))
                )
            {
                return self.show_file_properties();
            }
            return vec![];
        }
        if self.hotkeys.matches("revert", &key) {
            if self.current_section == Section::Files
                && matches!(
                    self.get_selection(),
                    Some(Selection::UnstagedFile(_)) | Some(Selection::StagedFile(_))
                )
            {
                return self.initiate_revert();
            }
            return vec![];
        }
        if self.hotkeys.matches("refresh", &key) {
            self.refresh();
            self.status_message = Some(termide_i18n::t().git_refreshed().to_string());
            if let Some(repo) = self.repo_manager.current() {
                use termide_core::event::{GitOperationType, PanelEvent};
                return vec![PanelEvent::GitOperation {
                    operation: GitOperationType::Fetch,
                    repo_path: repo.to_path_buf(),
                }];
            }
            return vec![];
        }

        // Vim-aware navigation (j/k/g/G when vim_mode is enabled)
        if is_move_up(&key, self.vim_mode) {
            self.handle_up_key();
            return vec![];
        }
        if is_move_down(&key, self.vim_mode) {
            self.handle_down_key();
            return vec![];
        }
        if is_go_home(&key, self.vim_mode) && self.current_section == Section::Files {
            self.cursor = self.first_selectable_line();
            self.ensure_cursor_visible();
            return vec![];
        }
        if is_go_end(&key, self.vim_mode) && self.current_section == Section::Files {
            self.cursor = self.last_selectable_line();
            self.ensure_cursor_visible();
            return vec![];
        }

        match key.code {
            KeyCode::Tab => {
                self.next_section();
            }
            KeyCode::BackTab => {
                self.prev_section();
            }
            KeyCode::PageUp => {
                if self.current_section == Section::Files {
                    let page_size = self.viewport_height.max(1);
                    let mut new_cursor = self.cursor.saturating_sub(page_size);
                    while new_cursor > 0 && !self.is_selectable_line(new_cursor) {
                        new_cursor -= 1;
                    }
                    if self.is_selectable_line(new_cursor) {
                        self.cursor = new_cursor;
                    }
                    self.ensure_cursor_visible();
                }
            }
            KeyCode::PageDown => {
                if self.current_section == Section::Files {
                    let max = self.total_virtual_lines();
                    let page_size = self.viewport_height.max(1);
                    let target = (self.cursor + page_size).min(max.saturating_sub(1));
                    let mut new_cursor = target;
                    while new_cursor > self.cursor && !self.is_selectable_line(new_cursor) {
                        new_cursor -= 1;
                    }
                    if new_cursor > self.cursor && self.is_selectable_line(new_cursor) {
                        self.cursor = new_cursor;
                    }
                    self.ensure_cursor_visible();
                    if self.cursor == self.last_selectable_line() && max > self.viewport_height {
                        self.scroll_offset = max.saturating_sub(self.viewport_height);
                    }
                }
            }
            KeyCode::Home => {
                if self.current_section == Section::Files {
                    let unstaged_end = 1 + self.unstaged_item_count();
                    let staged_header = unstaged_end;
                    if self.cursor < staged_header {
                        if self.unstaged_item_count() > 0 {
                            self.cursor = 1;
                        } else {
                            self.cursor = 0;
                        }
                    } else if self.staged_item_count() > 0 {
                        self.cursor = staged_header + 1;
                    } else {
                        self.cursor = staged_header;
                    }
                    self.ensure_cursor_visible();
                }
            }
            KeyCode::End => {
                if self.current_section == Section::Files {
                    let unstaged_end = 1 + self.unstaged_item_count();
                    let staged_header = unstaged_end;
                    let staged_end = staged_header + 1 + self.staged_item_count();
                    if self.cursor < staged_header {
                        if self.unstaged_item_count() > 0 {
                            self.cursor = unstaged_end - 1;
                        } else {
                            self.cursor = 0;
                        }
                    } else if self.staged_item_count() > 0 {
                        self.cursor = staged_end - 1;
                    } else {
                        self.cursor = staged_header;
                    }
                    self.ensure_cursor_visible();
                }
            }
            KeyCode::Left => match self.current_section {
                Section::BranchSelector => {
                    self.current_section = Section::RepoSelector;
                }
                Section::Buttons => {
                    if self.selected_button > 0 {
                        self.selected_button -= 1;
                    }
                }
                Section::Files => match self.get_selection() {
                    Some(Selection::UnstagedDir(idx)) => {
                        if matches!(
                            self.unstaged.tree[idx].kind,
                            tree::TreeNodeKind::Directory { expanded: true }
                        ) {
                            self.toggle_dir_expand(true, idx);
                        }
                    }
                    Some(Selection::StagedDir(idx)) => {
                        if matches!(
                            self.staged.tree[idx].kind,
                            tree::TreeNodeKind::Directory { expanded: true }
                        ) {
                            self.toggle_dir_expand(false, idx);
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            KeyCode::Right => match self.current_section {
                Section::RepoSelector => {
                    self.current_section = Section::BranchSelector;
                }
                Section::Buttons => {
                    let max = self.get_visible_buttons().len().saturating_sub(1);
                    if self.selected_button < max {
                        self.selected_button += 1;
                    }
                }
                Section::Files => match self.get_selection() {
                    Some(Selection::UnstagedDir(idx)) => {
                        if matches!(
                            self.unstaged.tree[idx].kind,
                            tree::TreeNodeKind::Directory { expanded: false }
                        ) {
                            self.toggle_dir_expand(true, idx);
                        }
                    }
                    Some(Selection::StagedDir(idx)) => {
                        if matches!(
                            self.staged.tree[idx].kind,
                            tree::TreeNodeKind::Directory { expanded: false }
                        ) {
                            self.toggle_dir_expand(false, idx);
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            KeyCode::Enter => {
                return self.handle_enter_key();
            }
            KeyCode::Esc => {
                if self.branch_dropdown_open {
                    self.branch_dropdown_open = false;
                    self.reset_branch_filter();
                } else if self.repo_dropdown_open {
                    self.repo_dropdown_open = false;
                    self.reset_repo_filter();
                }
            }
            _ => {}
        }

        vec![]
    }
}
