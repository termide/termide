//! Search and replace methods for the Editor.
//!
//! This module contains search and replace functionality including:
//! - Search initiation and navigation
//! - Match highlighting and selection
//! - Replace current/all operations
//! - Navigation helpers that interact with search state

use anyhow::Result;
use termide_buffer::SearchState;

use termide_core::{PanelEvent, Searchable};
use termide_modal::{FindBar, FindBarAction, FindBarBtn, FindBarConfig, FindField};

use crate::search;

use super::Editor;

impl Searchable for Editor {
    fn start_search(&mut self, query: String, case_sensitive: bool, use_regex: bool) {
        self.start_search(query, case_sensitive, use_regex);
    }

    fn search_next(&mut self) {
        self.search_next();
    }

    fn search_prev(&mut self) {
        self.search_prev();
    }

    fn close_search(&mut self) {
        self.close_search();
    }

    fn get_search_match_info(&self) -> Option<(usize, usize)> {
        self.get_search_match_info()
    }
}

impl Editor {
    // =========================================================================
    // Search Operations
    // =========================================================================

    /// Start search
    pub fn start_search(&mut self, query: String, case_sensitive: bool, use_regex: bool) {
        let mut search_state = SearchState::new(query, case_sensitive).with_regex(use_regex);

        // Perform search throughout document
        self.perform_search(&mut search_state);

        // Find closest match to current cursor
        search_state.find_closest_match(&self.cursor);

        // Move cursor to end of match and create selection
        if let Some(idx) = search_state.current_match {
            if let Some(match_cursor) = search_state.matches.get(idx).cloned() {
                let match_len = search_state.match_len_at(idx);
                let (selection, end_cursor) = search::get_match_selection(&match_cursor, match_len);
                self.cursor = end_cursor;
                self.selection = Some(selection);
            }
        }

        self.search.state = Some(search_state);
    }

    /// Perform search in document
    fn perform_search(&self, search_state: &mut SearchState) {
        search::perform_search(&self.buffer, search_state);
    }

    /// Go to next match
    pub fn search_next(&mut self) {
        if let Some(ref mut search_state) = self.search.state {
            search_state.next_match();
            if let Some(idx) = search_state.current_match {
                if let Some(match_cursor) = search_state.matches.get(idx).cloned() {
                    let match_len = search_state.match_len_at(idx);
                    let (selection, end_cursor) =
                        search::get_match_selection(&match_cursor, match_len);
                    self.cursor = end_cursor;
                    self.selection = Some(selection);
                }
            }
        }
    }

    /// Go to previous match
    pub fn search_prev(&mut self) {
        if let Some(ref mut search_state) = self.search.state {
            search_state.prev_match();
            if let Some(idx) = search_state.current_match {
                if let Some(match_cursor) = search_state.matches.get(idx).cloned() {
                    let match_len = search_state.match_len_at(idx);
                    let (selection, end_cursor) =
                        search::get_match_selection(&match_cursor, match_len);
                    self.cursor = end_cursor;
                    self.selection = Some(selection);
                }
            }
        }
    }

    /// Close search
    pub fn close_search(&mut self) {
        // Preserve the last search/replace query before closing
        if let Some(ref search) = self.search.state {
            if let Some(ref replace_with) = search.replace_with {
                // This is a replace operation - save to replace history
                self.search.last_replace_find = Some(search.query.clone());
                self.search.last_replace_with = Some(replace_with.clone());
            } else {
                // This is a search operation - save to search history
                self.search.last_query = Some(search.query.clone());
            }
        }
        self.search.state = None;
    }

    /// Get search match information (current index, total count)
    pub fn get_search_match_info(&self) -> Option<(usize, usize)> {
        if let Some(ref search) = self.search.state {
            let current = search.current_match.unwrap_or(0);
            let total = search.matches.len();
            Some((current, total))
        } else {
            None
        }
    }

    // =========================================================================
    // Replace Operations
    // =========================================================================

