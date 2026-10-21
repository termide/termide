//! Git Log Panel for termide.
//!
//! Provides a panel for viewing git commit history with graph visualization.

mod actions;
mod refresh;
mod rendering;
mod selection;

use refresh::{GitLogRefreshResult, LogView, Reload};

use std::any::Any;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};

use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up, Config, KeyBinding};
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, PanelState, RenderContext,
    ThemeColors, WidthPreference,
};
use termide_git::{self as git, CommitInfo, LogStream, RepoManager};
use termide_modal::ActiveModal;
use termide_state::PendingAction;
use termide_theme::Theme;

/// Section of the Git Log panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Repository selector
    RepoSelector,
    /// Branch selector
    BranchSelector,
    /// Commit list
    Commits,
}

/// Git Log Panel
pub struct GitLogPanel {
    /// Repository manager
    repo_manager: RepoManager,
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
    /// A repo update arrived while the panel was collapsed to its title bar;
    /// the log reloads once the panel shows content again.
    is_stale: bool,
    /// Current section
    current_section: Section,
    /// Current branch name (HEAD)
    branch: Option<String>,
    /// All branches for the current repo
    branches: Vec<String>,
    /// Selected branch to view log for (None = current HEAD)
    selected_branch: Option<String>,
    /// Branches checked out in a linked worktree
    worktrees: HashSet<String>,
    /// Whether the repo dropdown is open
    repo_dropdown_open: bool,
    /// Whether the branch dropdown is open
    branch_dropdown_open: bool,
    /// Cursor position in the open dropdown
    dropdown_cursor: usize,
    /// Cached area of the open dropdown for click detection
    dropdown_area: Option<Rect>,
    /// Cached area of the repo selector widget
    repo_selector_area: Option<Rect>,
    /// Cached area of the branch selector widget
    branch_selector_area: Option<Rect>,
    /// Commit log rows read so far: commits and a graph's connector rows
    commits: Vec<CommitInfo>,
    /// Currently selected commit index
    selected: usize,
    /// Scroll offset
    scroll: usize,
    /// The running `git log` the rows come from, read as far as the view needs
    log: Option<LogStream>,
    /// What `log` shows, so a refresh of the same view keeps its place
    log_view: Option<LogView>,
    /// A restart of the shown view waiting for its first rows
    reload: Option<Reload>,
    /// A request to `log` is in flight
    log_waiting: bool,
    /// `log` has no more history
    log_done: bool,
    /// Commits in the whole history, once counted
    total_commits: Option<usize>,
    /// Commits among `commits`
    loaded_commits: usize,
    /// Widest box-drawing graph among `commits`, which the text lines up after
    graph_width: usize,
    /// `End` was pressed while history remained: keep reading and keep the
    /// selection on the last row
    follow_end: bool,
    /// A thumb drag went past the rows read: scroll there once they are in
    scroll_target: Option<usize>,
    /// Cached theme colors
    cached_theme: ThemeColors,
    /// Last render area
    last_area: Rect,
    /// Status message
    status_message: Option<String>,
    /// Cached vim_mode setting for keyboard handling
    vim_mode: bool,
    /// Draw the commit graph with the box-drawing layout engine
    /// (`git_log.unicode_graph`); when false, use git's ASCII `--graph`.
    unicode_graph: bool,
    /// Pending modal request for the app to pick up
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,
    /// In-flight async refresh. While `Some`, the heavy
    /// `get_all_branches` / `get_log_with_graph` calls run on a worker
    /// thread; `tick()` swaps the result into place when ready.
    refresh_rx: Option<std::sync::mpsc::Receiver<GitLogRefreshResult>>,
    /// Set when `refresh()` is called while a worker is already in flight, so a
    /// single follow-up pass runs once it lands instead of stacking threads
    /// (and git subprocesses) per watcher event during a `.git` storm.
    refresh_pending: bool,
}

