//! Git Diff Panel for termide.
//!
//! Provides a panel for viewing all git diffs with syntax highlighting.

mod load;
mod model;
mod navigation;
mod render;

pub use model::{DiffHunk, DiffLine, FileDiff, FileStatus, LineKind};

use std::any::Any;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};

use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up, Config};
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel,
    ThemeColors, WidthPreference,
};
use termide_git::{self as git};
use termide_theme::Theme;

/// Git Diff Panel
pub struct GitDiffPanel {
    /// Repository path
    repo_path: PathBuf,
    /// Commit hash (None = working directory changes, Some = specific commit)
    commit_hash: Option<String>,
    /// Current branch name
    branch: Option<String>,
    /// Optional file path filter (show diff for single file only)
    file_filter: Option<String>,
    /// All file diffs
    diffs: Vec<FileDiff>,
    /// Vertical scroll offset (in lines)
    scroll: usize,
    /// Set of collapsed file indices
    collapsed: HashSet<usize>,
    /// Selected file index
    selected_file: usize,
    /// Cached theme colors
    cached_theme: ThemeColors,
    /// Last render area
    last_area: Rect,
    /// Total number of renderable lines (for scrollbar)
    total_lines: usize,
    /// Visible height
    visible_height: usize,
    /// Status message
    status_message: Option<String>,
    /// Cached vim_mode setting for keyboard handling
    vim_mode: bool,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,
    /// Whether this panel shows a stash diff (uses `git stash show -p`)
    is_stash: bool,
    /// Stash message (for title display instead of hash)
    stash_message: Option<String>,
}

/// Build HotkeyTable for the git diff panel from config.
fn build_git_diff_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.git_diff.keybindings;

    t.insert("toggle_collapse", &kb.toggle_collapse);
    t.insert("edit", &kb.edit);
    t.insert("refresh", &kb.refresh);
    t.insert("scroll_half_up", &kb.scroll_half_up);
    t.insert("scroll_half_down", &kb.scroll_half_down);
    t.insert("clipboard_copy", &kb.clipboard_copy);
    t
}

impl GitDiffPanel {
    /// Repository path.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Commit hash (None = working directory changes).
    pub fn commit_hash(&self) -> Option<&str> {
        self.commit_hash.as_deref()
    }

    /// File path filter (None = all files).
    pub fn file_filter(&self) -> Option<&str> {
        self.file_filter.as_deref()
    }

    /// Create a new Git Diff panel for working directory changes
    pub fn new(repo_path: PathBuf) -> Self {
        let branch = git::get_current_branch(&repo_path);
        let mut panel = Self {
            repo_path,
            commit_hash: None,
            branch,
            file_filter: None,
            diffs: Vec::new(),
            scroll: 0,
            collapsed: HashSet::new(),
            selected_file: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            total_lines: 0,
            visible_height: 0,
            status_message: None,
            vim_mode: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            is_stash: false,
            stash_message: None,
        };
        panel.refresh();
        panel
    }

    /// Create a new Git Diff panel for a specific commit
    pub fn new_for_commit(repo_path: PathBuf, commit_hash: String) -> Self {
        let branch = git::get_current_branch(&repo_path);
        let mut panel = Self {
            repo_path,
            commit_hash: Some(commit_hash),
            branch,
            file_filter: None,
            diffs: Vec::new(),
            scroll: 0,
            collapsed: HashSet::new(),
            selected_file: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            total_lines: 0,
            visible_height: 0,
            status_message: None,
            vim_mode: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            is_stash: false,
            stash_message: None,
        };
        panel.refresh();
        panel
    }

    /// Create a new Git Diff panel for a stash entry.
    ///
    /// Uses `git stash show -p` instead of `git show` to get proper diff output.
    pub fn new_for_stash(repo_path: PathBuf, stash_ref: String, message: String) -> Self {
        let branch = git::get_current_branch(&repo_path);
        let mut panel = Self {
            repo_path,
            commit_hash: Some(stash_ref),
            branch,
            file_filter: None,
            diffs: Vec::new(),
            scroll: 0,
            collapsed: HashSet::new(),
            selected_file: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            total_lines: 0,
            visible_height: 0,
            status_message: None,
            vim_mode: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            is_stash: true,
            stash_message: Some(message),
        };
        panel.refresh();
        panel
    }

    /// Create a new Git Diff panel filtered to a single file
    pub fn new_with_file_filter(repo_path: PathBuf, file_path: PathBuf) -> Self {
        let branch = git::get_current_branch(&repo_path);
        let file_filter = file_path.to_string_lossy().to_string();
        let mut panel = Self {
            repo_path,
            commit_hash: None,
            branch,
            file_filter: Some(file_filter),
            diffs: Vec::new(),
            scroll: 0,
            collapsed: HashSet::new(),
            selected_file: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            total_lines: 0,
            visible_height: 0,
            status_message: None,
            vim_mode: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            is_stash: false,
            stash_message: None,
        };
        panel.refresh();
        panel
    }
}