    /// Start search with replace
    pub fn start_replace(
        &mut self,
        query: String,
        replace_with: String,
        case_sensitive: bool,
        use_regex: bool,
    ) {
        let mut search_state = SearchState::new_with_replace(query, replace_with, case_sensitive)
            .with_regex(use_regex);

        // Perform search throughout document
        self.perform_search(&mut search_state);

        // Find closest match to current cursor
        search_state.find_closest_match(&self.cursor);

        // Move cursor to first match and create selection
        if let Some(idx) = search_state.current_match {
            if let Some(match_cursor) = search_state.matches.get(idx).cloned() {
                let match_len = search_state.match_len_at(idx);
                let (selection, end_cursor) = search::get_match_selection(&match_cursor, match_len);
                self.cursor = end_cursor;
                self.selection = Some(selection);
            }
        }

        self.search.state = Some(search_state);
    }

    /// Update replace_with value in active search state without rebuilding search
    pub fn update_replace_with(&mut self, replace_with: String) {
        if let Some(ref mut search) = self.search.state {
            search.replace_with = Some(replace_with);
        }
    }

    /// Replace current match
    pub fn replace_current(&mut self) -> Result<()> {
        // Collect what we need (regex-aware: per-match length + expanded
        // text) before mutating the buffer.
        let (match_cursor, match_len, replace_text) = {
            let Some(search_state) = self.search.state.as_ref() else {
                return Ok(());
            };
            let (Some(_), Some(idx)) = (&search_state.replace_with, search_state.current_match)
            else {
                return Ok(());
            };
            let Some(match_cursor) = search_state.matches.get(idx).cloned() else {
                return Ok(());
            };
            let match_len = search_state.match_len_at(idx);
            let replace_text =
                search::expand_replacement(&self.buffer, &match_cursor, match_len, search_state);
            (match_cursor, match_len, replace_text)
        };

        // Perform replacement
        let result =
            search::replace_at_position(&mut self.buffer, &match_cursor, match_len, &replace_text)?;
        self.cursor = result.new_cursor;

        // Invalidate caches (highlight + wrap) for changed lines
        let is_multiline = replace_text.contains('\n');
        self.invalidate_cache_after_edit(result.start_line, is_multiline);

        // Update search_state
        if let Some(ref mut search_state) = self.search.state {
            if let Some(idx) = search_state.current_match {
                // Remove this match (and its recorded length) from the lists
                search_state.matches.remove(idx);
                if idx < search_state.match_lengths.len() {
                    search_state.match_lengths.remove(idx);
                }

                // Shift remaining same-line matches by the length delta
                search::update_match_positions_after_replace(
                    &mut search_state.matches,
                    &match_cursor,
                    match_len,
                    replace_text.chars().count(),
                );

                // Update current match index
                if search_state.matches.is_empty() {
                    search_state.current_match = None;
                } else if idx >= search_state.matches.len() {
                    search_state.current_match = Some(search_state.matches.len() - 1);
                }

                // Move cursor to next match and create selection
                if let Some(cidx) = search_state.current_match {
                    if let Some(next_cursor) = search_state.matches.get(cidx).cloned() {
                        let next_len = search_state.match_len_at(cidx);
                        let (selection, end_cursor) =
                            search::get_match_selection(&next_cursor, next_len);
                        self.cursor = end_cursor;
                        self.selection = Some(selection);
                    }
                }
            }
        }

        Ok(())
    }

    /// Replace all matches
    pub fn replace_all(&mut self) -> Result<usize> {
        // Use take() instead of clone() to avoid allocation
        let Some(search_state) = self.search.state.take() else {
            return Ok(0);
        };

        if search_state.replace_with.is_none() {
            // Restore state if no replace_with
            self.search.state = Some(search_state);
            return Ok(0);
        }

        // Perform all replacements (regex-aware per-match length + expansion)
        let count = search::replace_all_matches(&mut self.buffer, &search_state)?;

        // Invalidate caches (highlight + wrap) for all affected lines
        if count > 0 {
            self.invalidate_cache_after_edit(0, true);
        }

        Ok(count)
    }