/// Build HotkeyTable for the git log panel from config.
fn build_git_log_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.git_log.keybindings;

    t.insert("info", &kb.info);
    t.insert("view_diff", &kb.view_diff);

    // open_external: shared config binding (file_manager.keybindings.open_external)
    let fm_kb = &config.file_manager.keybindings;
    if let Some(ref binding) = fm_kb.open_external {
        let mut keys: Vec<String> = match binding {
            KeyBinding::Single(s) => vec![s.clone()],
            KeyBinding::Multiple(v) => v.clone(),
        };
        if !keys.iter().any(|k| k == "O") {
            keys.push("O".into());
        }
        t.insert("open_external", &Some(KeyBinding::Multiple(keys)));
    } else {
        t.insert(
            "open_external",
            &Some(KeyBinding::Multiple(vec!["O".into(), "Alt+Enter".into()])),
        );
    }

    t.insert("checkout", &kb.checkout);
    t
}

impl GitLogPanel {
    fn create(repo_manager: RepoManager) -> Self {
        let mut panel = Self {
            repo_manager,
            scrollbars: termide_core::ScrollBars::default(),
            is_stale: false,
            current_section: Section::Commits,
            branch: None,
            branches: Vec::new(),
            selected_branch: None,
            worktrees: HashSet::new(),
            repo_dropdown_open: false,
            branch_dropdown_open: false,
            dropdown_cursor: 0,
            dropdown_area: None,
            repo_selector_area: None,
            branch_selector_area: None,
            commits: Vec::new(),
            selected: 0,
            scroll: 0,
            log: None,
            log_view: None,
            reload: None,
            log_waiting: false,
            log_done: false,
            total_commits: None,
            loaded_commits: 0,
            graph_width: 0,
            follow_end: false,
            scroll_target: None,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            status_message: None,
            vim_mode: false,
            // Default on; prepare_render syncs it from config before the first
            // user-driven refresh. Matches `GitLogSettings::default()`.
            unicode_graph: true,
            modal_request: None,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            refresh_rx: None,
            refresh_pending: false,
        };
        panel.refresh();
        panel
    }

    /// Create a new Git Log panel from a list of paths (from panels/layout),
    /// scoped to the project at `project_root` when one is known.
    pub fn new(paths: &[PathBuf], project_root: Option<&Path>) -> Self {
        Self::create(RepoManager::new(paths, project_root))
    }

    /// Create panel for a specific repository (used for layout restore),
    /// keeping the project's repositories in the dropdown when a
    /// `project_root` is known.
    pub fn new_for_repo(repo_path: PathBuf, project_root: Option<&Path>) -> Self {
        Self::create(RepoManager::for_repo(repo_path, project_root))
    }

    /// Index into `self.branches` of the branch whose history is shown.
    pub(crate) fn shown_branch_index(&self) -> usize {
        let shown = self.selected_branch.as_deref().or(self.branch.as_deref());
        self.branches
            .iter()
            .position(|b| Some(b.as_str()) == shown)
            .unwrap_or(0)
    }

    /// Branch dropdown labels: `●` marks HEAD, `⧉` a branch checked out in
    /// a linked worktree.
    pub(crate) fn branch_labels(&self) -> Vec<String> {
        self.branches
            .iter()
            .map(|name| {
                git::branch_label(
                    name,
                    self.branch.as_deref() == Some(name.as_str()),
                    self.worktrees.contains(name),
                )
            })
            .collect()
    }

    /// Show `branch` (`None` = HEAD) of the repository at `repo_path`; a
    /// repository not in the list leaves the selection as it is.
    fn show(&mut self, repo_path: &Path, branch: Option<String>) {
        if self.repo_manager.select_path(repo_path) {
            self.selected_branch = branch;
        }
        self.branch_dropdown_open = false;
        self.repo_dropdown_open = false;
        self.selected = 0;
        self.refresh();
    }

    /// Update repository list based on new paths from panels
    pub fn update_repos(&mut self, paths: &[PathBuf]) {
        if self.repo_manager.update(paths) {
            self.refresh();
        }
    }
}