impl Panel for GitDiffPanel {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            // Reloaded on focus gain (and Ctrl+R) so the diff picks up changes
            // made outside the panel without a manual refresh.
            PanelCommand::Reload | PanelCommand::RefreshIfStale => {
                self.refresh();
                CommandResult::NeedsRedraw(true)
            }
            // Live watcher updates: refresh an open diff as soon as something in
            // its repo changes (working-tree edit -> OnFsUpdate, commit/index ->
            // OnGitUpdate), instead of waiting for the next focus/Ctrl+R.
            PanelCommand::OnGitUpdate { repo_paths } => {
                let hit = git::repo_paths_overlap(&self.repo_path, repo_paths);
                if hit {
                    self.refresh();
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::OnFsUpdate { changed_path } => {
                if changed_path.starts_with(&self.repo_path) {
                    self.refresh();
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::NeedsRedraw(false)
            }
            _ => CommandResult::None,
        }
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        let t = termide_i18n::t();
        let repo_name = git::get_repo_name(&self.repo_path);
        let branch = self.branch.as_deref().unwrap_or("detached");

        // Build files string: "N files" or single filename
        let files = if self.diffs.len() == 1 {
            self.diffs[0].path.clone()
        } else {
            format!("{} files", self.diffs.len())
        };

        if let Some(ref msg) = self.stash_message {
            // Stash diff: show message instead of hash
            format!("Diff: {} — {}", msg, files)
        } else if let Some(ref hash) = self.commit_hash {
            // Show short hash (first 7 characters)
            let short_hash = if hash.len() > 7 { &hash[..7] } else { hash };
            t.git_diff_title_commit_fmt(&repo_name, branch, short_hash, &files)
        } else {
            t.git_diff_title_fmt(&repo_name, branch, &files)
        }
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        self.cached_theme = ThemeColors::from(theme);
        self.vim_mode = config.general.vim_mode;
        let config_ptr = std::sync::Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            self.hotkeys = build_git_diff_hotkey_table(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_area = area;
        self.render_content(area, buf, ctx.is_focused, ctx.border_right_x);
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        self.status_message = None;

        // Configurable actions via HotkeyTable
        if self.hotkeys.matches("toggle_collapse", &key) {
            self.toggle_collapse();
            return vec![];
        }
        if self.hotkeys.matches("edit", &key) {
            return self.open_file();
        }
        if self.hotkeys.matches("refresh", &key) {
            self.refresh();
            return vec![PanelEvent::NeedsRedraw];
        }
        if self.hotkeys.matches("scroll_half_up", &key) {
            self.scroll_up(self.visible_height / 2);
            return vec![];
        }
        if self.hotkeys.matches("scroll_half_down", &key) {
            self.scroll_down(self.visible_height / 2);
            return vec![];
        }
        if self.hotkeys.matches("clipboard_copy", &key) {
            if let Some(diff) = self.diffs.get(self.selected_file) {
                let path = diff.path.clone();
                let _ = termide_clipboard::copy(&path);
                self.status_message = Some(format!("Copied: {}", path));
            }
            return vec![];
        }

        // Vim-aware navigation (j/k/g/G when vim_mode is enabled)
        if is_move_up(&key, self.vim_mode) {
            self.move_up();
            return vec![];
        }
        if is_move_down(&key, self.vim_mode) {
            self.move_down();
            return vec![];
        }
        if is_go_home(&key, self.vim_mode) {
            self.go_to_start();
            return vec![];
        }
        if is_go_end(&key, self.vim_mode) {
            self.go_to_end();
            return vec![];
        }

        match key.code {
            // Collapse / Expand
            KeyCode::Left if key.modifiers.is_empty() => self.collapse_current(),
            KeyCode::Right if key.modifiers.is_empty() => self.expand_current(),
            // Page navigation
            KeyCode::PageUp => self.page_up(),
            KeyCode::PageDown => self.page_down(),
            _ => {}
        }

        vec![]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Find which file was clicked
                let content_y = self.last_area.y + 1;
                if event.row >= content_y {
                    let clicked_visual_line = (event.row - content_y) as usize + self.scroll;

                    // Find which file this line belongs to
                    let mut current_line = 0;
                    for (file_idx, diff) in self.diffs.iter().enumerate() {
                        let file_header_line = current_line;
                        current_line += 1;

                        if clicked_visual_line == file_header_line {
                            self.selected_file = file_idx;
                            self.toggle_collapse();
                            return vec![];
                        }

                        if !self.collapsed.contains(&file_idx) {
                            for hunk in &diff.hunks {
                                current_line += 1 + hunk.lines.len();
                            }
                        }

                        if clicked_visual_line < current_line {
                            self.selected_file = file_idx;
                            return vec![];
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3);
            }
            _ => {}
        }
        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
        if delta < 0 {
            self.scroll_up(lines);
        } else {
            self.scroll_down(lines);
        }
        vec![]
    }

    fn to_session(&self, _session_dir: &Path) -> Option<SessionPanel> {
        Some(SessionPanel::GitDiff {
            repo_path: self.repo_path.clone(),
            commit_hash: self.commit_hash.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        Some(self.repo_path.clone())
    }
}
