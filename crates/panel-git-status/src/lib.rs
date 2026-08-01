//! Git Status Panel for termide.
//!
//! Provides a panel for managing git operations: staging, unstaging, commits, push/pull.

#![allow(clippy::too_many_arguments)]

mod actions;
mod filter;
mod keyboard;
mod mouse;
mod refresh;
mod rendering;
mod selection;
pub mod tree;
mod types;

use refresh::GitStatusRefreshResult;
use types::FileTree;
pub use types::{Button, Section, Selection};

use std::any::Any;
use std::path::{Path, PathBuf};

use crossterm::event::MouseEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
};

use termide_config::Config;
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel,
    ThemeColors, WidthPreference,
};
use termide_git::{self as git, RepoManager, StagedFile, UnstagedFile};
use termide_modal::ActiveModal;
use termide_state::PendingAction;
use termide_theme::Theme;
use termide_ui::IndexClickTracker;

/// Git Status Panel
pub struct GitStatusPanel {
    /// Repository manager
    repo_manager: RepoManager,
    /// Current branch name
    branch: Option<String>,
    /// Available branches for current repo
    branches: Vec<String>,
    /// Ahead/behind counts
    ahead: usize,
    behind: usize,
    /// Unstaged files (modified + untracked)
    unstaged_files: Vec<UnstagedFile>,
    /// Staged files
    staged_files: Vec<StagedFile>,
    /// Current section
    current_section: Section,
    /// Cursor position as virtual line (0..total_virtual_lines, includes headers)
    cursor: usize,
    /// Selected button index
    selected_button: usize,
    /// Unified scroll offset for files area
    scroll_offset: usize,
    /// Cached viewport height for scroll calculations
    viewport_height: usize,
    /// Cached theme colors for rendering
    cached_theme: ThemeColors,
    /// Last render area (for mouse handling)
    last_area: Rect,
    /// Status message
    status_message: Option<String>,
    /// Is repo dropdown expanded
    repo_dropdown_open: bool,
    /// Is branch dropdown expanded
    branch_dropdown_open: bool,
    /// Cursor position in open dropdown
    dropdown_cursor: usize,
    // Layout zones for mouse handling
    /// Selector row Y position
    selector_y: u16,
    /// Branch selector X position (for mouse click detection)
    branch_selector_x: u16,
    /// Files area (combined unstaged + staged)
    files_area: Rect,
    /// Buttons row Y position
    buttons_y: u16,
    /// Cached height of the buttons area (may span multiple rows)
    cached_buttons_height: u16,
    /// Repo dropdown area (for mouse click detection)
    repo_dropdown_area: Option<Rect>,
    /// Branch dropdown area (for mouse click detection)
    branch_dropdown_area: Option<Rect>,
    /// Scroll offset in dropdown
    dropdown_scroll: usize,
    /// Branch filter text (hidden filter row in dropdown)
    branch_filter: String,
    /// Whether the branch filter row is visible (shown after first keystroke)
    show_branch_filter: bool,
    /// Repo filter text (hidden filter row in dropdown)
    repo_filter: String,
    /// Whether the repo filter row is visible (shown after first keystroke)
    show_repo_filter: bool,
    /// Stash button area (for dropdown anchoring)
    stash_button_area: Option<Rect>,
    /// Click tracker for double-click detection in files area
    click_tracker: IndexClickTracker,
    /// Modal request (for file properties)
    modal_request: Option<(termide_state::PendingAction, termide_modal::ActiveModal)>,
    /// Loading indicator flag
    is_loading: bool,
    /// Whether git operation (push/pull) is in progress
    git_operation_in_progress: bool,
    /// Current git operation name ("push" or "pull")
    current_operation: Option<String>,
    /// Spinner animation frame for Pushing/Pulling buttons
    spinner_frame: usize,
    /// Initial paths passed to the panel (for git init when no repo found)
    initial_paths: Vec<PathBuf>,
    /// Cached vim_mode setting for keyboard handling
    vim_mode: bool,
    /// Whether panel missed updates while collapsed (stale-on-collapse)
    is_stale: bool,
    /// Watched root registered with the watcher (None = not yet registered)
    watched_root: Option<PathBuf>,
    /// Tree state for unstaged files (modified + untracked)
    unstaged: FileTree,
    /// Tree state for staged files
    staged: FileTree,
    /// Pending initial fetch to update ahead/behind counts
    pending_init_fetch: bool,
    /// Cached stash count (for button label)
    stash_count: usize,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,
    /// In-flight async refresh — when set, the panel is rendering its
    /// `is_loading` placeholder while a background thread runs the
    /// `git status` / `git branch` / `git rev-list` commands. `tick()`
    /// polls the receiver and folds the result in once it lands.
    refresh_rx: Option<std::sync::mpsc::Receiver<GitStatusRefreshResult>>,
    /// Set when `refresh()` is called while a worker is already in flight, so a
    /// single follow-up pass runs once it lands instead of stacking threads
    /// (and git subprocesses) per watcher event during a `.git` storm.
    refresh_pending: bool,
}

