//! Terminal text search: literal/regex matching across scrollback and the
//! visible grid, the inline find bar, and the [`Searchable`] trait impl.
//! Extracted from the panel body; operates on `Terminal`'s private search
//! state via descendant-module visibility.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_core::{PanelEvent, Searchable};
use termide_modal::{FindBar, FindBarAction, FindBarBtn, FindBarConfig, FindField};

use super::terminal::TerminalScreen;
use super::{Terminal, TerminalSearchState};

impl Terminal {
    /// Find all search matches in scrollback + visible buffer. Literal by
    /// default; `use_regex` treats `query` as a regular expression (mirroring
    /// the editor / file-manager `[.*]` toggle). Positions are byte offsets
    /// into the line, consistent with the literal path and the cell renderer.
    fn find_matches(
        screen: &TerminalScreen,
        query: &str,
        case_sensitive: bool,
        use_regex: bool,
    ) -> Vec<(usize, usize, usize)> {
        let mut matches = Vec::new();
        let scrollback_len = screen.scrollback.len();
        let total_lines = scrollback_len + screen.rows;

        // Regex mode: an invalid pattern simply yields no matches.
        let regex = if use_regex {
            match regex::RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(r) => Some(r),
                Err(_) => return matches,
            }
        } else {
            None
        };

        let query_lower = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };

        for abs_row in 0..total_lines {
            let Some(row) = screen.get_line_by_absolute(abs_row) else {
                continue;
            };

            // Extract text from cells
            let line_text: String = row.iter().map(|c| c.ch).collect();

            if let Some(re) = regex.as_ref() {
                for m in re.find_iter(&line_text) {
                    // Skip zero-width matches (e.g. `a*`) — nothing to highlight.
                    if m.end() > m.start() {
                        matches.push((abs_row, m.start(), m.end() - m.start()));
                    }
                }
                continue;
            }

            // Literal path. Cow avoids cloning in the case-sensitive branch.
            let search_text: std::borrow::Cow<str> = if case_sensitive {
                std::borrow::Cow::Borrowed(&line_text)
            } else {
                std::borrow::Cow::Owned(line_text.to_lowercase())
            };

            // Find all occurrences in this line
            let mut start = 0;
            while let Some(pos) = search_text[start..].find(&query_lower) {
                let col = start + pos;
                matches.push((abs_row, col, query_lower.len()));
                start = col + query_lower.len();
                if start >= search_text.len() {
                    break;
                }
            }
        }
        matches
    }

    /// Open (or refocus) the inline find bar and run the current query.
    pub(super) fn open_find_bar(&mut self) {
        if self.find_bar.is_none() {
            // [Aa] Case + [.*] Regex toggles, then ◄ Prev / Next ► — the same
            // row the editor uses (the terminal navigates matches, no replace).
            self.find_bar = Some(FindBar::new(FindBarConfig {
                fields: vec![FindField::Find],
                buttons: vec![
                    FindBarBtn::Case,
                    FindBarBtn::Regex,
                    FindBarBtn::Prev,
                    FindBarBtn::Next,
                ],
            }));
        }
        if let Some(bar) = self.find_bar.as_mut() {
            bar.focus_field(FindField::Find);
        }
        self.find_bar_focus_buffer = false;
        self.rerun_bar_search();
    }

    /// Close the inline bar and clear the search highlight.
    pub(super) fn close_find_bar(&mut self) {
        self.find_bar = None;
        self.find_bar_focus_buffer = false;
        self.close_search();
    }

    /// Re-run the search from the bar's current field + toggles, then refresh
    /// the bar's match counter. An empty query clears the search.
    fn rerun_bar_search(&mut self) {
        let Some(bar) = self.find_bar.as_ref() else {
            return;
        };
        let query = bar.find_text().to_string();
        let case = bar.case_sensitive();
        let regex = bar.use_regex();
        if query.is_empty() {
            self.close_search();
        } else {
            self.start_search(query, case, regex);
        }
        self.update_bar_match_info();
    }

    /// Push the current match position (1-based) and total into the bar's
    /// counter so it shows "3 of 12" inline.
    fn update_bar_match_info(&mut self) {
        let (cur, total) = match self.search_state.as_ref() {
            Some(s) => (s.current_match.map(|i| i + 1).unwrap_or(0), s.matches.len()),
            None => (0, 0),
        };
        if let Some(bar) = self.find_bar.as_mut() {
            bar.set_match_info(cur, total);
        }
    }

    /// Apply a [`FindBarAction`] produced by a key or mouse event on the bar.
    pub(super) fn apply_find_bar_action(
        &mut self,
        action: Option<FindBarAction>,
    ) -> Vec<PanelEvent> {
        match action {
            Some(FindBarAction::QueryChanged) | Some(FindBarAction::Refresh) => {
                self.rerun_bar_search();
            }
            // Enter and Next both step forward; Previous steps back.
            Some(FindBarAction::Next) | Some(FindBarAction::Submit) => {
                self.search_next();
                self.update_bar_match_info();
            }
            Some(FindBarAction::Previous) => {
                self.search_prev();
                self.update_bar_match_info();
            }
            Some(FindBarAction::Close) => {
                self.close_find_bar();
            }
            // The terminal find bar has no replace / per-file selection.
            Some(FindBarAction::Replace)
            | Some(FindBarAction::ReplaceAll)
            | Some(FindBarAction::SelectAll)
            | None => {}
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Route a key to the open find bar. Returns panel events.
    pub(super) fn handle_find_bar_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        // F3 / Shift+F3 step matches regardless of focused control.
        if key.code == KeyCode::F(3) {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.search_prev();
            } else {
                self.search_next();
            }
            self.update_bar_match_info();
            return vec![PanelEvent::NeedsRedraw];
        }

        let Some(mut bar) = self.find_bar.take() else {
            return vec![];
        };
        let action = bar.handle_key(key);
        self.find_bar = Some(bar);
        self.apply_find_bar_action(action)
    }
}

