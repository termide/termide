//! Repo/branch selector dropdown filtering for the Git Status Panel.
//!
//! The filter is a text field like any other: typing, cursor movement,
//! selection and the clipboard work through the shared editing grammar
//! ([`termide_ui::edit_text_input`]); the dropdown keeps `↑`/`↓`, `Enter`,
//! `Tab` and `Esc`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_git::{self as git};
use termide_ui::fuzzy::{rank, Query};

use crate::GitStatusPanel;

impl GitStatusPanel {
    /// Return indices into `self.branches` that match the current filter.
    /// When the filter is empty, returns all indices.
    pub(crate) fn filtered_branch_indices(&self) -> Vec<usize> {
        let mut query = Query::fuzzy_path(self.branch_filter.text());
        rank(self.branches.iter().map(|b| query.score(b)))
    }

    /// Index into `self.branches` of the branch the panel shows.
    pub(crate) fn shown_branch_index(&self) -> usize {
        let shown = self.shown_branch();
        self.branches
            .iter()
            .position(|b| Some(b.as_str()) == shown)
            .unwrap_or(0)
    }

    /// Dropdown label of the branch at `idx`: `●` marks the main copy's
    /// branch, `⧉` one checked out in a linked worktree.
    pub(crate) fn branch_label(&self, idx: usize) -> String {
        let name = &self.branches[idx];
        git::branch_label(
            name,
            self.branch.as_deref() == Some(name.as_str()),
            self.worktrees.contains_key(name),
        )
    }

    /// Reset the branch filter state.
    pub(crate) fn reset_branch_filter(&mut self) {
        self.branch_filter.clear();
        self.show_branch_filter = false;
    }

    /// Return indices into repo list that match the current filter, best first.
    ///
    /// The whole path inside the project is scored, not the folder alone: in a
    /// project that groups its checkouts by host and team, `sigma/crm` or even
    /// `devops` has to find the repository its folder name cannot distinguish.
    pub(crate) fn filtered_repo_indices(&self) -> Vec<usize> {
        let repos = self.repo_manager.repos();
        let root = self.repo_manager.project_root();
        let mut query = Query::fuzzy_path(self.repo_filter.text());
        rank(
            repos
                .iter()
                .map(|p| query.score(&git::relative_name(p, root))),
        )
    }

    /// The project's repositories as the tree the dropdown draws: directories as
    /// dim rows, each repository under them, the closed folders' contents left
    /// out. Recomputed rather than stored, so the renderer and the mouse hit-test
    /// can never disagree about a row.
    pub(crate) fn repo_rows(&self) -> Vec<git::RepoRow> {
        self.repo_folds
            .rows(self.repo_manager.repos(), self.repo_manager.project_root())
    }

    /// Open the repository dropdown on the panel's repository: the folders above
    /// it open, the cursor on its row, the window from the top.
    pub(crate) fn open_repo_dropdown(&mut self) {
        self.dropdown_cursor = self.repo_folds.reveal(
            self.repo_manager.repos(),
            self.repo_manager.project_root(),
            self.repo_manager.selected_index(),
        );
        self.dropdown_scroll = 0;
        self.repo_dropdown_open = true;
    }

    /// Open or close the folder at `row` of the repository tree, leaving the
    /// cursor on it; `open` says which, `None` turns it over. `false` when the
    /// row is not a folder, or already as asked.
    pub(crate) fn set_repo_dir_open(&mut self, row: usize, open: Option<bool>) -> bool {
        let rows = self.repo_rows();
        let changed = self.repo_folds.set_open(&rows, row, open);
        if changed {
            self.dropdown_cursor = row;
        }
        changed
    }

    /// The same tree, holding only the repositories that match the filter, best
    /// match first; each row still names its index into the full list.
    pub(crate) fn filtered_repo_rows(&self) -> Vec<git::RepoRow> {
        let repos = self.repo_manager.repos();
        let matched: Vec<(usize, std::path::PathBuf)> = self
            .filtered_repo_indices()
            .into_iter()
            .map(|index| (index, repos[index].clone()))
            .collect();
        git::repo_tree_indexed(&matched, self.repo_manager.project_root())
    }

    /// Reset the repo filter state.
    pub(crate) fn reset_repo_filter(&mut self) {
        self.repo_filter.clear();
        self.show_repo_filter = false;
    }

    /// The filter of the open selector dropdown, with its "shown" flag;
    /// `None` while no dropdown is open.
    pub(crate) fn open_filter(&mut self) -> Option<(&mut termide_ui::TextInput, &mut bool)> {
        if self.branch_dropdown_open {
            Some((&mut self.branch_filter, &mut self.show_branch_filter))
        } else if self.repo_dropdown_open {
            Some((&mut self.repo_filter, &mut self.show_repo_filter))
        } else {
            None
        }
    }

