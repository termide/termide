//! Inline search/replace bar (file-name and in-file content search) plus
//! the file-search state helpers that drive the results view.

use termide_core::PanelEvent;
use termide_modal::{FindBar, FindBarAction, FindBarBtn, FindBarConfig, FindField};

use super::{file_search, FileManager};

/// Which kind of search the inline bar drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchBarKind {
    /// File-name search by glob mask (Ctrl+F): single Find field.
    Name,
    /// In-file content search/replace (Ctrl+Shift+F): Mask/Find/Repl.
    Content,
}

/// Where keyboard focus sits while the inline content bar is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarFocus {
    /// The bar's input fields / buttons consume keys.
    Input,
    /// The results list below the bar consumes keys (arrows move the cursor).
    Results,
}

impl FileManager {
    // ── File/content search methods ─────────────────────────────────────

    /// Start file glob search
    pub fn start_file_search(&mut self, glob_mask: &str, use_regex: bool, case_sensitive: bool) {
        let mut state = file_search::FileSearchState::new_file_glob(self.current_path.clone());
        state.start_file_search(glob_mask, use_regex, case_sensitive);
        self.file_search = Some(state);
    }

    /// Start content search (glob mask + query). `use_regex` treats the query
    /// as a regular expression; otherwise it is matched literally.
    pub fn start_content_search(
        &mut self,
        glob_mask: &str,
        query: &str,
        use_regex: bool,
        case_sensitive: bool,
    ) {
        let max_file_size = self.cached_config.content_search_max_file_size_mb * 1024 * 1024;
        let mut state =
            file_search::FileSearchState::new_content(self.current_path.clone(), max_file_size);
        state.start_content_search(glob_mask, query, use_regex, case_sensitive);
        self.file_search = Some(state);
    }

    /// Navigate to next search result
    pub fn search_next(&mut self) {
        if let Some(ref mut state) = self.file_search {
            state.next_result();
        }
    }

    /// Navigate to previous search result
    pub fn search_prev(&mut self) {
        if let Some(ref mut state) = self.file_search {
            state.prev_result();
        }
    }

    /// Close file search and return to normal tree view
    pub fn close_file_search(&mut self) {
        self.file_search = None;
    }

    /// Set the in-progress replacement text for the content search (drives the
    /// `-old/+new` preview on the cursor match).
    pub fn set_content_replace(&mut self, text: Option<String>) {
        if let Some(ref mut state) = self.file_search {
            state.set_replace_text(text);
        }
    }

    /// Apply `replace_with` to every matched file of the active content search.
    /// Returns (files_changed, occurrences_replaced).
    pub fn replace_all_in_content_results(&mut self, replace_with: &str) -> (usize, usize) {
        match self.file_search.as_ref() {
            Some(state) => state.replace_all(replace_with),
            None => (0, 0),
        }
    }

    /// Close search and apply the selected result.
    /// For FileGlob: navigates to the file in the tree.
    /// For Content: returns `Some(PanelEvent::OpenFileAt { .. })` so the caller can open the file.
    pub fn close_search_with_selection(&mut self) -> Option<PanelEvent> {
        use super::file_search::SelectedSearchResult;
        let selection = self.file_search.as_ref()?.get_selected_result();
        self.close_file_search();
        match selection? {
            SelectedSearchResult::NavigateToFile(path) => {
                self.navigate_to_file(&path);
                None
            }
            SelectedSearchResult::OpenAtLine { path, line } => Some(PanelEvent::OpenFileAt {
                path,
                line,
                column: 0,
            }),
            SelectedSearchResult::OpenDir(path) => {
                // Enter (cd into) the directory.
                self.current_path = std::fs::canonicalize(&path).unwrap_or(path);
                self.selected = 0;
                self.scroll_offset = 0;
                let _ = self.load_directory();
                None
            }
        }
    }

    // === Inline content search / replace bar (Ctrl+Shift+F) ===

    /// Open the inline content bar. `replace = false` is a pure search
    /// (Ctrl+Shift+F); `replace = true` adds the replacement field
    /// (Ctrl+Shift+H).
    pub fn open_content_bar(&mut self, replace: bool) {
        self.open_search_bar(SearchBarKind::Content, replace);
    }

