//! Git Log Panel for termide.
//!
//! Provides a panel for viewing git commit history with graph visualization.

use std::any::Any;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_width::UnicodeWidthStr;

use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up, Config, KeyBinding};
use termide_core::{
    CommandResult, HotkeyTable, Panel, PanelCommand, PanelEvent, RenderContext, SessionPanel,
    ThemeColors, WidthPreference,
};
use termide_git::{self as git, truncate_right, truncate_to_width, CommitInfo, RepoManager};
use termide_modal::{ActiveModal, InfoModal, ModalValue, SegmentStyle, StyledSegment};
use termide_state::PendingAction;
use termide_theme::Theme;
use termide_ui::ScrollBar;
use termide_ui_render::{render_simple_dropdown, InlineSelector};

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
    /// Current section
    current_section: Section,
    /// Current branch name (HEAD)
    branch: Option<String>,
    /// All branches for the current repo
    branches: Vec<String>,
    /// Selected branch to view log for (None = current HEAD)
    selected_branch: Option<String>,
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
    /// Commit log entries
    commits: Vec<CommitInfo>,
    /// Currently selected commit index
    selected: usize,
    /// Scroll offset
    scroll: usize,
    /// Number of commits to load
    commit_count: usize,
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
}

/// Snapshot returned by the background refresh worker.
struct GitLogRefreshResult {
    branch: Option<String>,
    branches: Vec<String>,
    commits: Vec<CommitInfo>,
}

/// Colour for graph lane `col`, cycling through the theme's accent colours so
/// adjacent lanes stay visually distinct. Lane 0 (the mainline) is stable at
/// the first entry.
fn lane_color(theme: &ThemeColors, col: usize) -> ratatui::style::Color {
    let palette = [
        theme.info,
        theme.success,
        theme.warning,
        theme.error,
        theme.cursor,
    ];
    palette[col % palette.len()]
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
    t.insert("clipboard_copy", &kb.clipboard_copy);
    t
}

impl GitLogPanel {
    fn create(repo_manager: RepoManager) -> Self {
        let mut panel = Self {
            repo_manager,
            current_section: Section::Commits,
            branch: None,
            branches: Vec::new(),
            selected_branch: None,
            repo_dropdown_open: false,
            branch_dropdown_open: false,
            dropdown_cursor: 0,
            dropdown_area: None,
            repo_selector_area: None,
            branch_selector_area: None,
            commits: Vec::new(),
            selected: 0,
            scroll: 0,
            commit_count: 100,
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
        };
        panel.refresh();
        panel
    }

    /// Create a new Git Log panel from a list of paths (from panels/session)
    pub fn new(paths: &[PathBuf]) -> Self {
        Self::create(RepoManager::new(paths))
    }

    /// Create panel for a specific repository (used for session restore)
    pub fn new_for_repo(repo_path: PathBuf) -> Self {
        Self::create(RepoManager::for_repo(repo_path))
    }

    /// Update repository list based on new paths from panels
    pub fn update_repos(&mut self, paths: &[PathBuf]) {
        if self.repo_manager.update(paths) {
            self.refresh();
        }
    }