    // =========================================================================
    // Navigation Helpers
    // =========================================================================

    /// Prepare for navigation: close search, clear selection, and close popups.
    pub(crate) fn prepare_for_navigation(&mut self) {
        self.close_search();
        self.selection = None;
        // Close popups on cursor movement
        self.lsp.completion_popup = None;
        self.close_hover_popup();
    }

    /// Prepare for navigation with selection: close search, start/extend selection, and close popups.
    pub(crate) fn prepare_for_navigation_with_selection(&mut self) {
        self.close_search();
        self.start_or_extend_selection();
        // Close popups on cursor movement
        self.lsp.completion_popup = None;
        self.close_hover_popup();
    }

    // =========================================================================
    // Inline find/replace bar
    // =========================================================================

    /// Open (or refocus) the inline find bar. `replace` adds the Replace field
    /// and the Replace / Replace-all buttons. The find field is seeded from the
    /// active or last query, and the search runs immediately.
    pub(crate) fn open_find_bar(&mut self, replace: bool) {
        let rebuild = self
            .find_bar
            .as_ref()
            .map(|b| b.has_field(FindField::Replace) != replace)
            .unwrap_or(true);
        if rebuild {
            let bar = if replace {
                FindBar::new(FindBarConfig {
                    fields: vec![FindField::Find, FindField::Replace],
                    // [Aa] [.*]  ◄ Prev  Next ►  Replace  Replace all
                    buttons: vec![
                        FindBarBtn::Case,
                        FindBarBtn::Regex,
                        FindBarBtn::Prev,
                        FindBarBtn::Next,
                        FindBarBtn::Replace,
                        FindBarBtn::ReplaceAll,
                    ],
                })
            } else {
                FindBar::new(FindBarConfig {
                    fields: vec![FindField::Find],
                    buttons: vec![
                        FindBarBtn::Case,
                        FindBarBtn::Regex,
                        FindBarBtn::Prev,
                        FindBarBtn::Next,
                    ],
                })
            };
            self.find_bar = Some(bar);
        }

        // A single-line selection seeds the Find field (overwriting any prior
        // query) — the common "Ctrl+F searches the current selection" behavior.
        // Multi-line selections are ignored.
        let selection_seed = self
            .get_selected_text()
            .filter(|s| !s.is_empty() && !s.contains('\n'));

        let seed_find = self
            .search
            .state
            .as_ref()
            .map(|s| s.query.clone())
            .or_else(|| self.search.last_query.clone())
            .or_else(|| self.search.last_replace_find.clone());
        let seed_replace = self.search.last_replace_with.clone();

        if let Some(bar) = self.find_bar.as_mut() {
            if let Some(sel) = selection_seed {
                bar.set_text(FindField::Find, sel);
            } else if let Some(q) = seed_find {
                if !q.is_empty() && bar.find_text().is_empty() {
                    bar.set_text(FindField::Find, q);
                }
            }
            if replace {
                if let Some(r) = seed_replace {
                    if !r.is_empty() && bar.replace_text().is_empty() {
                        bar.set_text(FindField::Replace, r);
                    }
                }
                bar.focus_field(FindField::Replace);
            } else {
                bar.focus_field(FindField::Find);
            }
        }
        self.find_bar_focus_buffer = false;
        self.rerun_bar_search();
    }

    /// Whether the inline find bar is open and owns the keyboard (Tab has
    /// not moved focus to the buffer). Hosts that add plain-key shortcuts
    /// must let such keys reach the bar.
    pub fn find_bar_has_focus(&self) -> bool {
        self.find_bar.is_some() && !self.find_bar_focus_buffer
    }

    /// Close the inline bar and clear the search highlight.
    pub(crate) fn close_find_bar(&mut self) {
        self.find_bar = None;
        self.find_bar_focus_buffer = false;
        self.close_search();
    }