    /// Open (or re-focus) the inline file-name search bar (single glob field).
    pub fn open_name_bar(&mut self) {
        self.open_search_bar(SearchBarKind::Name, false);
    }

    /// Open the inline bar for `kind`, rebuilding it if the kind or the
    /// search/replace shape changed. Field texts are preserved across a
    /// rebuild. The file manager has no left-hand content, so navigation lives
    /// in the results zone (Tab) rather than Prev/Next buttons.
    fn open_search_bar(&mut self, kind: SearchBarKind, replace: bool) {
        let has_replace = self
            .search_bar
            .as_ref()
            .map(|b| b.has_field(FindField::Replace));
        let rebuild = self.search_bar.is_none()
            || self.bar_kind != kind
            || (kind == SearchBarKind::Content && has_replace != Some(replace));

        if rebuild {
            // Preserve typed text across a rebuild (e.g. Search → Replace).
            let (mask, find, repl) = self
                .search_bar
                .as_ref()
                .map(|b| {
                    (
                        b.mask_text().to_string(),
                        b.find_text().to_string(),
                        b.replace_text().to_string(),
                    )
                })
                .unwrap_or_default();

            let mut bar = match kind {
                SearchBarKind::Name => FindBar::new(FindBarConfig {
                    fields: vec![FindField::Find],
                    // `[Aa]` case + `[.*]` regex, like the content form.
                    buttons: vec![FindBarBtn::Case, FindBarBtn::Regex],
                }),
                SearchBarKind::Content => {
                    let mut fields = vec![FindField::Mask, FindField::Find];
                    // Toggles first (`[Aa] [.*]`), then selection + replace.
                    let mut buttons = vec![FindBarBtn::Case, FindBarBtn::Regex];
                    if replace {
                        fields.push(FindField::Replace);
                        buttons.push(FindBarBtn::SelectAll);
                        buttons.push(FindBarBtn::ReplaceAll);
                    }
                    let mut bar = FindBar::new(FindBarConfig { fields, buttons });
                    // The glob field reads as "Find:" (matching the name
                    // search), the content query as "Text:".
                    bar.set_label(FindField::Mask, "Find: ");
                    bar.set_label(FindField::Find, "Text: ");
                    // The replace-all button acts on selected files → "Replace".
                    if replace {
                        bar.set_button_label(FindBarBtn::ReplaceAll, "Replace");
                    }
                    bar.set_text(
                        FindField::Mask,
                        if mask.is_empty() { "*".into() } else { mask },
                    );
                    bar
                }
            };
            if !find.is_empty() {
                bar.set_text(FindField::Find, find);
            }
            if replace && !repl.is_empty() {
                bar.set_text(FindField::Replace, repl);
            }
            self.search_bar = Some(bar);
            self.bar_kind = kind;
        }
        if let Some(bar) = self.search_bar.as_mut() {
            // Focus the query field (Find = the text/glob you type first).
            bar.focus_field(FindField::Find);
        }
        self.bar_focus = BarFocus::Input;
        self.rerun_search();
        self.sync_bar_status();
    }

    /// Close the inline bar and clear its search results.
    fn close_search_bar(&mut self) -> Vec<PanelEvent> {
        self.search_bar = None;
        self.close_file_search();
        self.bar_focus = BarFocus::Input;
        vec![PanelEvent::NeedsRedraw]
    }

    /// Re-run the search from the bar's current fields (called on every edit /
    /// toggle). An empty query clears the results. Dispatches on the bar kind.
    pub(crate) fn rerun_search(&mut self) {
        let Some(bar) = self.search_bar.as_ref() else {
            return;
        };
        let query = bar.find_text().to_string();
        if query.is_empty() {
            self.file_search = None;
            return;
        }
        match self.bar_kind {
            SearchBarKind::Name => {
                self.start_file_search(&query, bar.use_regex(), bar.case_sensitive())
            }
            SearchBarKind::Content => {
                let mask = bar.mask_text().to_string();
                let mask = if mask.is_empty() { "*" } else { mask.as_str() };
                let use_regex = bar.use_regex();
                let case_sensitive = bar.case_sensitive();
                let replace = bar.replace_text().to_string();
                // Replace bar (it has the Repl field) → per-file selection.
                let replace_mode = bar.has_field(FindField::Replace);
                self.start_content_search(mask, &query, use_regex, case_sensitive);
                self.set_content_replace((!replace.is_empty()).then_some(replace));
                if let Some(s) = self.file_search.as_mut() {
                    s.set_replace_mode(replace_mode);
                }
            }
        }
    }