    /// Trigger a refresh of the commit log.
    ///
    /// Returns immediately; the `get_all_branches` and
    /// `get_log_with_graph` git commands run on a worker thread and
    /// `tick()` folds the result in via [`Self::poll_refresh`].
    /// Reset displayed log state to empty — used when no repository is selected
    /// (e.g. the current repo's `.git` was deleted) so stale commits/branch
    /// don't linger.
    fn clear_git_state(&mut self) {
        self.branch = None;
        self.branches.clear();
        self.selected_branch = None;
        self.commits.clear();
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn refresh(&mut self) {
        let Some(repo) = self.repo_manager.current() else {
            self.clear_git_state();
            return;
        };
        let repo = repo.to_path_buf();
        let count = self.commit_count;
        let selected_branch = self.selected_branch.clone();
        let unicode_graph = self.unicode_graph;

        let (tx, rx) = std::sync::mpsc::channel();
        self.refresh_rx = Some(rx);
        std::thread::spawn(move || {
            let branch = git::get_current_branch(&repo);
            let branches = git::get_all_branches(&repo);
            let commits = if unicode_graph {
                git::get_log_graph_unicode(&repo, count, selected_branch.as_deref())
            } else {
                git::get_log_with_graph(&repo, count, selected_branch.as_deref())
            };
            let _ = tx.send(GitLogRefreshResult {
                branch,
                branches,
                commits,
            });
        });
    }

    /// Apply an async refresh result if one is ready. Returns `true`
    /// when the panel state changed so the caller can emit
    /// `NeedsRedraw`.
    fn poll_refresh(&mut self) -> bool {
        let Some(rx) = self.refresh_rx.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.refresh_rx = None;
                return true;
            }
        };
        self.refresh_rx = None;

        self.branch = result.branch;
        self.branches = result.branches;

        // If the previously selected branch no longer exists, reset to HEAD
        if let Some(ref b) = self.selected_branch {
            if !self.branches.contains(b) {
                self.selected_branch = None;
            }
        }