    /// Re-run the search from the bar's current fields. Empty query clears it.
    fn rerun_bar_search(&mut self) {
        let Some(bar) = self.find_bar.as_ref() else {
            return;
        };
        let query = bar.find_text().to_string();
        if query.is_empty() {
            self.close_search();
            return;
        }
        let case = bar.case_sensitive();
        let regex = bar.use_regex();
        if bar.has_field(FindField::Replace) {
            let replace = bar.replace_text().to_string();
            self.start_replace(query, replace, case, regex);
        } else {
            self.start_search(query, case, regex);
        }
    }

    /// Replace the current match, surfacing a failure in the status bar
    /// (matches how `replace_all` reports, instead of silently dropping it).
    fn replace_current_event(&mut self) -> Vec<PanelEvent> {
        match self.replace_current() {
            Ok(()) => vec![PanelEvent::NeedsRedraw],
            Err(e) => vec![PanelEvent::SetStatusMessage {
                message: format!("Replace failed: {e}"),
                is_error: true,
            }],
        }
    }

    /// Route a key to the inline bar while it is open. Returns panel events.
    pub(crate) fn handle_find_bar_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Vec<PanelEvent> {
        use crossterm::event::{KeyCode, KeyModifiers};

        // F3 / Shift+F3 step matches regardless of which field has focus.
        if key.code == KeyCode::F(3) {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.search_prev();
            } else {
                self.search_next();
            }
            return vec![PanelEvent::NeedsRedraw];
        }

        let Some(mut bar) = self.find_bar.take() else {
            return vec![];
        };
        let action = bar.handle_key(key);
        let field = bar.focused_field();
        self.find_bar = Some(bar);
        self.apply_find_bar_action(action, field)
    }

    /// Route a mouse event the open bar owns (see [`FindBar::mouse_hits`]) to
    /// it: a click in a field places its cursor and a drag selects, a click on
    /// a control acts. The bar takes the keyboard focus back.
    pub(crate) fn handle_find_bar_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
    ) -> Vec<PanelEvent> {
        let Some(mut bar) = self.find_bar.take() else {
            return vec![];
        };
        let action = bar.handle_mouse(mouse);
        let field = bar.focused_field();
        self.find_bar = Some(bar);
        self.find_bar_focus_buffer = false;
        self.apply_find_bar_action(action, field)
    }

    /// Carry out what the bar asked for, `field` being the one it has focused.
    fn apply_find_bar_action(
        &mut self,
        action: Option<FindBarAction>,
        field: Option<FindField>,
    ) -> Vec<PanelEvent> {
        match action {
            Some(FindBarAction::QueryChanged) => {
                // Editing Replace only updates the replacement; editing Find or
                // flipping a toggle re-runs the search.
                if field == Some(FindField::Replace) {
                    let replace = self
                        .find_bar
                        .as_ref()
                        .map(|b| b.replace_text().to_string())
                        .unwrap_or_default();
                    self.update_replace_with(replace);
                } else {
                    self.rerun_bar_search();
                }
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Refresh) => {
                self.rerun_bar_search();
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Next) => {
                self.search_next();
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Previous) => {
                self.search_prev();
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Replace) => self.replace_current_event(),
            Some(FindBarAction::ReplaceAll) => {
                let count = self.replace_all().unwrap_or(0);
                vec![PanelEvent::SetStatusMessage {
                    message: format!(
                        "Replaced {} occurrence{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ),
                    is_error: false,
                }]
            }
            // Enter on Replace replaces the current match; otherwise step next.
            Some(FindBarAction::Submit) => {
                if field == Some(FindField::Replace) {
                    self.replace_current_event()
                } else {
                    self.search_next();
                    vec![PanelEvent::NeedsRedraw]
                }
            }
            Some(FindBarAction::Close) => {
                self.close_find_bar();
                vec![PanelEvent::NeedsRedraw]
            }
            // The editor's find bar has no per-file selection.
            Some(FindBarAction::SelectAll) | None => {
                vec![PanelEvent::NeedsRedraw]
            }
        }
    }
}