    /// Update only the replacement-preview text without restarting the search
    /// (editing the Replace field must not re-run the query or reset the
    /// cursor).
    fn sync_replace_preview(&mut self) {
        let replace = match self.search_bar.as_ref() {
            Some(bar) => bar.replace_text().to_string(),
            None => return,
        };
        self.set_content_replace((!replace.is_empty()).then_some(replace));
    }

    /// Build the "replace N matches in M files?" confirmation event from the
    /// **selected** files, or a hint when nothing is selected.
    pub(crate) fn content_replace_all_event(&self) -> Vec<PanelEvent> {
        let Some(bar) = self.search_bar.as_ref() else {
            return vec![];
        };
        let replace = bar.replace_text().to_string();
        let summary = self.file_search.as_ref().map(|s| s.selected_summary());
        match summary {
            Some((files, matches)) if files > 0 && matches > 0 => vec![PanelEvent::ShowConfirm {
                message: termide_i18n::t().replace_confirm_fmt(matches, files),
                on_confirm: termide_core::ConfirmAction::ReplaceInContent(replace),
            }],
            _ => vec![PanelEvent::ShowMessage(
                termide_i18n::t().replace_no_files_selected().to_string(),
            )],
        }
    }

    /// Route a key to the inline bar (called from `handle_key` while the bar is
    /// open). Returns the panel events produced.
    pub(crate) fn handle_search_bar_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Vec<PanelEvent> {
        let events = match self.bar_focus {
            BarFocus::Input => self.handle_bar_input_key(key),
            BarFocus::Results => self.handle_bar_results_key(key),
        };
        self.sync_bar_status();
        events
    }

    /// Refresh the bar's right-aligned status: the "N of M" found counter, and
    /// in replace mode the selected-files / replacement summary.
    pub(crate) fn sync_bar_status(&mut self) {
        let data = self.file_search.as_ref().map(|s| {
            (
                s.get_match_info(),
                s.show_checkboxes,
                s.selected_summary(),
                s.all_selected(),
            )
        });
        if let Some(bar) = self.search_bar.as_mut() {
            let (info, replace_mode, (sel_files, sel_matches), all_selected) =
                data.unwrap_or((None, false, (0, 0), false));
            match info {
                Some((c, t)) => bar.set_match_info(c + 1, t),
                None => bar.clear_match_info(),
            }
            bar.set_select_all(all_selected);
            if replace_mode {
                let total = info.map(|(_, t)| t).unwrap_or(0);
                bar.set_info_text(Some(termide_i18n::t().replace_selection_fmt(
                    sel_files,
                    total,
                    sel_matches,
                )));
            } else {
                bar.set_info_text(None);
            }
        }
    }

    fn handle_bar_input_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<PanelEvent> {
        use crossterm::event::KeyCode;

        // Tab / Shift+Tab switch to the results zone (where arrows walk the
        // matches), like section switching in the git-status panel. Within the
        // bar, fields and buttons are reached with arrow keys.
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            if self.file_search.is_some() {
                self.bar_focus = BarFocus::Results;
            }
            return vec![PanelEvent::NeedsRedraw];
        }

        let Some(mut bar) = self.search_bar.take() else {
            return vec![];
        };
        let action = bar.handle_key(key);
        let field = bar.focused_field();
        self.search_bar = Some(bar);