        self.commits = result.commits;
        if self.selected >= self.commits.len() && !self.commits.is_empty() {
            self.selected = self.commits.len() - 1;
        }
        self.scroll = 0;
        true
    }

    /// Move to next section (cycles: RepoSelector → BranchSelector → Commits → RepoSelector)
    fn next_section(&mut self) {
        self.current_section = match self.current_section {
            Section::RepoSelector => Section::BranchSelector,
            Section::BranchSelector => Section::Commits,
            Section::Commits => Section::RepoSelector,
        };
    }

    /// Move to previous section
    fn prev_section(&mut self) {
        self.current_section = match self.current_section {
            Section::RepoSelector => Section::Commits,
            Section::BranchSelector => Section::RepoSelector,
            Section::Commits => Section::BranchSelector,
        };
    }

    /// Move selection up
    /// Item count of the currently open selector dropdown (repo or branch).
    fn open_dropdown_len(&self) -> usize {
        if self.repo_dropdown_open {
            self.repo_manager.len()
        } else if self.branch_dropdown_open {
            self.branches.len()
        } else {
            0
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.ensure_visible();
        }
    }

    /// Move selection down
    fn move_down(&mut self) {
        if self.selected + 1 < self.commits.len() {
            self.selected += 1;
            self.ensure_visible();
        }
    }

    /// Page up
    fn page_up(&mut self, page_size: usize) {
        if self.selected > page_size {
            self.selected -= page_size;
        } else {
            self.selected = 0;
        }
        self.ensure_visible();
    }

    /// Page down
    fn page_down(&mut self, page_size: usize) {
        let max = self.commits.len().saturating_sub(1);
        if self.selected + page_size < max {
            self.selected += page_size;
        } else {
            self.selected = max;
        }
        self.ensure_visible();
    }

    /// Go to first commit
    fn go_to_start(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    /// Go to last commit
    fn go_to_end(&mut self) {
        if !self.commits.is_empty() {
            self.selected = self.commits.len() - 1;
            self.ensure_visible();
        }
    }

    /// Ensure selected item is visible
    fn ensure_visible(&mut self) {
        let visible_height = self.last_area.height.saturating_sub(2) as usize;
        if visible_height == 0 {
            return;
        }

        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_height {
            self.scroll = self.selected - visible_height + 1;
        }
    }

    /// Get selected commit
    fn selected_commit(&self) -> Option<&CommitInfo> {
        self.commits.get(self.selected)
    }

    /// Take pending modal request (called by the app via PanelExt).
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    /// Show commit info modal for the selected commit.
    fn show_commit_info(&mut self) {
        let Some(commit) = self.selected_commit() else {
            return;
        };
        if commit.hash.is_empty() {
            return;
        }
        let hash = commit.hash.clone();

        let Some(repo) = self.repo_manager.current() else {
            return;
        };
        let repo = repo.to_path_buf();

        let Some(details) = git::get_commit_details(&repo, &hash) else {
            return;
        };

        let t = termide_i18n::t();
        let short_hash = if details.hash.len() > 8 {
            &details.hash[..8]
        } else {
            &details.hash
        };
        let title = t.git_commit_info_title(short_hash);

        // Build colored file status segments (only non-zero counts)
        let mut file_segments = Vec::new();
        if details.files_modified > 0 {
            file_segments.push(StyledSegment {
                text: details.files_modified.to_string(),
                style: SegmentStyle::Warning,
            });
            file_segments.push(StyledSegment {
                text: format!(" {}", t.git_commit_files_modified()),
                style: SegmentStyle::Default,
            });
        }
        if details.files_added > 0 {
            if !file_segments.is_empty() {
                file_segments.push(StyledSegment {
                    text: "  ".to_string(),
                    style: SegmentStyle::Default,
                });
            }
            file_segments.push(StyledSegment {
                text: details.files_added.to_string(),
                style: SegmentStyle::Success,
            });
            file_segments.push(StyledSegment {
                text: format!(" {}", t.git_commit_files_added()),
                style: SegmentStyle::Default,
            });
        }
        if details.files_deleted > 0 {
            if !file_segments.is_empty() {
                file_segments.push(StyledSegment {
                    text: "  ".to_string(),
                    style: SegmentStyle::Default,
                });
            }
            file_segments.push(StyledSegment {
                text: details.files_deleted.to_string(),
                style: SegmentStyle::Error,
            });
            file_segments.push(StyledSegment {
                text: format!(" {}", t.git_commit_files_deleted()),
                style: SegmentStyle::Default,
            });
        }
        // Fallback if all zero
        if file_segments.is_empty() {
            file_segments.push(StyledSegment {
                text: "0".to_string(),
                style: SegmentStyle::Disabled,
            });
        }

        // Build colored lines segments (+N green, -N red)
        let lines_segments = vec![
            StyledSegment {
                text: format!("+{}", details.insertions),
                style: SegmentStyle::Success,
            },
            StyledSegment {
                text: " / ".to_string(),
                style: SegmentStyle::Default,
            },
            StyledSegment {
                text: format!("-{}", details.deletions),
                style: SegmentStyle::Error,
            },
        ];

        let data = vec![
            (
                t.git_commit_author().to_string(),
                ModalValue::Text(details.author),
            ),
            (
                t.git_commit_date().to_string(),
                ModalValue::Text(details.date),
            ),
            (
                t.git_commit_message().to_string(),
                ModalValue::Text(details.message),
            ),
            (
                t.git_commit_files().to_string(),
                ModalValue::Segments(file_segments),
            ),
            (
                t.git_commit_lines().to_string(),
                ModalValue::Segments(lines_segments),
            ),
        ];

        let modal = InfoModal::new_rich(title, data);
        self.modal_request = Some((
            PendingAction::VfsMessage,
            ActiveModal::Info(Box::new(modal)),
        ));
    }

    /// Open selected commit in browser via remote URL, or show fallback message.
    fn open_commit_external(&mut self) -> Vec<PanelEvent> {
        let Some(commit) = self.selected_commit() else {
            return vec![];
        };
        if commit.hash.is_empty() {
            return vec![];
        }
        let hash = commit.hash.clone();

        let Some(repo) = self.repo_manager.current() else {
            return vec![];
        };

        if let Some(url) = git::get_commit_web_url(repo, &hash) {
            return vec![PanelEvent::OpenExternal(PathBuf::from(url))];
        }

        let t = termide_i18n::t();
        self.status_message = Some(t.git_no_remote_url().to_string());
        vec![]
    }

    /// View diff for selected commit
    fn view_diff(&mut self) -> Vec<PanelEvent> {
        let Some(commit) = self.selected_commit() else {
            return vec![];
        };
        if commit.hash.is_empty() {
            return vec![];
        }

        let Some(repo) = self.repo_manager.current() else {
            return vec![];
        };

        // Open Git Diff panel for this commit
        vec![PanelEvent::OpenGitDiff {
            repo_path: repo.to_path_buf(),
            commit_hash: Some(commit.hash.clone()),
            file_path: None,
        }]
    }

    /// Render repo selector
    /// Returns the rendered width so the caller can position the next widget.
    fn render_repo_selector(
        &mut self,
        x: u16,
        y: u16,
        max_width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) -> u16 {
        let name = self.repo_manager.current().map(git::get_repo_name);
        if let Some(name) = name {
            let is_focused = self.current_section == Section::RepoSelector;
            let w = InlineSelector::new(&name, self.repo_dropdown_open, is_focused, theme)
                .render(x, y, max_width, buf);
            self.repo_selector_area = Some(Rect {
                x,
                y,
                width: w,
                height: 1,
            });
            return w;
        }
        0
    }

    /// Render branch selector
    fn render_branch_selector(
        &mut self,
        x: u16,
        y: u16,
        max_width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) {
        let name: String = self
            .selected_branch
            .as_deref()
            .or(self.branch.as_deref())
            .unwrap_or("—")
            .to_owned();
        let is_focused = self.current_section == Section::BranchSelector;
        let w = InlineSelector::new(&name, self.branch_dropdown_open, is_focused, theme)
            .render(x, y, max_width, buf);
        self.branch_selector_area = Some(Rect {
            x,
            y,
            width: w,
            height: 1,
        });
    }

    /// Render the panel content
    fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        border_right_x: Option<u16>,
    ) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        let theme = self.cached_theme;

        // Use the full inner area (the host already draws the border): content
        // fills the panel edge-to-edge, so the selection bar spans the whole
        // width and no blank row is left at the bottom.
        let content_area = area;

        // Always render header row: [repo ▼] [branch ▼] (branch follows repo with 2-char gap)
        let repo_w = self.render_repo_selector(
            content_area.x,
            content_area.y,
            content_area.width / 2,
            buf,
            &theme,
        );
        let branch_x = content_area.x + repo_w + 2;
        let branch_max_w = content_area.width.saturating_sub(repo_w + 2);
        self.render_branch_selector(branch_x, content_area.y, branch_max_w, buf, &theme);
        let y_offset = 2u16;

        let commits_area_height = content_area.height.saturating_sub(y_offset) as usize;
        let commits_start_y = content_area.y + y_offset;

        // Check if scrollbar is needed (will be rendered on border, so no width reservation)
        let needs_scrollbar = ScrollBar::needs_scrollbar(commits_area_height, self.commits.len());
        let commits_width = content_area.width;

        // Render commits
        for (i, commit) in self
            .commits
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(commits_area_height)
        {
            let y = commits_start_y + (i - self.scroll) as u16;
            // Only show the selection cursor while focused (hidden otherwise).
            let is_selected =
                is_focused && i == self.selected && self.current_section == Section::Commits;

            // Clear line first
            let clear_style = if is_selected {
                Style::default().bg(theme.fg)
            } else {
                Style::default().bg(theme.bg)
            };
            for x in content_area.x..content_area.x + commits_width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(clear_style);
                }
            }

            let mut x_pos = content_area.x;
            let max_x = content_area.x + commits_width;

            // Graph prefix (if available)
            if let Some(ref graph) = commit.graph {
                if self.unicode_graph {
                    // Box-drawing engine: colour each lane by its column so a
                    // branch can be followed by colour (tig/lazygit style). A
                    // glyph's char index equals its lane column (1-cell glyphs,
                    // lanes start at 0); trailing pad spaces stay uncoloured.
                    for (col, ch) in graph.chars().enumerate() {
                        let cx = x_pos + col as u16;
                        if cx >= max_x {
                            break;
                        }
                        if ch == ' ' {
                            continue;
                        }
                        if let Some(cell) = buf.cell_mut((cx, y)) {
                            cell.set_char(ch);
                            if is_selected {
                                // Cursor row: invert the panel's fg/bg.
                                cell.set_fg(theme.bg);
                                cell.set_bg(theme.fg);
                            } else {
                                cell.set_fg(lane_color(&theme, col));
                            }
                        }
                    }
                } else {
                    // ASCII git --graph fallback: diagonals shift columns, so a
                    // single muted colour reads better than per-column tinting.
                    let graph_style = if is_selected {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else {
                        Style::default().fg(theme.disabled)
                    };
                    buf.set_string(x_pos, y, graph, graph_style);
                }
                x_pos += graph.width() as u16;
            }

            if !commit.hash.is_empty() && x_pos < max_x {
                // Hash
                let hash_style = if is_selected {
                    Style::default().fg(theme.bg).bg(theme.fg)
                } else {
                    Style::default().fg(theme.cursor)
                };
                buf.set_string(x_pos, y, &commit.hash, hash_style);
                x_pos += commit.hash.width() as u16 + 1;

                // Refs (if any) - render with colors
                if let Some(ref refs) = commit.refs {
                    if x_pos < max_x {
                        x_pos = self.render_refs(x_pos, y, max_x, refs, is_selected, buf, &theme);
                        x_pos += 1; // space after refs
                    }
                }

                // Author
                if x_pos < max_x {
                    let author = truncate_right(&commit.author, 15);
                    let author_style = if is_selected {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else {
                        Style::default().fg(theme.info)
                    };
                    buf.set_string(x_pos, y, &author, author_style);
                    x_pos += author.width() as u16 + 1;
                }

                // Date
                if x_pos < max_x {
                    let date = truncate_right(&commit.date, 12);
                    let date_style = if is_selected {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else {
                        Style::default().fg(theme.disabled)
                    };
                    buf.set_string(x_pos, y, &date, date_style);
                    x_pos += date.width() as u16 + 1;
                }

                // Message
                if x_pos < max_x {
                    let remaining = (max_x - x_pos) as usize;
                    let message = if commit.message.width() > remaining {
                        truncate_to_width(&commit.message, remaining)
                    } else {
                        commit.message.clone()
                    };
                    let msg_style = if is_selected {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    buf.set_string(x_pos, y, &message, msg_style);
                }
            }
        }

        // Render scrollbar on border
        if needs_scrollbar {
            if let Some(border_x) = border_right_x {
                ScrollBar::render(
                    buf,
                    border_x,
                    commits_start_y,
                    commits_area_height as u16,
                    self.scroll,
                    commits_area_height,
                    self.commits.len(),
                    &theme,
                    is_focused,
                );
            }
        }

        // Dropdown overlays (rendered last so they appear on top)
        if self.repo_dropdown_open {
            let repo_names: Vec<String> = self
                .repo_manager
                .repos()
                .iter()
                .map(|p| git::get_repo_name(p))
                .collect();
            let dropdown_y = content_area.y + 1;
            let max_h = content_area.height.saturating_sub(3);
            let selected_idx = self.repo_manager.selected_index();
            let visible_count = repo_names.len().min(max_h as usize);
            self.dropdown_area = Some(Rect {
                x: content_area.x,
                y: dropdown_y,
                width: content_area.width / 2,
                height: visible_count as u16 + 2,
            });
            render_simple_dropdown(
                &repo_names,
                selected_idx,
                self.dropdown_cursor,
                content_area.x,
                dropdown_y,
                content_area.width / 2,
                max_h,
                buf,
                &theme,
            );
        } else if self.branch_dropdown_open {
            let branches = self.branches.clone();
            let current_branch_idx = branches
                .iter()
                .position(|b| Some(b.as_str()) == self.branch.as_deref())
                .unwrap_or(0);
            let dropdown_y = content_area.y + 1;
            let max_h = content_area.height.saturating_sub(3);
            // Use actual branch selector x (set during header render above)
            let branch_x = self
                .branch_selector_area
                .map(|a| a.x)
                .unwrap_or(content_area.x);
            let dropdown_w = content_area
                .width
                .saturating_sub(branch_x.saturating_sub(content_area.x));
            let visible_count = branches.len().min(max_h as usize);
            self.dropdown_area = Some(Rect {
                x: branch_x,
                y: dropdown_y,
                width: dropdown_w,
                height: visible_count as u16 + 2,
            });
            render_simple_dropdown(
                &branches,
                current_branch_idx,
                self.dropdown_cursor,
                branch_x,
                dropdown_y,
                dropdown_w,
                max_h,
                buf,
                &theme,
            );
        } else {
            self.dropdown_area = None;
        }

        // Status message at bottom
        if let Some(ref msg) = self.status_message {
            let msg_y = area.y + area.height.saturating_sub(2);
            let style = Style::default().fg(theme.cursor);
            let max_w = content_area.width as usize;
            let truncated: std::borrow::Cow<str> = if msg.width() > max_w {
                let mut end = 0;
                let mut w = 0;
                for ch in msg.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if w + cw > max_w {
                        break;
                    }
                    w += cw;
                    end += ch.len_utf8();
                }
                std::borrow::Cow::Borrowed(&msg[..end])
            } else {
                std::borrow::Cow::Borrowed(msg)
            };
            buf.set_string(content_area.x, msg_y, truncated, style);
        }
    }

    /// Render refs with colors
    /// Returns the x position after rendering
    #[allow(clippy::too_many_arguments)]
    fn render_refs(
        &self,
        start_x: u16,
        y: u16,
        max_x: u16,
        refs: &str,
        is_selected: bool,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) -> u16 {
        let mut x = start_x;

        // Refs format: (HEAD -> main, origin/main, tag: v1.0)
        // Remove parentheses
        let refs_inner = refs.trim_start_matches('(').trim_end_matches(')');
        if refs_inner.is_empty() {
            return x;
        }

        // Render opening paren
        let paren_style = if is_selected {
            Style::default().fg(theme.bg).bg(theme.fg)
        } else {
            Style::default().fg(theme.disabled)
        };
        buf.set_string(x, y, "(", paren_style);
        x += 1;

        // Parse and render each ref
        for (i, ref_part) in refs_inner.split(", ").enumerate() {
            if x >= max_x {
                break;
            }

            // Add comma separator
            if i > 0 {
                buf.set_string(x, y, ", ", paren_style);
                x += 2;
            }

            if x >= max_x {
                break;
            }

            // Determine ref type and color
            let style = if is_selected {
                Style::default().fg(theme.bg).bg(theme.fg)
            } else {
                let fg = if ref_part.contains("HEAD") {
                    theme.error
                } else if ref_part.starts_with("tag:") {
                    theme.warning
                } else if ref_part.contains('/') {
                    theme.info
                } else {
                    theme.success
                };
                Style::default().fg(fg)
            };

            let text_width = ref_part.width() as u16;
            if x + text_width <= max_x {
                buf.set_string(x, y, ref_part, style);
                x += text_width;
            } else {
                // Truncate
                let remaining = (max_x - x) as usize;
                if remaining > 0 {
                    let truncated = truncate_to_width(ref_part, remaining);
                    buf.set_string(x, y, &truncated, style);
                    x = max_x;
                }
                break;
            }
        }

        // Render closing paren
        if x < max_x {
            buf.set_string(x, y, ")", paren_style);
            x += 1;
        }

        x
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
        let branch = self.branch.as_deref().unwrap_or(t.git_branch_detached());
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

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::Reload | PanelCommand::RefreshIfStale => {
                self.refresh();
                CommandResult::NeedsRedraw(true)
            }
            PanelCommand::UpdateRepoPaths { paths } => {
                self.update_repos(&paths);
                CommandResult::NeedsRedraw(true)
            }
            // Live watcher updates: a commit / ref / rebase touches `.git` and
            // arrives as OnGitUpdate — reload the log if it's this repo. (Plain
            // working-tree edits don't change the graph, so OnFsUpdate is
            // ignored.)
            PanelCommand::OnGitUpdate { repo_paths } => {
                let hit = if let Some(cur) = self.repo_manager.current() {
                    repo_paths
                        .iter()
                        .any(|p| cur.starts_with(p) || p.starts_with(cur))
                } else {
                    false
                };
                if hit {
                    self.refresh();
                    return CommandResult::NeedsRedraw(true);
                }
                CommandResult::NeedsRedraw(false)
            }
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
        events
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        // Clear status message on any key
        self.status_message = None;

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
            if self.hotkeys.matches("clipboard_copy", &key) {
                if let Some(commit) = self.selected_commit() {
                    if !commit.hash.is_empty() {
                        let hash = commit.hash.clone();
                        let _ = termide_clipboard::copy(&hash);
                        self.status_message = Some(format!("Copied: {}", hash));
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
                            self.dropdown_cursor = self
                                .branches
                                .iter()
                                .position(|b| Some(b.as_str()) == self.branch.as_deref())
                                .unwrap_or(0);
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

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
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
                            let scroll_offset = if self.dropdown_cursor >= visible_rows {
                                self.dropdown_cursor - visible_rows + 1
                            } else {
                                0
                            };
                            let item_idx = scroll_offset + (row - area.y - 1) as usize;
                            if self.repo_dropdown_open {
                                self.repo_manager.select(item_idx);
                                self.repo_dropdown_open = false;
                                self.selected_branch = None;
                                self.refresh();
                            } else if self.branch_dropdown_open {
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
                                self.dropdown_cursor = self
                                    .branches
                                    .iter()
                                    .position(|b| Some(b.as_str()) == self.branch.as_deref())
                                    .unwrap_or(0);
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
                            self.dropdown_cursor = self
                                .branches
                                .iter()
                                .position(|b| Some(b.as_str()) == self.branch.as_deref())
                                .unwrap_or(0);
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
                    self.move_up();
                }
            }
            MouseEventKind::ScrollDown => {
                if self.repo_dropdown_open || self.branch_dropdown_open {
                    let max = self.open_dropdown_len().saturating_sub(1);
                    if self.dropdown_cursor < max {
                        self.dropdown_cursor += 1;
                    }
                } else {
                    self.move_down();
                }
            }
            _ => {}
        }
        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
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
        if delta < 0 {
            // Scroll up - move selection up by delta
            self.selected = self.selected.saturating_sub(lines);
        } else {
            // Scroll down - move selection down by delta
            self.selected = (self.selected + lines).min(self.commits.len().saturating_sub(1));
        }
        self.ensure_visible();
        vec![]
    }

    fn to_session(&self, _session_dir: &Path) -> Option<SessionPanel> {
        self.repo_manager
            .current()
            .map(|repo| SessionPanel::GitLog {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_color_is_stable_and_cycles() {
        let theme = ThemeColors::default();
        // Lane 0 (mainline) keeps the first palette entry.
        assert_eq!(lane_color(&theme, 0), theme.info);
        // Palette has 5 entries and wraps around.
        assert_eq!(lane_color(&theme, 5), lane_color(&theme, 0));
        assert_eq!(lane_color(&theme, 6), lane_color(&theme, 1));
        // Adjacent lanes differ.
        assert_ne!(lane_color(&theme, 0), lane_color(&theme, 1));
    }
}
