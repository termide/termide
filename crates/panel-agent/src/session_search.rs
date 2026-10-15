//! The welcome banner's session search (`Ctrl+F`). The prompt box holds the
//! query while it is on, its top border titled so, and the banner lists only
//! the sessions whose titles match, best first, the matched letters
//! highlighted; what the prompt held waits and comes back when the search
//! ends. The arrows walk the list straight from the query, as the prompt's
//! history has no use for them here; `Tab` moves between the query and the
//! list, `Enter` opens the session under the cursor and `Esc` ends the search
//! at once, as every search and filter in termide does.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_core::PanelEvent;
use termide_ui::fuzzy::{rank, Query};
use termide_ui::textarea::TextArea;
use termide_ui::FieldEdit;

use crate::{single_line, truncate_title, AgentPanel, Paste};

/// What the prompt box held when the search took it.
pub(crate) struct SessionSearch {
    draft: TextArea,
    pastes: Vec<Paste>,
    paste_seq: usize,
    shell_mode: bool,
}

impl AgentPanel {
    /// Whether the banner's session search holds the prompt box.
    pub(crate) fn session_search_on(&self) -> bool {
        self.session_search.is_some()
    }

    /// Start the session search, when the banner lists sessions to search:
    /// the prompt box is set aside and emptied for the query. `false` when
    /// there is nothing to search.
    pub(crate) fn start_session_search(&mut self) -> bool {
        if self.session_search.is_some() {
            return true;
        }
        if !self.banner_shown() || self.recent_all.is_empty() {
            return false;
        }
        self.completion = None;
        self.history_pos = None;
        self.session_search = Some(SessionSearch {
            draft: self.input_area().clone(),
            pastes: std::mem::take(&mut self.pastes),
            paste_seq: self.paste_seq,
            shell_mode: std::mem::take(&mut self.shell_mode),
        });
        self.clear_input();
        self.chat_focus = false;
        self.apply_session_filter();
        true
    }

    /// End the session search: the full list and the prompt box's own text
    /// come back. Nothing happens while there is no search.
    pub(crate) fn end_session_search(&mut self) {
        let Some(search) = self.session_search.take() else {
            return;
        };
        *self.input_area_mut() = search.draft;
        self.pastes = search.pastes;
        self.paste_seq = search.paste_seq;
        self.shell_mode = search.shell_mode;
        self.chat_focus = false;
        let selected = self
            .recent_sessions
            .get(self.recent_selected)
            .map(|summary| summary.path.clone());
        self.apply_session_filter();
        self.recent_selected = selected
            .and_then(|path| self.recent_sessions.iter().position(|s| s.path == path))
            .unwrap_or(0);
        self.scroll_recent_selection_into_view();
    }

    /// The query typed changed: the list follows it, the cursor on the best
    /// match and the banner back at its top.
    pub(crate) fn after_search_edit(&mut self) {
        self.apply_session_filter();
        self.recent_selected = 0;
        self.banner_top = 0;
        if self.recent_sessions.is_empty() {
            self.chat_focus = false;
        }
    }

    /// Show the sessions the query matches, best first, with the matched
    /// positions of each shown title; without a search, all of them. The
    /// title is matched as the banner shows it, so the highlight lands on
    /// what is on screen.
    pub(crate) fn apply_session_filter(&mut self) {
        if self.session_search.is_none() {
            self.recent_sessions = self.recent_all.clone();
            self.recent_hits.clear();
            return;
        }
        let mut query = Query::fuzzy(&self.input_area().text());
        let titles: Vec<String> = self
            .recent_all
            .iter()
            .map(|summary| truncate_title(&summary.display_label()))
            .collect();
        let order = rank(titles.iter().map(|title| query.score(title)));
        self.recent_hits = order
            .iter()
            .map(|&index| query.positions(&titles[index]).unwrap_or_default())
            .collect();
        self.recent_sessions = order
            .into_iter()
            .map(|index| self.recent_all[index].clone())
            .collect();
    }

    /// A key while the query has the keyboard. The arrows and the page keys
    /// walk the list, `Enter` opens the session under the cursor, `Tab`
    /// moves into the list, `Esc` ends the search; the editing keys edit the
    /// query. Nothing else reaches the prompt: a query is not a request.
    pub(crate) fn search_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.viewport_height().max(1) as isize;
        match key.code {
            KeyCode::Esc => self.end_session_search(),
            KeyCode::Enter if !shift && !alt => {
                if self.recent_sessions.is_empty() {
                    return vec![];
                }
                self.open_recent_session(self.recent_selected);
            }
            KeyCode::Tab if self.recent_list_shown() => {
                self.chat_focus = true;
                self.scroll_recent_selection_into_view();
            }
            KeyCode::Up if !ctrl && !shift => self.move_recent_selection(-1),
            KeyCode::Down if !ctrl && !shift => self.move_recent_selection(1),
            KeyCode::PageUp => self.move_recent_selection(-page),
            KeyCode::PageDown => self.move_recent_selection(page),
            KeyCode::Char('c') if ctrl && !alt => {
                if !self.copy_input_selection() {
                    return vec![];
                }
            }
            KeyCode::Char('x') if ctrl && !alt => {
                if !self.cut_input_selection() {
                    return vec![];
                }
                self.after_search_edit();
            }
            KeyCode::Char('v') if ctrl && !alt => {
                match termide_ui::clipboard::paste().filter(|text| !text.is_empty()) {
                    Some(text) => self.search_paste(&text),
                    None => return vec![],
                }
            }
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Char(_)
                if !alt =>
            {
                match self.input.edit_field(0, key) {
                    FieldEdit::NotHandled => return vec![],
                    FieldEdit::Edited => self.after_search_edit(),
                    _ => {}
                }
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Paste `text` into the query, on one line: a query has no use for a
    /// large paste's placeholder or a picture.
    pub(crate) fn search_paste(&mut self, text: &str) {
        let text = single_line(text);
        self.input_area_mut().insert_str(&text);
        self.after_search_edit();
    }

    /// Whether F2 and F8 act on the session under the banner list's cursor
    /// rather than on the panel's own: while the list has the keyboard, or
    /// while a search shows its cursor.
    pub(crate) fn recent_list_targeted(&self) -> bool {
        (self.chat_focus || self.session_search_on()) && self.recent_list_shown()
    }

    /// The matched positions of the shown session at `index`, empty when
    /// there is no search.
    pub(crate) fn recent_hit(&self, index: usize) -> &[usize] {
        self.recent_hits.get(index).map_or(&[], Vec::as_slice)
    }
}
