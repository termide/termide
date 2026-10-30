//! Git Diff Panel for termide.
//!
//! Provides a panel for viewing all git diffs with syntax highlighting.

mod load;
mod model;
mod navigation;
mod render;
mod unified;

pub use model::{DiffHunk, DiffLine, FileDiff, FileStatus, LineKind};

use std::any::Any;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};

use termide_config::constants::spinner_frame;
use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up, Config};
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, PanelState, RenderContext,
    ThemeColors, WidthPreference,
};
use termide_git::{self as git};
use termide_theme::Theme;

/// Git Diff Panel
pub struct GitDiffPanel {
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
    /// A repo update arrived while the panel was collapsed to its title bar;
    /// the diff reloads once the panel shows content again.
    is_stale: bool,
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
    /// Cached editor tab size: diff lines are drawn with their tabs expanded.
    tab_size: usize,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
    /// Pointer of the last Arc<Config> used to build hotkeys (skip rebuild when unchanged)
    last_config_ptr: usize,
    /// Whether this panel shows a stash diff (uses `git stash show -p`)
    is_stash: bool,
    /// Stash message (for title display instead of hash)
    stash_message: Option<String>,
    /// Result channel of the in-flight refresh worker, drained by `tick()`.
    /// `Some` means a worker is running.
    refresh_rx: Option<std::sync::mpsc::Receiver<load::GitDiffRefreshResult>>,
    /// A refresh was requested while a worker was in flight; run one more pass
    /// when it finishes instead of spawning workers per event.
    refresh_pending: bool,
    /// Whether a refresh is in flight, for the loading marker in `title()`.
    is_loading: bool,
    /// The diff was handed over rather than loaded from git: it never
    /// refreshes, and the panel is not saved with the session.
    given: Option<Given>,
}

/// What a panel showing a handed-over diff keeps of it.
struct Given {
    title: String,
    /// The id the app gave a plugin's panel.
    plugin_panel: Option<u64>,
    /// The plugin's own `[≡]` entries, `(label, action)`.
    menu: Vec<(String, String)>,
}

/// Prefix of the context-menu actions a plugin's panel adds.
const PLUGIN_MENU_PREFIX: &str = "plugin-menu:";