impl Searchable for Terminal {
    fn start_search(&mut self, query: String, case_sensitive: bool, use_regex: bool) {
        if query.is_empty() {
            self.search_state = None;
            self.cached_lines = None;
            return;
        }

        let screen = self.read_screen();
        let matches = Self::find_matches(&screen, &query, case_sensitive, use_regex);
        let scrollback_len = screen.scrollback.len();
        let visible_rows = screen.rows;
        let scroll_offset = screen.scroll_offset;
        drop(screen);

        // Find nearest match to current viewport
        let current_match = if matches.is_empty() {
            None
        } else {
            // Calculate what absolute row is at the center of the viewport
            let total_lines = scrollback_len + visible_rows;
            let view_bottom = total_lines.saturating_sub(scroll_offset);
            let view_center = view_bottom.saturating_sub(visible_rows / 2);

            // Find the match closest to the center of current viewport
            let idx = matches
                .iter()
                .enumerate()
                .min_by_key(|(_, (row, _, _))| {
                    (*row as isize - view_center as isize).unsigned_abs()
                })
                .map(|(i, _)| i);
            idx
        };

        self.search_state = Some(TerminalSearchState {
            matches,
            current_match,
        });

        // Scroll to current match
        if let Some(idx) = current_match {
            if let Some(state) = &self.search_state {
                let abs_row = state.matches[idx].0;
                self.scroll_to_abs_row(abs_row);
            }
        }

        self.cached_lines = None;
    }

    fn search_next(&mut self) {
        let should_scroll = if let Some(ref mut state) = self.search_state {
            if state.matches.is_empty() {
                false
            } else {
                let next = match state.current_match {
                    Some(idx) => (idx + 1) % state.matches.len(),
                    None => 0,
                };
                state.current_match = Some(next);
                true
            }
        } else {
            false
        };

        if should_scroll {
            if let Some(ref state) = self.search_state {
                if let Some(idx) = state.current_match {
                    let abs_row = state.matches[idx].0;
                    self.scroll_to_abs_row(abs_row);
                }
            }
            self.cached_lines = None;
        }
    }

    fn search_prev(&mut self) {
        let should_scroll = if let Some(ref mut state) = self.search_state {
            if state.matches.is_empty() {
                false
            } else {
                let prev = match state.current_match {
                    Some(0) => state.matches.len() - 1,
                    Some(idx) => idx - 1,
                    None => state.matches.len() - 1,
                };
                state.current_match = Some(prev);
                true
            }
        } else {
            false
        };

        if should_scroll {
            if let Some(ref state) = self.search_state {
                if let Some(idx) = state.current_match {
                    let abs_row = state.matches[idx].0;
                    self.scroll_to_abs_row(abs_row);
                }
            }
            self.cached_lines = None;
        }
    }

    fn close_search(&mut self) {
        self.search_state = None;
        self.cached_lines = None;
    }

    fn get_search_match_info(&self) -> Option<(usize, usize)> {
        self.search_state
            .as_ref()
            .and_then(|state| state.current_match.map(|idx| (idx, state.matches.len())))
    }
}