/// Build HotkeyTable for the git status panel.
fn build_git_status_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.git_status.keybindings;

    t.insert("stage", &kb.stage);
    t.insert("unstage", &kb.unstage);
    t.insert("view", &kb.view);
    t.insert("edit", &kb.edit);
    t.insert("info", &kb.info);
    t.insert("revert", &kb.revert);
    t.insert("refresh", &kb.refresh);
    t
}

impl GitStatusPanel {
    /// Create a new Git Status panel from a list of paths (from panels/session)
    pub fn new(paths: &[PathBuf]) -> Self {
        Self::create(RepoManager::new(paths), paths.to_vec())
    }

    /// Create panel for a specific repository
    pub fn new_for_repo(repo_path: PathBuf) -> Self {
        let initial_paths = vec![repo_path.clone()];
        Self::create(RepoManager::for_repo(repo_path), initial_paths)
    }

    fn create(repo_manager: RepoManager, initial_paths: Vec<PathBuf>) -> Self {
        let mut panel = Self {
            repo_manager,
            branch: None,
            branches: Vec::new(),
            ahead: 0,
            behind: 0,
            unstaged_files: Vec::new(),
            staged_files: Vec::new(),
            current_section: Section::RepoSelector,
            cursor: 0,
            selected_button: 0,
            scroll_offset: 0,
            viewport_height: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            status_message: None,
            repo_dropdown_open: false,
            branch_dropdown_open: false,
            dropdown_cursor: 0,
            selector_y: 0,
            branch_selector_x: 0,
            files_area: Rect::default(),
            buttons_y: 0,
            cached_buttons_height: 1,
            repo_dropdown_area: None,
            branch_dropdown_area: None,
            dropdown_scroll: 0,
            branch_filter: String::new(),
            show_branch_filter: false,
            repo_filter: String::new(),
            show_repo_filter: false,
            stash_button_area: None,
            click_tracker: IndexClickTracker::new(),
            modal_request: None,
            is_loading: false,
            git_operation_in_progress: false,
            current_operation: None,
            spinner_frame: 0,
            initial_paths,
            vim_mode: false,
            is_stale: false,
            watched_root: None,
            unstaged: FileTree::new(),
            staged: FileTree::new(),
            pending_init_fetch: true,
            stash_count: 0,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            refresh_rx: None,
            refresh_pending: false,
        };

        panel.refresh();
        panel
    }

    /// Update repository list based on new paths from panels
    pub fn update_repos(&mut self, paths: &[PathBuf]) {
        if self.repo_manager.update(paths) {
            // Reset watched_root so the app watcher re-registers the new repo
            self.watched_root = None;
            self.refresh();
        }
    }

    /// Take modal request for app to handle
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    /// Get disk space information for the current repository.
    pub fn get_disk_space_info(&self) -> Option<termide_system_monitor::DiskSpaceInfo> {
        self.repo_manager
            .current()
            .and_then(termide_system_monitor::get_disk_space_info)
    }
}

