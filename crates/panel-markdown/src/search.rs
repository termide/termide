//! Incremental in-panel search (find bar, match tracking, navigation).

use termide_core::PanelEvent;
use termide_modal::{FindBar, FindBarAction, FindBarBtn, FindBarConfig, FindField};

use crate::text::{find_in_line, slice_chars};
use crate::MarkdownPanel;

impl MarkdownPanel {
    pub(crate) fn open_find(&mut self) {
        let mut bar = FindBar::new(FindBarConfig {
            fields: vec![FindField::Find],
            // Same button order as the editor: [Aa] Case, ◄ Prev, Next ►.
            buttons: vec![FindBarBtn::Case, FindBarBtn::Prev, FindBarBtn::Next],
        });
        // Seed the Find field from a single-line selection (the common
        // "Ctrl+F searches the current selection" behavior).
        let seed = self.selection().and_then(|(s, e)| {
            (s.0 == e.0 && s != e).then(|| slice_chars(&self.line_text(s.0), s.1, e.1))
        });
        if let Some(text) = seed {
            bar.set_text(FindField::Find, text);
        }
        bar.focus_first();
        self.find_bar = Some(bar);
        self.matches.clear();
        self.match_idx = 0;
        self.run_search();
    }

    pub(crate) fn close_find(&mut self) {
        self.find_bar = None;
        self.matches.clear();
    }

    /// Re-run the search and jump to the first match at/after the cursor.
    pub(crate) fn run_search(&mut self) {
        let Some(bar) = self.find_bar.as_ref() else {
            return;
        };
        let query = bar.find_text().to_string();
        let ci = !bar.case_sensitive();
        self.matches.clear();
        self.match_idx = 0;
        if query.is_empty() {
            if let Some(bar) = self.find_bar.as_mut() {
                bar.clear_match_info();
            }
            return;
        }
        self.match_len = query.chars().count();
        for line in 0..self.line_count() {
            for col in find_in_line(&self.line_text(line), &query, ci) {
                self.matches.push((line, col));
            }
        }
        // Prefer the first match at/after the current cursor.
        if let Some(idx) = self.matches.iter().position(|&m| m >= self.cursor) {
            self.match_idx = idx;
        }
        if let Some(bar) = self.find_bar.as_mut() {
            if self.matches.is_empty() {
                bar.set_match_info(0, 0);
            } else {
                bar.set_match_info(self.match_idx + 1, self.matches.len());
            }
        }
        self.jump_to_current_match();
    }

    pub(crate) fn step_match(&mut self, forward: bool) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len();
        self.match_idx = if forward {
            (self.match_idx + 1) % n
        } else {
            (self.match_idx + n - 1) % n
        };
        if let Some(bar) = self.find_bar.as_mut() {
            bar.set_match_info(self.match_idx + 1, n);
        }
        self.jump_to_current_match();
    }

    fn jump_to_current_match(&mut self) {
        if let Some(&(line, col)) = self.matches.get(self.match_idx) {
            self.anchor = None;
            self.cursor = (line, col);
            self.clamp_cursor();
            self.ensure_cursor_visible();
        }
    }

    pub(crate) fn handle_find_action(&mut self, action: FindBarAction) -> Vec<PanelEvent> {
        match action {
            FindBarAction::QueryChanged | FindBarAction::Refresh => self.run_search(),
            FindBarAction::Next | FindBarAction::Submit => self.step_match(true),
            FindBarAction::Previous => self.step_match(false),
            FindBarAction::Close => self.close_find(),
            _ => {}
        }
        vec![PanelEvent::NeedsRedraw]
    }
}