impl Panel for GitLogPanel {
    fn name(&self) -> &'static str {
        "git_log"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        let t = termide_i18n::t();
        let repo_name = self
            .repo_manager
            .current()
            .map(git::get_repo_name)
            .unwrap_or_else(|| t.git_no_repo().to_string());
        let branch = self
            .selected_branch
            .as_deref()
            .or(self.branch.as_deref())
            .unwrap_or(t.git_branch_detached());
        t.git_log_title_fmt(&repo_name, branch)
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        self.cached_theme = ThemeColors::from(theme);
        self.vim_mode = config.general.vim_mode;
        self.unicode_graph = config.git_log.unicode_graph;
        let config_ptr = std::sync::Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            self.hotkeys = build_git_log_hotkey_table(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_area = area;
        self.render_content(area, buf, ctx.is_focused, ctx.border_right_x);
    }

    fn close_popups(&mut self) {
        if self.repo_dropdown_open || self.branch_dropdown_open {
            self.repo_dropdown_open = false;
            self.branch_dropdown_open = false;
            self.dropdown_cursor = 0;
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::Reload => {
                self.is_stale = false;
                self.refresh();
                CommandResult::NeedsRedraw(true)
            }
            // Stale-on-collapse: while the panel is a bare title bar the app
            // sends MarkStale instead of OnGitUpdate; the reload happens once
            // the panel shows content again. Asked every tick, so it must be
            // a no-op when nothing is stale.
            PanelCommand::MarkStale => {
                self.is_stale = true;
                CommandResult::None
            }
            PanelCommand::RefreshIfStale => {
                if self.is_stale {
                    self.is_stale = false;
                    self.refresh();
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::None
                }
            }
            PanelCommand::UpdateRepoPaths { paths } => {
                self.update_repos(&paths);
                CommandResult::NeedsRedraw(true)
            }
            PanelCommand::ShowGitLog { repo_path, branch } => {
                self.show(&repo_path, branch);
                CommandResult::NeedsRedraw(true)
            }
            // Live watcher updates: a commit / ref / rebase touches `.git` and
            // arrives as OnGitUpdate — reload the log if it's this repo. (Plain
            // working-tree edits don't change the graph, so OnFsUpdate is
            // ignored.)
            PanelCommand::OnGitUpdate { repo_paths } => {
                let hit = self
                    .repo_manager
                    .current()
                    .is_some_and(|cur| git::repo_paths_overlap(cur, repo_paths));
                if hit {
                    self.refresh();
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.follow_end = false;
                // The scrollbar spans history not read yet; dragged past the
                // rows read, the view waits at their end until the rest is in.
                let visible = self.scrollbars.vertical.map_or(1, |bar| bar.visible).max(1);
                let reachable = self.commits.len().saturating_sub(visible);
                self.scroll_target = (offset > reachable && self.has_more()).then_some(offset);
                self.scroll = offset.min(reachable);
                // A thumb drag moves the window, so pull the selection into
                // it, as the wheel does — otherwise the highlight sits off
                // screen and the next arrow key jumps back.
                let offset = self.scroll;
                let last = self.commits.len().saturating_sub(1);
                let low = offset.min(last);
                let high = (offset + visible - 1).min(last);
                self.selected = self.selected.clamp(low, high);
                self.ensure_loaded();
                CommandResult::NeedsRedraw(true)
            }
            PanelCommand::Copy => {
                if let Some(commit) = self.selected_commit() {
                    if !commit.hash.is_empty() {
                        let hash = commit.hash.clone();
                        let _ = termide_clipboard::copy(&hash);
                        self.status_message = Some(format!("Copied: {}", hash));
                    }
                }
                CommandResult::Handled(true)
            }
            PanelCommand::Cut | PanelCommand::Paste => CommandResult::Handled(false),
            _ => CommandResult::None,
        }
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();
        // Repository discovery (submodules, and nested repos under a non-repo
        // root) runs in the background; pull its result in so the repo dropdown
        // reflects the full list once available without blocking the constructor.
        let before = self.repo_manager.current().map(|p| p.to_path_buf());
        if self.repo_manager.poll() {
            events.push(PanelEvent::NeedsRedraw);
            let after = self.repo_manager.current().map(|p| p.to_path_buf());
            if after.is_none() {
                // Repo(s) vanished (e.g. `.git` deleted) — drop stale commits.
                if before.is_some() {
                    self.clear_git_state();
                }
            } else if before != after {
                // Newly discovered repo, or current one removed and selection
                // moved — load its log (poll() only fills the list).
                self.refresh();
            }
        }
        // Async refresh worker — swap branches / commits into place
        // once `git log` finishes off the UI thread.
        if self.poll_refresh() {
            events.push(PanelEvent::NeedsRedraw);
        }
        if self.poll_log() {
            events.push(PanelEvent::NeedsRedraw);
        }
        events
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        // No text input here: every key is a shortcut, matched on the
        // layout-normalized form so it works on a Cyrillic layout too.
        let key = chord.canonical;
        // Clear status message on any key
        self.status_message = None;
        // Any key but `End` itself stops reading to the end.
        self.follow_end = false;
        let events = self.handle_key_inner(key);
        self.ensure_loaded();
        events
    }

    fn handle_mouse(&mut self, event: MouseEvent, panel_area: Rect) -> Vec<PanelEvent> {
        self.follow_end = false;
        let events = self.handle_mouse_inner(event, panel_area);
        self.ensure_loaded();
        events
    }

    fn handle_scroll(&mut self, delta: i32, panel_area: Rect) -> Vec<PanelEvent> {
        self.follow_end = false;
        let events = self.handle_scroll_inner(delta, panel_area);
        self.ensure_loaded();
        events
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        self.repo_manager.current().map(|repo| PanelState::GitLog {
            repo_path: repo.to_path_buf(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.repo_manager.current().map(|p| p.to_path_buf())
    }
}

impl GitLogPanel {
    fn handle_key_inner(&mut self, key: crossterm::event::KeyEvent) -> Vec<PanelEvent> {
        let page_size = self.last_area.height.saturating_sub(4) as usize;

        // Escape closes any open dropdown
        if key.code == KeyCode::Esc && (self.repo_dropdown_open || self.branch_dropdown_open) {
            self.repo_dropdown_open = false;
            self.branch_dropdown_open = false;
            self.dropdown_cursor = 0;
            return vec![];
        }

        // Configurable actions via HotkeyTable (only for Commits section, not dropdowns)
        if !self.repo_dropdown_open && !self.branch_dropdown_open {
            if self.hotkeys.matches("open_external", &key) {
                return self.open_commit_external();
            }
            if self.hotkeys.matches("checkout", &key) {
                if let Some(commit) = self.selected_commit() {
                    if !commit.hash.is_empty() {
                        let t = termide_i18n::t();
                        self.status_message = Some(format!(
                            "Checkout {} {}",
                            commit.hash,
                            t.git_checkout_not_impl()
                        ));
                    }
                }
                return vec![];
            }
        }

        // Vim-aware navigation (j/k/g/G when vim_mode is enabled)
        if is_move_up(&key, self.vim_mode) {
            if self.repo_dropdown_open || self.branch_dropdown_open {
                self.dropdown_cursor = self.dropdown_cursor.saturating_sub(1);
                return vec![];
            }
            match self.current_section {
                Section::RepoSelector => {
                    if self.repo_manager.selected_index() > 0 {
                        self.repo_manager.select_prev();
                        self.selected = 0;
                        self.refresh();
                    }
                }
                Section::BranchSelector => {}
                Section::Commits => self.move_up(),
            }
            return vec![];
        }
        if is_move_down(&key, self.vim_mode) {
            if self.repo_dropdown_open {
                let max = self.repo_manager.len().saturating_sub(1);
                if self.dropdown_cursor < max {
                    self.dropdown_cursor += 1;
                }
                return vec![];
            }
            if self.branch_dropdown_open {
                let max = self.branches.len().saturating_sub(1);
                if self.dropdown_cursor < max {
                    self.dropdown_cursor += 1;
                }
                return vec![];
            }
            match self.current_section {
                Section::RepoSelector => {
                    if self.repo_manager.selected_index() + 1 < self.repo_manager.len() {
                        self.repo_manager.select_next();
                        self.selected = 0;
                        self.refresh();
                    }
                }
                Section::BranchSelector => {}
                Section::Commits => self.move_down(),
            }
            return vec![];
        }
        if is_go_home(&key, self.vim_mode) {
            if !self.repo_dropdown_open && !self.branch_dropdown_open {
                self.go_to_start();
            }
            return vec![];
        }
        if is_go_end(&key, self.vim_mode) {
            if !self.repo_dropdown_open && !self.branch_dropdown_open {
                self.go_to_end();
            }
            return vec![];
        }

        match key.code {
            // Tab switches sections
            KeyCode::Tab => {
                self.repo_dropdown_open = false;
                self.branch_dropdown_open = false;
                self.next_section();
            }
            KeyCode::BackTab => {
                self.repo_dropdown_open = false;
                self.branch_dropdown_open = false;
                self.prev_section();
            }
            KeyCode::PageUp => {
                self.page_up(page_size);
            }
            KeyCode::PageDown => {
                self.page_down(page_size);
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                return self.open_commit_external();
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.current_section {
                    Section::RepoSelector => {
                        if self.repo_dropdown_open {
                            // Confirm repo selection
                            let idx = self.dropdown_cursor;
                            self.repo_manager.select(idx);
                            self.repo_dropdown_open = false;
                            self.selected_branch = None;
                            self.refresh();
                        } else {
                            self.dropdown_cursor = self.repo_manager.selected_index();
                            self.repo_dropdown_open = true;
                        }
                    }
                    Section::BranchSelector => {
                        if self.branch_dropdown_open {
                            // Confirm branch selection
                            let selected = self.branches.get(self.dropdown_cursor).cloned();
                            let is_current = selected.as_deref() == self.branch.as_deref();
                            self.selected_branch = if is_current { None } else { selected };
                            self.branch_dropdown_open = false;
                            self.refresh();
                        } else {
                            self.dropdown_cursor = self.shown_branch_index();
                            self.branch_dropdown_open = true;
                        }
                    }
                    Section::Commits => {
                        if key.code == KeyCode::Enter {
                            return self.view_diff();
                        } else {
                            // Space on commits shows commit info
                            self.show_commit_info();
                        }
                    }
                }
            }
            KeyCode::Char('d') => {
                return self.view_diff();
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // The reloaded log is self-evident; no lingering status note.
                self.refresh();
            }
            _ => {}
        }

        vec![]
    }

    fn handle_mouse_inner(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let col = event.column;
                let row = event.row;

                // Dropdown overlay click handling (takes priority)
                if let Some(area) = self.dropdown_area {
                    if row >= area.y
                        && row < area.y + area.height
                        && col >= area.x
                        && col < area.x + area.width
                    {
                        // Click inside dropdown: select item (skip border rows)
                        if row > area.y && row < area.y + area.height - 1 {
                            let visible_rows = (area.height as usize).saturating_sub(2);
                            if self.repo_dropdown_open {
                                // The same rows the dropdown drew, scrolled the same
                                // way: the line clicked is the repository it names,
                                // and a directory row selects nothing.
                                let rows = git::repo_tree(
                                    self.repo_manager.repos(),
                                    self.repo_manager.project_root(),
                                );
                                let cursor_row = rows
                                    .iter()
                                    .position(|r| r.repo == Some(self.dropdown_cursor))
                                    .unwrap_or(0);
                                let scroll_offset = if cursor_row >= visible_rows {
                                    cursor_row - visible_rows + 1
                                } else {
                                    0
                                };
                                let clicked_row = scroll_offset + (row - area.y - 1) as usize;
                                // A heading, or a row past the list, selects nothing.
                                if let Some(repo_idx) = rows.get(clicked_row).and_then(|r| r.repo) {
                                    self.repo_manager.select(repo_idx);
                                    self.repo_dropdown_open = false;
                                    self.selected_branch = None;
                                    self.refresh();
                                }
                            } else if self.branch_dropdown_open {
                                // Branches are a flat list: the row is the branch.
                                let scroll_offset = if self.dropdown_cursor >= visible_rows {
                                    self.dropdown_cursor - visible_rows + 1
                                } else {
                                    0
                                };
                                let item_idx = scroll_offset + (row - area.y - 1) as usize;
                                let selected = self.branches.get(item_idx).cloned();
                                let is_current = selected.as_deref() == self.branch.as_deref();
                                self.selected_branch = if is_current { None } else { selected };
                                self.branch_dropdown_open = false;
                                self.refresh();
                            }
                        }
                        return vec![];
                    }
                    // Click outside dropdown: close it, but check if click landed on a
                    // selector — if so, open that selector (or keep closed if it was
                    // already the open one, to preserve toggle-off behavior).
                    let was_repo_open = self.repo_dropdown_open;
                    let was_branch_open = self.branch_dropdown_open;
                    self.repo_dropdown_open = false;
                    self.branch_dropdown_open = false;
                    self.dropdown_area = None;

                    if let Some(area) = self.repo_selector_area {
                        if row == area.y && col >= area.x && col < area.x + area.width {
                            self.current_section = Section::RepoSelector;
                            if !was_repo_open {
                                self.dropdown_cursor = self.repo_manager.selected_index();
                                self.repo_dropdown_open = true;
                            }
                            return vec![];
                        }
                    }
                    if let Some(area) = self.branch_selector_area {
                        if row == area.y && col >= area.x && col < area.x + area.width {
                            self.current_section = Section::BranchSelector;
                            if !was_branch_open {
                                self.dropdown_cursor = self.shown_branch_index();
                                self.branch_dropdown_open = true;
                            }
                            return vec![];
                        }
                    }
                    return vec![];
                }

                // Repo selector click
                if let Some(area) = self.repo_selector_area {
                    if row == area.y && col >= area.x && col < area.x + area.width {
                        self.current_section = Section::RepoSelector;
                        if self.repo_dropdown_open {
                            self.repo_dropdown_open = false;
                        } else {
                            self.dropdown_cursor = self.repo_manager.selected_index();
                            self.repo_dropdown_open = true;
                        }
                        return vec![];
                    }
                }

                // Branch selector click
                if let Some(area) = self.branch_selector_area {
                    if row == area.y && col >= area.x && col < area.x + area.width {
                        self.current_section = Section::BranchSelector;
                        if self.branch_dropdown_open {
                            self.branch_dropdown_open = false;
                        } else {
                            self.dropdown_cursor = self.shown_branch_index();
                            self.branch_dropdown_open = true;
                        }
                        return vec![];
                    }
                }

                // Commit list click (content fills the inner area; header takes
                // the first y_offset=2 rows).
                let commits_start_y = self.last_area.y + 2;
                if row >= commits_start_y {
                    let clicked_idx = self.scroll + (row - commits_start_y) as usize;
                    if clicked_idx < self.commits.len() {
                        self.current_section = Section::Commits;
                        self.selected = clicked_idx;
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if self.repo_dropdown_open || self.branch_dropdown_open {
                    self.dropdown_cursor = self.dropdown_cursor.saturating_sub(1);
                } else {
                    self.wheel(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.repo_dropdown_open || self.branch_dropdown_open {
                    let max = self.open_dropdown_len().saturating_sub(1);
                    if self.dropdown_cursor < max {
                        self.dropdown_cursor += 1;
                    }
                } else {
                    self.wheel(3);
                }
            }
            _ => {}
        }
        vec![]
    }

    fn handle_scroll_inner(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        let lines = delta.unsigned_abs() as usize;
        // While a selector dropdown is open, the wheel scrolls it, not commits.
        if self.repo_dropdown_open || self.branch_dropdown_open {
            if delta < 0 {
                self.dropdown_cursor = self.dropdown_cursor.saturating_sub(lines);
            } else {
                let max = self.open_dropdown_len().saturating_sub(1);
                self.dropdown_cursor = (self.dropdown_cursor + lines).min(max);
            }
            return vec![];
        }
        self.wheel(delta as isize * 3); // 3 lines per scroll unit
        vec![]
    }
}