impl Panel for GitStatusPanel {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferNarrow
    }

    fn title(&self) -> String {
        use termide_config::constants::spinner_frame;

        let t = termide_i18n::t();
        let repo_name = self
            .repo_manager
            .current()
            .map(git::get_repo_name)
            .unwrap_or_else(|| t.git_no_repo().to_string());
        let detached = t.git_branch_detached().to_string();
        let branch = self.branch.as_deref().unwrap_or(&detached);

        let uncommitted = self.unstaged_files.len() + self.staged_files.len();
        let status = format!("*{} ↑{} ↓{}", uncommitted, self.ahead, self.behind);

        if self.is_loading {
            let spinner = spinner_frame();
            format!(
                "{} {} ({}) {} ({})",
                spinner,
                repo_name,
                branch,
                status,
                t.git_status_loading()
            )
        } else {
            format!("{} ({}) {}", repo_name, branch, status)
        }
    }

    fn colorize_title(&self, truncated: &str, base_style: Style) -> Line<'static> {
        let markers: &[(&str, ratatui::style::Color)] = &[
            ("*", self.cached_theme.error),
            ("\u{2191}", self.cached_theme.success), // ↑
            ("\u{2193}", self.cached_theme.warning), // ↓
        ];

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut rest = truncated;

        while !rest.is_empty() {
            // Find the earliest marker
            let mut earliest: Option<(usize, &str, ratatui::style::Color)> = None;
            for &(marker, color) in markers {
                if let Some(pos) = rest.find(marker) {
                    if earliest.is_none_or(|(e_pos, _, _)| pos < e_pos) {
                        earliest = Some((pos, marker, color));
                    }
                }
            }

            match earliest {
                Some((pos, marker, color)) => {
                    // Text before marker
                    if pos > 0 {
                        spans.push(Span::styled(rest[..pos].to_string(), base_style));
                    }
                    // Marker + following digits
                    let after_marker = &rest[pos + marker.len()..];
                    let digit_count = after_marker
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .count();
                    let end = pos + marker.len() + digit_count;
                    let value: usize = after_marker[..digit_count].parse().unwrap_or(0);
                    let marker_style = if value > 0 {
                        base_style.fg(color)
                    } else {
                        base_style
                    };
                    spans.push(Span::styled(rest[pos..end].to_string(), marker_style));
                    rest = &rest[end..];
                }
                None => {
                    spans.push(Span::styled(rest.to_string(), base_style));
                    break;
                }
            }
        }

        Line::from(spans)
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        self.cached_theme = ThemeColors::from(theme);
        self.vim_mode = config.general.vim_mode;
        let config_ptr = std::sync::Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            self.hotkeys = build_git_status_hotkey_table(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_area = area;

        // Render content (border is handled by ui-render)
        self.render_content(area, buf, ctx.is_focused, ctx.border_right_x);
    }

    fn captures_escape(&self) -> bool {
        // Capture Escape when dropdown is open (don't close panel)
        self.repo_dropdown_open || self.branch_dropdown_open
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::OnGitUpdate { repo_paths } => {
                // Check if current repo is in the updated list
                if let Some(current_repo) = self.repo_manager.current() {
                    let should_refresh = git::repo_paths_overlap(current_repo, repo_paths);
                    if should_refresh {
                        self.refresh();
                        return CommandResult::NeedsRedraw(true);
                    }
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::OnFsUpdate { changed_path } => {
                // Refresh on file changes within current repo
                if let Some(current_repo) = self.repo_manager.current() {
                    if changed_path.starts_with(current_repo) {
                        self.refresh();
                        return CommandResult::NeedsRedraw(true);
                    }
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::SetGitOperationInProgress {
                in_progress,
                operation,
                spinner_frame,
            } => {
                let changed = self.git_operation_in_progress != in_progress
                    || self.current_operation != operation
                    || self.spinner_frame != spinner_frame;
                if changed {
                    self.git_operation_in_progress = in_progress;
                    self.current_operation = operation;
                    self.spinner_frame = spinner_frame;
                    // Adjust selected button if Push/Pull disappeared
                    let buttons = self.get_visible_buttons();
                    if self.selected_button >= buttons.len() {
                        self.selected_button = buttons.len().saturating_sub(1);
                    }
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::MarkStale => {
                self.is_stale = true;
                self.refresh_title_data();
                CommandResult::NeedsRedraw(true)
            }
            PanelCommand::Reload => {
                self.refresh();
                CommandResult::NeedsRedraw(true)
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
            PanelCommand::GetFsWatchInfo => {
                // Return watch info so the app watcher registers the repo root.
                // current_path is the repo root (used by app to call find_repo_root).
                let current_path = self
                    .repo_manager
                    .current()
                    .map(|p| p.to_path_buf())
                    .or_else(|| self.initial_paths.first().cloned())
                    .unwrap_or_default();
                CommandResult::FsWatchInfo {
                    watched_root: self.watched_root.clone(),
                    current_path,
                    is_git_repo: self.repo_manager.current().is_some(),
                }
            }
            PanelCommand::SetFsWatchRoot { root, .. } => {
                self.watched_root = root;
                CommandResult::None
            }
            _ => CommandResult::None,
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        self.on_key(chord)
    }

    fn handle_mouse(&mut self, event: MouseEvent, panel_area: Rect) -> Vec<PanelEvent> {
        self.on_mouse(event, panel_area)
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        // An open selector dropdown takes the wheel first.
        if self.repo_dropdown_open || self.branch_dropdown_open {
            let lines = delta.unsigned_abs() as usize;
            if delta < 0 {
                self.dropdown_cursor = self.dropdown_cursor.saturating_sub(lines);
            } else {
                let max = self.open_dropdown_len().saturating_sub(1);
                self.dropdown_cursor = (self.dropdown_cursor + lines).min(max);
            }
            return vec![];
        }
        let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
        if delta < 0 {
            // Scroll up
            self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        } else {
            // Scroll down
            let total_lines = self.total_virtual_lines();
            let max_scroll = total_lines.saturating_sub(self.viewport_height);
            self.scroll_offset = (self.scroll_offset + lines).min(max_scroll);
        }
        vec![]
    }

    fn to_session(&self, _session_dir: &Path) -> Option<SessionPanel> {
        self.repo_manager
            .current()
            .map(|repo| SessionPanel::GitStatus {
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

    fn tick(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();

        // Repository discovery (submodules, and nested repos under a non-repo
        // root) runs in the background; pull its result in here so the repo
        // dropdown reflects the full list once available without ever blocking
        // the constructor.
        let before = self.repo_manager.current().map(|p| p.to_path_buf());
        if self.repo_manager.poll() {
            events.push(PanelEvent::NeedsRedraw);
            let after = self.repo_manager.current().map(|p| p.to_path_buf());
            if after.is_none() {
                // The repo(s) vanished (e.g. `.git` deleted) — drop the now
                // stale branch/file lists instead of leaving them on screen.
                if before.is_some() {
                    self.clear_git_state();
                }
            } else if before != after {
                // A repo was just discovered (async nested scan) or the current
                // one was removed and selection moved — load its status, since
                // `poll()` only fills the list.
                self.refresh();
            }
        }

        // Async refresh worker — when ready, swap branch/files/etc.
        // into place. Until then the panel keeps showing the
        // `is_loading` placeholder. `refresh()` is fire-and-forget.
        if self.poll_refresh() {
            events.push(PanelEvent::NeedsRedraw);
        }

        // Trigger initial fetch once when panel is ready and has a repo
        if self.pending_init_fetch && !self.repo_manager.is_empty() {
            self.pending_init_fetch = false;
            if let Some(repo) = self.repo_manager.current() {
                use termide_core::event::{GitOperationType, PanelEvent};
                events.push(PanelEvent::GitOperation {
                    operation: GitOperationType::Fetch,
                    repo_path: repo.to_path_buf(),
                });
            }
        }
        events
    }
}