    /// Give `key` to the open dropdown's filter. `false` when it is not the
    /// filter's: the list keys, `Enter`, `Tab`, `Esc` on an empty filter,
    /// `Alt` chords (hotkeys), and a move the field cannot make — so `←`/`→`
    /// on an empty filter still walk the selectors.
    pub(crate) fn handle_filter_key(&mut self, key: KeyEvent) -> bool {
        let Some((filter, shown)) = self.open_filter() else {
            return false;
        };
        if key.code == KeyCode::Esc {
            if filter.is_empty() {
                return false;
            }
            filter.clear();
            *shown = false;
            self.dropdown_cursor = 0;
            return true;
        }
        if key.modifiers.contains(KeyModifiers::ALT)
            || matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Enter
                    | KeyCode::Tab
                    | KeyCode::BackTab
            )
        {
            return false;
        }
        match termide_ui::edit_text_input(filter, key) {
            termide_ui::FieldEdit::NotHandled => false,
            termide_ui::FieldEdit::Navigated => true,
            termide_ui::FieldEdit::Edited => {
                *shown = !filter.is_empty();
                self.dropdown_cursor = 0;
                true
            }
        }
    }

    /// Paste `text` at the open dropdown filter's cursor.
    pub(crate) fn paste_into_filter(&mut self, text: &str) {
        if let Some((filter, shown)) = self.open_filter() {
            filter.paste(text);
            *shown = !filter.is_empty();
            self.dropdown_cursor = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> GitStatusPanel {
        GitStatusPanel::new(&[], None)
    }

    #[test]
    fn empty_branch_filter_returns_all_indices() {
        let mut p = panel();
        p.branches = vec!["main".into(), "dev".into()];
        assert_eq!(p.filtered_branch_indices(), vec![0, 1]);
    }

    #[test]
    fn branch_filter_matches_case_insensitive_substrings() {
        let mut p = panel();
        p.branches = vec![
            "main".into(),
            "feature/login".into(),
            "Feature/Logout".into(),
        ];
        p.branch_filter = termide_ui::TextInput::with_text("log");
        assert_eq!(p.filtered_branch_indices(), vec![1, 2]);
    }

    #[test]
    fn branch_filter_with_no_match_is_empty() {
        // The Enter handler relies on this: an empty result means `.get(cursor)`
        // yields None, so no branch is checked out (previously it fell back to
        // index 0 and checked out the first branch).
        let mut p = panel();
        p.branches = vec!["main".into(), "dev".into()];
        p.branch_filter = termide_ui::TextInput::with_text("zzz");
        assert!(p.filtered_branch_indices().is_empty());
    }

    fn branches_open() -> GitStatusPanel {
        let mut p = panel();
        p.branches = vec!["main".into(), "dev".into(), "feature/login".into()];
        p.branch_dropdown_open = true;
        p
    }

    fn press(p: &mut GitStatusPanel, code: KeyCode, modifiers: KeyModifiers) {
        p.on_key(termide_core::KeyChord::identity(KeyEvent::new(
            code, modifiers,
        )));
    }

    fn type_text(p: &mut GitStatusPanel, text: &str) {
        for ch in text.chars() {
            press(p, KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }

    #[test]
    fn the_branch_filter_edits_at_its_cursor() {
        let mut p = branches_open();
        type_text(&mut p, "lgin");
        for _ in 0..3 {
            press(&mut p, KeyCode::Left, KeyModifiers::NONE);
        }
        type_text(&mut p, "o");
        assert_eq!(p.branch_filter.text(), "login");
        assert!(p.show_branch_filter);
        assert_eq!(p.filtered_branch_indices(), vec![2]);
    }

    #[test]
    fn a_selection_in_the_filter_is_deleted_as_one() {
        let mut p = branches_open();
        type_text(&mut p, "devxx");
        press(&mut p, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut p, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut p, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(p.branch_filter.text(), "dev");
    }

    #[test]
    fn ctrl_v_does_not_type_a_v() {
        let mut p = branches_open();
        type_text(&mut p, "ma");
        // A stand-in clipboard, so the user's is never read or written.
        termide_ui::clipboard::use_fake_clipboard("PASTE");
        let clip = "PASTE";
        press(&mut p, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(p.branch_filter.text(), format!("ma{clip}"));
    }

    #[test]
    fn esc_clears_the_filter_then_closes_and_a_paste_lands_in_the_repo_filter() {
        let mut p = branches_open();
        type_text(&mut p, "x");
        press(&mut p, KeyCode::Esc, KeyModifiers::NONE);
        assert!(p.branch_filter.is_empty());
        assert!(!p.show_branch_filter);
        assert!(p.branch_dropdown_open, "the first Esc only clears");
        press(&mut p, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!p.branch_dropdown_open);

        p.repo_dropdown_open = true;
        let result = termide_core::Panel::handle_command(
            &mut p,
            termide_core::PanelCommand::PasteText { text: "api".into() },
        );
        assert!(matches!(result, termide_core::CommandResult::Handled(true)));
        assert_eq!(p.repo_filter.text(), "api");
        assert!(p.show_repo_filter);
    }

    /// A project of `host/devops/{multica,tardis}` and `host/sigma/crm`, its panel
    /// with every repository found.
    fn grouped_project(name: &str) -> (std::path::PathBuf, GitStatusPanel) {
        let tmp =
            std::env::temp_dir().join(format!("termide-gs-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for repo in [
            "host/devops/multica",
            "host/devops/tardis",
            "host/sigma/crm",
        ] {
            std::fs::create_dir_all(tmp.join(repo).join(".git")).unwrap();
        }
        let root = dunce::canonicalize(&tmp).unwrap();
        let mut p = GitStatusPanel::new(std::slice::from_ref(&root), Some(&root));
        for _ in 0..300 {
            p.repo_manager.poll();
            if p.repo_manager.len() == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(p.repo_manager.len(), 3, "the walk found the repositories");
        p.current_section = crate::types::Section::RepoSelector;
        (tmp, p)
    }

    fn labels(p: &GitStatusPanel) -> Vec<(String, bool)> {
        p.repo_rows()
            .into_iter()
            .map(|r| (r.label, r.expanded))
            .collect()
    }

    // The folders of the repository tree behave as in the file tree: the cursor
    // reaches them, Enter or ←/→ close and open them, a closed one hides what is
    // under it — and only a repository is ever chosen.
    #[test]
    fn the_repository_tree_folds_like_the_file_tree() {
        let (tmp, mut p) = grouped_project("fold");
        press(&mut p, KeyCode::Enter, KeyModifiers::NONE);
        assert!(p.repo_dropdown_open);
        let rows = p.repo_rows();
        assert_eq!(
            rows.len(),
            6,
            "host, devops, two repos, sigma, crm: {rows:?}"
        );

        // Up to `devops`, the second row: a folder takes the cursor.
        while p.dropdown_cursor > 1 {
            press(&mut p, KeyCode::Up, KeyModifiers::NONE);
        }
        press(&mut p, KeyCode::Enter, KeyModifiers::NONE);
        assert!(p.repo_dropdown_open, "a folder is not a choice");
        assert_eq!(
            labels(&p),
            [
                ("host".to_string(), true),
                ("devops".to_string(), false),
                ("sigma".to_string(), true),
                ("crm".to_string(), true),
            ]
        );
        assert_eq!(p.dropdown_cursor, 1, "the cursor stays on the folder");
        press(&mut p, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(p.repo_rows().len(), 6, "→ opens it again");
        press(&mut p, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(p.repo_rows().len(), 4, "← closes it");

        // Down to `crm`, the last row, and choose it.
        press(&mut p, KeyCode::Down, KeyModifiers::NONE);
        press(&mut p, KeyCode::Down, KeyModifiers::NONE);
        press(&mut p, KeyCode::Enter, KeyModifiers::NONE);
        let chosen = p.repo_manager.current().map(|c| c.to_path_buf());
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!p.repo_dropdown_open);
        assert!(
            chosen.is_some_and(|c| c.ends_with("host/sigma/crm")),
            "the repository under the cursor was chosen"
        );
    }

    // A list opened on a repository inside a closed folder opens that folder, so
    // the cursor rests on the repository and not on a row that hides it.
    #[test]
    fn opening_on_a_hidden_repository_opens_its_folder() {
        let (tmp, mut p) = grouped_project("reveal");
        let tardis = p
            .repo_manager
            .repos()
            .iter()
            .position(|r| r.ends_with("tardis"))
            .unwrap();
        p.select_repo(tardis);
        let rows = p.repo_rows();
        let devops = rows.iter().position(|r| r.label == "devops").unwrap();
        p.repo_folds.set_open(&rows, devops, Some(false));
        press(&mut p, KeyCode::Enter, KeyModifiers::NONE);
        let _ = std::fs::remove_dir_all(&tmp);
        let rows = p.repo_rows();
        assert_eq!(
            rows.get(p.dropdown_cursor).and_then(|r| r.repo),
            Some(tardis)
        );
    }
}