/// Build HotkeyTable for the git diff panel from config.
fn build_git_diff_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.git_diff.keybindings;

    t.insert("toggle_collapse", &kb.toggle_collapse);
    t.insert("edit", &kb.edit);
    t.insert("refresh", &kb.refresh);
    t.insert("scroll_half_up", &kb.scroll_half_up);
    t.insert("scroll_half_down", &kb.scroll_half_down);
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

    /// Build the panel empty and start the first load.
    ///
    /// The four entry points below differ only in which target they point at,
    /// and `refresh()` is asynchronous, so none of them touches git before
    /// returning.
    fn for_target(
        repo_path: PathBuf,
        commit_hash: Option<String>,
        file_filter: Option<String>,
        is_stash: bool,
        stash_message: Option<String>,
    ) -> Self {
        let mut panel = Self::blank(repo_path, commit_hash, file_filter, is_stash, stash_message);
        panel.refresh();
        panel
    }

    /// The panel with nothing loaded yet.
    fn blank(
        repo_path: PathBuf,
        commit_hash: Option<String>,
        file_filter: Option<String>,
        is_stash: bool,
        stash_message: Option<String>,
    ) -> Self {
        Self {
            repo_path,
            commit_hash,
            branch: None,
            file_filter,
            diffs: Vec::new(),
            scrollbars: termide_core::ScrollBars::default(),
            is_stale: false,
            scroll: 0,
            collapsed: HashSet::new(),
            selected_file: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            total_lines: 0,
            visible_height: 0,
            status_message: None,
            vim_mode: false,
            tab_size: termide_config::defaults::TAB_SIZE,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            is_stash,
            stash_message,
            refresh_rx: None,
            refresh_pending: false,
            is_loading: false,
            given: None,
        }
    }

    /// A panel showing `diff`, a unified diff of one or more files, under
    /// `title`. File paths are taken relative to `root` for opening them.
    pub fn from_unified(root: PathBuf, title: String, diff: &str) -> Self {
        let mut panel = Self::blank(root, None, None, false, None);
        panel.given = Some(Given {
            title,
            plugin_panel: None,
            menu: Vec::new(),
        });
        panel.set_unified(None, diff);
        panel
    }

    /// [`Self::from_unified`] for the plugin panel `id`.
    pub fn for_plugin(id: u64, root: PathBuf, title: String, diff: &str) -> Self {
        let mut panel = Self::from_unified(root, title, diff);
        if let Some(given) = panel.given.as_mut() {
            given.plugin_panel = Some(id);
        }
        panel
    }

    /// Whether the panel loads its diff from git, rather than showing one
    /// it was handed.
    pub fn loads_from_git(&self) -> bool {
        self.given.is_none()
    }

    /// The id the app gave a plugin's panel; `None` for the others.
    pub fn plugin_panel_id(&self) -> Option<u64> {
        self.given.as_ref().and_then(|given| given.plugin_panel)
    }

    /// Set a plugin panel's own `[≡]` entries, `(label, action)`.
    pub fn set_plugin_menu(&mut self, menu: Vec<(String, String)>) {
        if let Some(given) = self.given.as_mut() {
            given.menu = menu;
        }
    }

    /// Show `diff` instead, and `title` when given. The selected file and
    /// the collapsed ones stay as they were where their paths are still
    /// there. Does nothing on a panel that loads from git.
    pub fn set_unified(&mut self, title: Option<String>, diff: &str) {
        let Some(given) = self.given.as_mut() else {
            return;
        };
        if let Some(title) = title {
            given.title = title;
        }
        let selected = self.diffs.get(self.selected_file).map(|d| d.path.clone());
        let collapsed: HashSet<String> = self
            .collapsed
            .iter()
            .filter_map(|&i| self.diffs.get(i).map(|d| d.path.clone()))
            .collect();
        self.diffs = unified::parse_unified(diff);
        self.collapsed = (0..self.diffs.len())
            .filter(|&i| collapsed.contains(&self.diffs[i].path))
            .collect();
        self.selected_file = selected
            .and_then(|path| self.diffs.iter().position(|d| d.path == path))
            .unwrap_or_else(|| self.selected_file.min(self.diffs.len().saturating_sub(1)));
        self.calculate_total_lines();
        self.scroll = self.scroll.min(self.total_lines.saturating_sub(1));
    }

    /// The plugin is gone: its menu goes, the diff stays, and the title
    /// ends with `message`.
    pub fn set_stopped(&mut self, message: String) {
        if let Some(given) = self.given.as_mut() {
            given.menu.clear();
            given.title = format!("{} — {message}", given.title);
        }
    }

    /// Create a new Git Diff panel for working directory changes
    pub fn new(repo_path: PathBuf) -> Self {
        Self::for_target(repo_path, None, None, false, None)
    }

    /// Create a new Git Diff panel for a specific commit
    pub fn new_for_commit(repo_path: PathBuf, commit_hash: String) -> Self {
        Self::for_target(repo_path, Some(commit_hash), None, false, None)
    }

    /// Create a new Git Diff panel for a stash entry.
    ///
    /// Uses `git stash show -p` instead of `git show` to get proper diff output.
    pub fn new_for_stash(repo_path: PathBuf, stash_ref: String, message: String) -> Self {
        Self::for_target(repo_path, Some(stash_ref), None, true, Some(message))
    }

    /// Create a new Git Diff panel filtered to a single file
    pub fn new_with_file_filter(repo_path: PathBuf, file_path: PathBuf) -> Self {
        let file_filter = file_path.to_string_lossy().to_string();
        Self::for_target(repo_path, None, Some(file_filter), false, None)
    }
}