        match action {
            Some(FindBarAction::QueryChanged) => {
                // Editing the Replace field only refreshes the preview; editing
                // Find/Mask or flipping a toggle re-runs the search.
                if field == Some(FindField::Replace) {
                    self.sync_replace_preview();
                } else {
                    self.rerun_search();
                }
                vec![PanelEvent::NeedsRedraw]
            }
            Some(FindBarAction::Refresh) => {
                // Re-run against current directory contents (Ctrl+R).
                self.rerun_search();
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
            Some(FindBarAction::ReplaceAll) => self.content_replace_all_event(),
            Some(FindBarAction::SelectAll) => {
                // Toggle: select all when not everything is selected, else clear.
                if let Some(s) = self.file_search.as_mut() {
                    let all = s.all_selected();
                    s.set_all_selected(!all);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            // Enter on the Replace field replaces all (with confirmation);
            // Enter on Mask/Find jumps focus into the results list.
            Some(FindBarAction::Submit) => {
                if field == Some(FindField::Replace) {
                    self.content_replace_all_event()
                } else {
                    self.bar_focus = BarFocus::Results;
                    vec![PanelEvent::NeedsRedraw]
                }
            }
            Some(FindBarAction::Close) => self.close_search_bar(),
            // No per-match Replace button is configured for the file manager.
            Some(FindBarAction::Replace) | None => vec![PanelEvent::NeedsRedraw],
        }
    }

    fn handle_bar_results_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<PanelEvent> {
        use crossterm::event::KeyCode;
        // Page size = the visible results height (fallback to a sane default).
        let page = self
            .search_results_area
            .map(|r| r.height as usize)
            .unwrap_or(10)
            .max(1);
        match key.code {
            KeyCode::Up => {
                self.search_prev();
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Down => {
                self.search_next();
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::PageUp => {
                if let Some(s) = self.file_search.as_mut() {
                    s.page_up(page);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::PageDown => {
                if let Some(s) = self.file_search.as_mut() {
                    s.page_down(page);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            // Replace mode: Space toggles the file's checkbox, `a` toggles all.
            KeyCode::Char(' ') => {
                if let Some(s) = self.file_search.as_mut() {
                    s.toggle_selected_at_cursor();
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Char('a') => {
                if let Some(s) = self.file_search.as_mut() {
                    let all = s.all_selected();
                    s.set_all_selected(!all);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            // Collapse / expand the file group at the cursor (content mode).
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(s) = self.file_search.as_mut() {
                    s.set_collapse_at_cursor(true);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(s) = self.file_search.as_mut() {
                    s.set_collapse_at_cursor(false);
                }
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(bar) = self.search_bar.as_mut() {
                    bar.focus_first();
                }
                self.bar_focus = BarFocus::Input;
                vec![PanelEvent::NeedsRedraw]
            }
            KeyCode::Enter => {
                // Enter opens the selection (a file header opens the file at its
                // first match); on a directory it toggles collapse instead.
                let opens = self
                    .file_search
                    .as_ref()
                    .and_then(|s| s.get_selected_result())
                    .is_some();
                if opens {
                    let open = self.close_search_with_selection();
                    self.search_bar = None;
                    self.bar_focus = BarFocus::Input;
                    let mut out = vec![PanelEvent::NeedsRedraw];
                    out.extend(open);
                    out
                } else {
                    if let Some(s) = self.file_search.as_mut() {
                        s.toggle_collapse_at_cursor();
                    }
                    vec![PanelEvent::NeedsRedraw]
                }
            }
            KeyCode::Esc => self.close_search_bar(),
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_file_manager_in_temp() -> (FileManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let fm = FileManager::new_with_path(temp_dir.path().to_path_buf());
        (fm, temp_dir)
    }

    #[test]
    fn content_bar_opens_focused_on_input_and_esc_closes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut fm, _t) = create_file_manager_in_temp();

        fm.open_content_bar(false);
        assert!(fm.search_bar.is_some());
        assert_eq!(fm.bar_focus, BarFocus::Input);

        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(fm.search_bar.is_none());
        assert!(fm.file_search.is_none());
    }

    #[test]
    fn typing_query_starts_search_and_enter_focuses_results() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut fm, _t) = create_file_manager_in_temp();

        fm.open_content_bar(false); // focus lands on the Find field
        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(
            fm.file_search.is_some(),
            "typing a query should start a content search"
        );

        // Enter on Find jumps focus into the results list for arrow navigation.
        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(fm.bar_focus, BarFocus::Results);
    }

    #[test]
    fn clearing_query_clears_results() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut fm, _t) = create_file_manager_in_temp();

        fm.open_content_bar(false);
        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(fm.file_search.is_some());

        fm.handle_search_bar_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            fm.file_search.is_none(),
            "emptying the query should clear the results"
        );
    }
}