impl Panel for GitDiffPanel {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        if self.given.is_some()
            && matches!(
                cmd,
                PanelCommand::Reload
                    | PanelCommand::MarkStale
                    | PanelCommand::RefreshIfStale
                    | PanelCommand::OnGitUpdate { .. }
                    | PanelCommand::OnFsUpdate { .. }
            )
        {
            return CommandResult::None;
        }
        match cmd {
            PanelCommand::Reload => {
                self.is_stale = false;
                self.refresh();
                CommandResult::NeedsRedraw(true)
            }
            // Stale-on-collapse: while the panel is a bare title bar the app
            // sends MarkStale instead of the live updates below; the reload
            // happens once the panel shows content again. Asked every tick,
            // so it must be a no-op when nothing is stale.
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
            // Global clipboard: copy the selected file path (previously the
            // per-panel `clipboard_copy` keybinding).
            PanelCommand::Copy => {
                if let Some(diff) = self.diffs.get(self.selected_file) {
                    let path = diff.path.clone();
                    let _ = termide_clipboard::copy(&path);
                    self.status_message = Some(format!("Copied: {}", path));
                }
                CommandResult::Handled(true)
            }
            PanelCommand::Cut => CommandResult::Handled(false),
            PanelCommand::Paste => CommandResult::Handled(false),
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.scroll = offset;
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn icon(&self) -> Option<&str> {
        self.plugin_panel_id().map(|_| "🧩")
    }

    fn title(&self) -> String {
        if let Some(given) = &self.given {
            return given.title.clone();
        }
        let t = termide_i18n::t();
        let repo_name = git::get_repo_name(&self.repo_path);
        // The branch name arrives with the first refresh result, so until then
        // there is nothing to show — and "detached" would be a lie.
        let branch = self.branch.as_deref().unwrap_or("…");

        // Build files string: "N files" or single filename
        let files = if self.diffs.len() == 1 {
            self.diffs[0].path.clone()
        } else {
            format!("{} files", self.diffs.len())
        };

        let title = if let Some(ref msg) = self.stash_message {
            // Stash diff: show message instead of hash
            format!("Diff: {} — {}", msg, files)
        } else if let Some(ref hash) = self.commit_hash {
            // Show short hash (first 7 characters)
            let short_hash = if hash.len() > 7 { &hash[..7] } else { hash };
            t.git_diff_title_commit_fmt(&repo_name, branch, short_hash, &files)
        } else {
            t.git_diff_title_fmt(&repo_name, branch, &files)
        };

        if self.is_loading {
            format!("{} {} ({})", spinner_frame(), title, t.git_status_loading())
        } else {
            title
        }
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        // Fold in the async refresh once the worker is done. Until then the
        // panel shows the previous diff with a loading marker in the title.
        if self.poll_refresh() {
            return vec![PanelEvent::NeedsRedraw];
        }
        // Keep the title's spinner animating while a refresh is in flight.
        if self.is_loading {
            return vec![PanelEvent::NeedsRedraw];
        }
        vec![]
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        self.cached_theme = ThemeColors::from(theme);
        self.vim_mode = config.general.vim_mode;
        self.tab_size = config.editor.tab_size;
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
        if self.hotkeys.matches("refresh", &key) && self.given.is_none() {
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

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        if self.given.is_some() {
            return None;
        }
        Some(PanelState::GitDiff {
            repo_path: self.repo_path.clone(),
            commit_hash: self.commit_hash.clone(),
        })
    }

    fn context_menu_items(&self) -> Vec<(String, String)> {
        self.given.as_ref().map_or_else(Vec::new, |given| {
            given
                .menu
                .iter()
                .map(|(label, action)| (label.clone(), format!("{PLUGIN_MENU_PREFIX}{action}")))
                .collect()
        })
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match (
            self.plugin_panel_id(),
            action.strip_prefix(PLUGIN_MENU_PREFIX),
        ) {
            (Some(panel), Some(action)) => vec![PanelEvent::PluginAction {
                panel,
                action: action.to_string(),
            }],
            _ => vec![],
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    use tempfile::TempDir;

    /// A directory that is deliberately *not* a git repository: every command
    /// the worker runs fails, so it returns an empty diff quickly and without
    /// depending on a git fixture. These tests cover the refresh state
    /// machine, which is what moving the load off the UI thread introduced.
    fn not_a_repo() -> TempDir {
        TempDir::new().expect("temp dir")
    }

    /// Drain the worker, polling until it delivers. Returns once the result
    /// has been applied.
    fn wait_for_result(panel: &mut GitDiffPanel) {
        for _ in 0..500 {
            if panel.poll_refresh() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the refresh worker never delivered a result");
    }

    /// Hand a result straight to the panel, bypassing the worker, so the
    /// apply step can be checked exactly.
    fn deliver(panel: &mut GitDiffPanel, branch: Option<&str>, paths: &[&str]) {
        let (tx, rx) = mpsc::channel();
        tx.send(load::GitDiffRefreshResult {
            branch: branch.map(str::to_string),
            diffs: paths
                .iter()
                .map(|path| FileDiff {
                    path: (*path).to_string(),
                    status: FileStatus::Modified,
                    staged: false,
                    additions: 0,
                    deletions: 0,
                    hunks: Vec::new(),
                })
                .collect(),
        })
        .expect("send");
        panel.refresh_rx = Some(rx);
        assert!(panel.poll_refresh(), "a queued result must be applied");
    }

    #[test]
    fn construction_does_not_load_synchronously() {
        let dir = not_a_repo();
        let panel = GitDiffPanel::new(dir.path().to_path_buf());

        // The constructor used to run `git status` twice plus a `git diff` per
        // changed file before returning. Now it hands back a loading panel and
        // the diff arrives through `poll_refresh`.
        assert!(panel.is_loading);
        assert!(panel.refresh_rx.is_some());
        assert!(panel.diffs.is_empty());
    }

    #[test]
    fn polling_a_finished_worker_clears_the_loading_state() {
        let dir = not_a_repo();
        let mut panel = GitDiffPanel::new(dir.path().to_path_buf());

        wait_for_result(&mut panel);

        assert!(!panel.is_loading);
        assert!(panel.refresh_rx.is_none());
        // Nothing further is queued, so a later poll has no work.
        assert!(!panel.poll_refresh());
    }

    #[test]
    fn requests_during_one_worker_collapse_into_a_single_follow_up() {
        let dir = not_a_repo();
        let mut panel = GitDiffPanel::new(dir.path().to_path_buf());
        assert!(panel.refresh_rx.is_some(), "the first worker is in flight");

        // The watcher refreshes on every filesystem and git event, so bursts
        // are the normal case rather than the exception.
        panel.refresh();
        panel.refresh();
        panel.refresh();
        assert!(panel.refresh_pending);

        // Draining the first result runs exactly one coalesced pass...
        wait_for_result(&mut panel);
        assert!(!panel.refresh_pending);
        assert!(
            panel.refresh_rx.is_some(),
            "the coalesced follow-up should be in flight"
        );

        // ...and that pass queues nothing more.
        wait_for_result(&mut panel);
        assert!(panel.refresh_rx.is_none());
        assert!(!panel.refresh_pending);
        assert!(!panel.is_loading);
    }

    #[test]
    fn a_file_filter_keeps_only_its_own_diff() {
        let dir = not_a_repo();
        let mut panel =
            GitDiffPanel::new_with_file_filter(dir.path().to_path_buf(), PathBuf::from("src/b.rs"));
        wait_for_result(&mut panel);

        deliver(
            &mut panel,
            Some("main"),
            &["src/a.rs", "src/b.rs", "src/c.rs"],
        );

        assert_eq!(panel.diffs.len(), 1);
        assert_eq!(panel.diffs[0].path, "src/b.rs");
        assert_eq!(panel.branch.as_deref(), Some("main"));
        // One file header, no hunks.
        assert_eq!(panel.total_lines, 1);
    }

    #[test]
    fn selection_is_clamped_into_a_shorter_diff_list() {
        let dir = not_a_repo();
        let mut panel = GitDiffPanel::new(dir.path().to_path_buf());
        wait_for_result(&mut panel);

        deliver(&mut panel, None, &["a", "b", "c"]);
        panel.selected_file = 2;

        deliver(&mut panel, None, &["a"]);

        assert_eq!(panel.diffs.len(), 1);
        assert_eq!(panel.selected_file, 0);
    }

    #[test]
    fn a_worker_that_dies_without_sending_does_not_wedge_the_panel() {
        let dir = not_a_repo();
        let mut panel = GitDiffPanel::new(dir.path().to_path_buf());
        wait_for_result(&mut panel);

        // Drop the sender to simulate a panicking worker thread.
        let (tx, rx) = mpsc::channel::<load::GitDiffRefreshResult>();
        drop(tx);
        panel.refresh_rx = Some(rx);
        panel.is_loading = true;

        assert!(panel.poll_refresh());
        assert!(!panel.is_loading, "a dead worker must not leave it loading");
        assert!(panel.refresh_rx.is_none());
    }

    const TWO_FILES: &str = "\
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-x
+y
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-p
+q
";

    #[test]
    fn a_given_diff_never_touches_git() {
        let dir = not_a_repo();
        let mut panel =
            GitDiffPanel::for_plugin(7, dir.path().to_path_buf(), "diff a.txt".into(), TWO_FILES);

        assert!(panel.refresh_rx.is_none() && !panel.is_loading);
        assert_eq!(panel.diffs.len(), 2);
        assert_eq!(panel.title(), "diff a.txt");
        assert_eq!(panel.icon(), Some("🧩"));
        assert!(!panel.loads_from_git());
        assert!(panel.to_state(dir.path()).is_none());
        let changed = dir.path().join("a.txt");
        let result = panel.handle_command(PanelCommand::OnFsUpdate {
            changed_path: &changed,
        });
        assert!(matches!(result, CommandResult::None));
        assert!(matches!(
            panel.handle_command(PanelCommand::Reload),
            CommandResult::None
        ));
        assert!(panel.refresh_rx.is_none());
    }

    #[test]
    fn a_given_diff_keeps_the_selected_and_collapsed_files_on_update() {
        let dir = not_a_repo();
        let mut panel =
            GitDiffPanel::from_unified(dir.path().to_path_buf(), "diff".into(), TWO_FILES);
        panel.selected_file = 1;
        panel.collapsed.insert(1);

        let reordered = format!("--- a/new.txt\n+++ b/new.txt\n{TWO_FILES}");
        panel.set_unified(Some("diff 3".into()), &reordered);

        assert_eq!(panel.diffs[panel.selected_file].path, "b.txt");
        assert_eq!(panel.collapsed, HashSet::from([2]));
        assert_eq!(panel.title(), "diff 3");
    }

    #[test]
    fn a_plugin_diff_sends_its_menu_actions_until_the_plugin_stops() {
        let dir = not_a_repo();
        let mut panel =
            GitDiffPanel::for_plugin(7, dir.path().to_path_buf(), "diff".into(), TWO_FILES);
        panel.set_plugin_menu(vec![("Refresh".into(), "refresh".into())]);

        let items = panel.context_menu_items();
        assert_eq!(items.len(), 1);
        let events = panel.handle_status_action(&items[0].1);
        assert!(matches!(
            events.as_slice(),
            [PanelEvent::PluginAction { panel: 7, action }] if action == "refresh"
        ));

        panel.set_stopped("The plugin p4 stopped.".into());
        assert!(panel.context_menu_items().is_empty());
        assert_eq!(panel.title(), "diff — The plugin p4 stopped.");
        assert_eq!(panel.diffs.len(), 2);
    }

    /// A tab-indented diff line keeps its indentation: `set_string` would
    /// drop the tab, so the line is drawn with tabs expanded.
    #[test]
    fn a_tab_indented_line_keeps_its_indentation() {
        use ratatui::{buffer::Buffer, layout::Rect};

        let dir = not_a_repo();
        let mut panel = GitDiffPanel::new(dir.path().to_path_buf());
        wait_for_result(&mut panel);
        deliver(&mut panel, Some("main"), &["a.lua"]);
        panel.tab_size = 4;
        panel.diffs[0].hunks = vec![DiffHunk {
            header: "@@ -1 +1 @@".to_string(),
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: "\tname = 1".to_string(),
                old_line: None,
                new_line: Some(1),
            }],
        }];

        let area = Rect::new(0, 0, 60, 8);
        let mut buf = Buffer::empty(area);
        panel.render_content(area, &mut buf, true, None);
        let text: Vec<String> = (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        assert!(
            text.iter().any(|row| row.contains("+    name = 1")),
            "{text:#?}"
        );
    }
}
