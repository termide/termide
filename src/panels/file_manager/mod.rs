mod file_info;
mod navigation;
mod operations;
mod rendering;
mod selection;
mod utils;

pub use file_info::{DiskSpaceInfo, FileInfo};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect, prelude::Widget, widgets::Paragraph};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;

use super::Panel;
use crate::git::{get_git_status, GitStatus, GitStatusCache};
use crate::i18n;
use crate::state::AppState;

use crate::state::{ActiveModal, DirSizeResult, PendingAction};
use crate::ui::modal::{ConfirmModal, InputModal};

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragMode {
    Select, // Shift+drag - selection
    Toggle, // Ctrl+drag - toggle selection
}

/// Smart file manager with advanced features
pub struct FileManager {
    current_path: PathBuf,
    entries: Vec<FileEntry>,
    selected: usize,
    scroll_offset: usize,
    /// Last displayed title (cached for [X] clicks)
    display_title: String,
    /// File to open in editor (set when Enter is pressed on a file)
    file_to_open: Option<PathBuf>,
    /// Modal window request (action, modal)
    modal_request: Option<(PendingAction, ActiveModal)>,
    /// Visible area height (updated during rendering)
    visible_height: usize,
    /// Time of last click for double-click detection
    last_click_time: Option<std::time::Instant>,
    /// Index of element that was last clicked
    last_click_index: Option<usize>,
    /// Set of selected items (indices)
    selected_items: HashSet<usize>,
    /// Git status cache for the current directory
    git_status_cache: Option<GitStatusCache>,
    /// Channel receiver for directory size calculation results (needs to be passed to AppState)
    pub dir_size_receiver: Option<mpsc::Receiver<DirSizeResult>>,
    /// Starting index for drag selection
    drag_start_index: Option<usize>,
    /// Drag mode (Shift/Ctrl)
    drag_mode: Option<DragMode>,
    /// Set of items already processed during current drag (to avoid re-toggling)
    dragged_items: HashSet<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    #[allow(dead_code)]
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_executable: bool,
    pub is_readonly: bool,
    pub git_status: GitStatus,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

impl FileManager {
    /// Create a new smart file manager
    pub fn new() -> Self {
        let current_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self::new_with_path(current_path)
    }

    /// Create a new smart file manager with the specified path
    pub fn new_with_path(current_path: PathBuf) -> Self {
        let display_title = current_path.display().to_string();
        let mut fm = Self {
            current_path: current_path.clone(),
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            display_title,
            file_to_open: None,
            modal_request: None,
            visible_height: 10, // Default value, will be updated during rendering
            last_click_time: None,
            last_click_index: None,
            selected_items: HashSet::new(),
            git_status_cache: None,
            dir_size_receiver: None,
            drag_start_index: None,
            drag_mode: None,
            dragged_items: HashSet::new(),
        };
        let _ = fm.load_directory();
        fm
    }

    /// Get the current directory
    pub fn get_current_directory(&self) -> PathBuf {
        self.current_path.clone()
    }

    /// Load the contents of the current directory
    pub fn load_directory(&mut self) -> Result<()> {
        // Save current file name to restore position
        let current_name = self.entries.get(self.selected).map(|e| e.name.clone());

        self.entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        // Clear selection when changing directory
        self.selected_items.clear();
        // Clear drag state
        self.drag_start_index = None;
        self.drag_mode = None;
        self.dragged_items.clear();

        // Update displayed title (will be truncated during rendering if needed)
        self.display_title = self.current_path.display().to_string();

        // Load git statuses for the current directory
        self.git_status_cache = get_git_status(&self.current_path);

        // Add parent directory if not at root
        if self.current_path.parent().is_some() {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                is_hidden: false,
                is_symlink: false,
                is_executable: false,
                is_readonly: false,
                git_status: GitStatus::Unmodified,
                size: None,
                modified: None,
            });
        }

        // Read directory contents
        if let Ok(read_dir) = fs::read_dir(&self.current_path) {
            for entry in read_dir.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_hidden = name.starts_with('.');

                    // Determine git status for this entry
                    let git_status = if metadata.is_dir() {
                        // For directories: check recursively for nested changes
                        self.git_status_cache
                            .as_ref()
                            .map(|cache| cache.get_directory_status(&name))
                            .unwrap_or(GitStatus::Unmodified)
                    } else {
                        // For files: use direct status
                        self.git_status_cache
                            .as_ref()
                            .map(|cache| cache.get_status(&name))
                            .unwrap_or(GitStatus::Unmodified)
                    };

                    // Check if this is a symlink (use symlink_metadata to not follow links)
                    let is_symlink = if let Ok(link_metadata) = fs::symlink_metadata(entry.path()) {
                        link_metadata.is_symlink()
                    } else {
                        false
                    };

                    // Check if file is executable (Unix permissions)
                    #[cfg(unix)]
                    let is_executable = {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.permissions().mode() & 0o111 != 0
                    };
                    #[cfg(not(unix))]
                    let is_executable = false;

                    // Check if file is read-only (Unix permissions)
                    #[cfg(unix)]
                    let is_readonly = {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = metadata.permissions().mode();
                        (mode & 0o200) == 0 // owner write bit
                    };
                    #[cfg(not(unix))]
                    let is_readonly = metadata.permissions().readonly();

                    // Get size (files only) and modification time
                    let size = if metadata.is_file() {
                        Some(metadata.len())
                    } else {
                        None
                    };
                    let modified = metadata.modified().ok();

                    self.entries.push(FileEntry {
                        name,
                        is_dir: metadata.is_dir(),
                        is_hidden,
                        is_symlink,
                        is_executable,
                        is_readonly,
                        git_status,
                        size,
                        modified,
                    });
                }
            }
        }

        // Sort: directories first, then files
        self.entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        // Restore cursor position if a file with that name exists
        if let Some(name) = current_name {
            if let Some(pos) = self.entries.iter().position(|e| e.name == name) {
                self.selected = pos;
                // Update scroll_offset if needed
                self.adjust_scroll_offset(20); // 20 - approximate visible area height
            }
        }

        Ok(())
    }

    /// Update git status for current directory without reloading entire directory
    pub fn update_git_status(&mut self) -> Result<()> {
        // Refresh git status cache
        self.git_status_cache = get_git_status(&self.current_path);

        // Update git_status for each entry (except "..")
        for entry in &mut self.entries {
            if entry.name == ".." {
                continue;
            }

            entry.git_status = if entry.is_dir {
                // For directories: check recursively for nested changes
                self.git_status_cache
                    .as_ref()
                    .map(|cache| cache.get_directory_status(&entry.name))
                    .unwrap_or(GitStatus::Unmodified)
            } else {
                // For files: use direct status
                self.git_status_cache
                    .as_ref()
                    .map(|cache| cache.get_status(&entry.name))
                    .unwrap_or(GitStatus::Unmodified)
            };
        }

        Ok(())
    }

    /// Get current directory path
    pub fn current_path(&self) -> &std::path::Path {
        &self.current_path
    }

    /// Enter directory or open file
    fn enter(&mut self) -> Result<()> {
        if let Some(entry) = self.entries.get(self.selected) {
            // Prohibit operations on deleted files
            if entry.git_status == GitStatus::Deleted {
                return Ok(());
            }

            if entry.name == ".." {
                if let Some(parent) = self.current_path.parent() {
                    self.current_path = parent.to_path_buf();
                    self.load_directory()?;
                }
            } else if entry.is_dir {
                self.current_path.push(&entry.name);
                self.load_directory()?;
            } else {
                // This is a file - set path for opening in editor
                let file_path = self.current_path.join(&entry.name);
                self.file_to_open = Some(file_path);
            }
        }
        Ok(())
    }

    /// Open file for editing (F4)
    fn edit_file(&mut self) -> Result<()> {
        if let Some(entry) = self.entries.get(self.selected) {
            // Prohibit operations on deleted files
            if entry.git_status == GitStatus::Deleted {
                return Ok(());
            }

            // Check that this is a file, not a directory and not ".."
            if !entry.is_dir && entry.name != ".." {
                let file_path = self.current_path.join(&entry.name);
                self.file_to_open = Some(file_path);
            }
        }
        Ok(())
    }

    /// Format file size in human-readable format (public method for external use)
    pub fn format_size_static(bytes: u64) -> String {
        utils::format_size(bytes)
    }
}

impl Panel for FileManager {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        panel_index: usize,
        state: &AppState,
    ) {
        // Automatically update scroll offset
        let content_height = area.height.saturating_sub(2) as usize; // -2 for borders
        self.visible_height = content_height; // Save for use in handle_key

        if self.selected >= self.scroll_offset + content_height {
            self.scroll_offset = self.selected - content_height + 1;
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }

        // Determine if this panel can be closed (for correct path width calculation)
        let can_close = crate::ui::panel_helpers::can_close_panel(panel_index, state);

        // Get display path taking into account panel width and [X] button presence
        let display_title = self.get_display_title(area.width, can_close);
        self.display_title = display_title.clone();

        // Calculate available width for file names (subtract borders: 2 chars)
        let content_width = area.width.saturating_sub(2) as usize;
        let items = self.get_items(content_height, content_width, state.theme, is_focused);

        let block = crate::ui::panel_helpers::create_panel_block(
            &display_title,
            is_focused,
            panel_index,
            state,
        );

        let paragraph = Paragraph::new(items).block(block);

        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Translate Cyrillic to Latin for hotkeys
        let key = crate::keyboard::translate_hotkey(key);

        match (key.code, key.modifiers) {
            // Ctrl+A - select all
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.select_all();
            }
            // Ctrl+R - refresh file list
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.load_directory()?;
            }
            // Insert - toggle selection of current item and move down
            (KeyCode::Insert, KeyModifiers::NONE) => {
                self.toggle_selection();
                self.move_down();
            }
            // Space - show file information
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                self.show_file_info();
            }
            // Shift+Down - select down
            (KeyCode::Down, KeyModifiers::SHIFT) => {
                self.move_down_with_selection();
            }
            // Shift+Up - select up
            (KeyCode::Up, KeyModifiers::SHIFT) => {
                self.move_up_with_selection();
            }
            // Shift+PageDown - select page down
            (KeyCode::PageDown, KeyModifiers::SHIFT) => {
                self.page_down_with_selection();
            }
            // Shift+PageUp - select page up
            (KeyCode::PageUp, KeyModifiers::SHIFT) => {
                self.page_up_with_selection();
            }
            // Shift+Home - select to beginning
            (KeyCode::Home, KeyModifiers::SHIFT) => {
                self.select_to_home();
            }
            // Shift+End - select to end
            (KeyCode::End, KeyModifiers::SHIFT) => {
                self.select_to_end();
            }
            // Ctrl+Down - toggle selection down
            (KeyCode::Down, KeyModifiers::CONTROL) => {
                self.move_down_with_toggle();
            }
            // Ctrl+Up - toggle selection up
            (KeyCode::Up, KeyModifiers::CONTROL) => {
                self.move_up_with_toggle();
            }
            // Ctrl+PageDown - toggle selection page down
            (KeyCode::PageDown, KeyModifiers::CONTROL) => {
                self.page_down_with_toggle();
            }
            // Ctrl+PageUp - toggle selection page up
            (KeyCode::PageUp, KeyModifiers::CONTROL) => {
                self.page_up_with_toggle();
            }
            // Regular keys - move without clearing selection
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.move_down();
            }
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.move_up();
            }
            // Escape - clear selection
            (KeyCode::Esc, KeyModifiers::NONE) => {
                self.selected_items.clear();
            }
            (KeyCode::Enter, _) => {
                self.enter()?;
            }
            (KeyCode::Backspace, _) => {
                // Return to parent directory
                if let Some(parent) = self.current_path.parent() {
                    self.current_path = parent.to_path_buf();
                    self.load_directory()?;
                }
            }
            (KeyCode::PageUp, KeyModifiers::NONE) => {
                // Scroll up by visible area
                self.selected = self.selected.saturating_sub(self.visible_height);
            }
            (KeyCode::PageDown, KeyModifiers::NONE) => {
                // Scroll down by visible area
                let max_index = self.entries.len().saturating_sub(1);
                self.selected = (self.selected + self.visible_height).min(max_index);
            }
            (KeyCode::Home, KeyModifiers::NONE) => {
                // Go to beginning of list
                self.selected = 0;
                self.scroll_offset = 0;
            }
            (KeyCode::End, KeyModifiers::NONE) => {
                // Go to end of list
                self.selected = self.entries.len().saturating_sub(1);
            }
            (KeyCode::Char('~'), _) => {
                // Go to home directory
                if let Some(home) = dirs::home_dir() {
                    self.current_path = home;
                    self.load_directory()?;
                }
            }
            (KeyCode::Char('f'), _) | (KeyCode::Char('F'), _) => {
                // Create new file - open InputModal
                let t = i18n::t();
                let modal = InputModal::new(t.modal_create_file_title(), "");
                let action = PendingAction::CreateFile {
                    panel_index: 0, // will be updated in app.rs
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            (KeyCode::Char('d'), _) | (KeyCode::Char('D'), _) | (KeyCode::F(7), _) => {
                // Create new directory - open InputModal
                let t = i18n::t();
                let modal = InputModal::new(t.modal_create_dir_title(), "");
                let action = PendingAction::CreateDirectory {
                    panel_index: 0, // will be updated in app.rs
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            (KeyCode::Delete, _) | (KeyCode::F(8), _) => {
                // Delete selected files/directories - open ConfirmModal
                let paths = self.get_selected_paths();
                if paths.is_empty() {
                    return Ok(());
                }

                let t = i18n::t();
                let title = if paths.len() == 1 {
                    let file_name = paths[0].file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    t.modal_delete_single_title(file_name)
                } else {
                    t.modal_delete_multiple_title(paths.len())
                };

                let modal = crate::ui::modal::ConfirmModal::new(&title, "");
                let action = PendingAction::DeletePath {
                    panel_index: 0, // will be updated in app.rs
                    paths,
                };
                self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
            }
            (KeyCode::F(4), _) => {
                // Open selected file for editing
                self.edit_file()?;
            }
            // Ctrl+C - copy selected files to clipboard
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let text = paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    crate::clipboard::copy(text);
                }
            }
            // Ctrl+X - cut selected files to clipboard
            (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let text = paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    crate::clipboard::cut(text);
                }
            }
            // Ctrl+V - paste files from clipboard
            (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
                let (text, mode) = crate::clipboard::paste();

                // Split text by newlines and convert to paths
                let files: Vec<std::path::PathBuf> = text
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(std::path::PathBuf::from)
                    .filter(|path| path.exists()) // Only existing paths
                    .collect();

                if !files.is_empty() {
                    // Create confirmation modal
                    let t = i18n::t();
                    let mode_str = if matches!(mode, crate::clipboard::ClipboardMode::Copy) {
                        "Copy"
                    } else {
                        "Move"
                    };
                    let message = t.fm_paste_confirm(
                        files.len(),
                        mode_str,
                        &self.current_path.display().to_string(),
                    );

                    let action = match mode {
                        crate::clipboard::ClipboardMode::Copy => PendingAction::CopyPath {
                            panel_index: 0,
                            sources: files,
                            target_directory: Some(self.current_path.clone()),
                        },
                        crate::clipboard::ClipboardMode::Cut => PendingAction::MovePath {
                            panel_index: 0,
                            sources: files,
                            target_directory: Some(self.current_path.clone()),
                        },
                    };

                    let modal = crate::ui::modal::ConfirmModal::new("Confirm", &message);
                    self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));

                    // If it was a cut operation, clear the clipboard after execution
                    if matches!(mode, crate::clipboard::ClipboardMode::Cut) {
                        crate::clipboard::clear();
                    }
                }
            }
            (KeyCode::Char('c'), _) | (KeyCode::Char('C'), _) | (KeyCode::F(5), _) => {
                // Copy selected files/directories
                let paths = self.get_selected_paths();
                if paths.is_empty() {
                    return Ok(());
                }

                // Default - current directory
                let default_dest = format!("{}/", self.current_path.display());

                let t = i18n::t();
                let message = if paths.len() == 1 {
                    let name = paths[0].file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    t.fm_copy_prompt(name)
                } else {
                    format!("Copy {} items to:", paths.len())
                };

                let modal = InputModal::with_default("Copy", &message, &default_dest);
                let action = PendingAction::CopyPath {
                    panel_index: 0, // will be updated in app.rs
                    sources: paths,
                    target_directory: None, // will be set in app.rs
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            (KeyCode::Char('m'), _) | (KeyCode::Char('M'), _) | (KeyCode::F(6), _) => {
                // Move selected files/directories
                let paths = self.get_selected_paths();
                if paths.is_empty() {
                    return Ok(());
                }

                let t = i18n::t();
                let (message, default_dest) = if paths.len() == 1 {
                    let name = paths[0].file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    (t.fm_move_prompt(name), name.to_string())
                } else {
                    (
                        format!("Move {} items to:", paths.len()),
                        format!("{}/", self.current_path.display()),
                    )
                };

                let modal = InputModal::with_default("Move", &message, &default_dest);
                let action = PendingAction::MovePath {
                    panel_index: 0, // will be updated in app.rs
                    sources: paths,
                    target_directory: None, // will be set in app.rs
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            // Tab - go to next panel
            (KeyCode::Tab, KeyModifiers::NONE) => {
                // Use dummy ConfirmModal that won't be shown
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::NextPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            // Shift+Tab - go to previous panel
            (KeyCode::BackTab, _) => {
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::PrevPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn title(&self) -> String {
        self.display_title.clone()
    }

    fn take_file_to_open(&mut self) -> Option<PathBuf> {
        self.file_to_open.take()
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        Some(self.current_path.clone())
    }

    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Result<()> {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

        // Handle scroll first (works anywhere in panel)
        let visible_height = panel_area.height.saturating_sub(2) as usize;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected >= self.scroll_offset + visible_height {
                    self.selected = (self.scroll_offset + visible_height).saturating_sub(1);
                }
                return Ok(());
            }
            MouseEventKind::ScrollDown => {
                let max_scroll = self.entries.len().saturating_sub(visible_height);
                self.scroll_offset = (self.scroll_offset + 3).min(max_scroll);
                // Keep selected in visible area so render doesn't reset scroll
                if self.selected < self.scroll_offset {
                    self.selected = self.scroll_offset;
                }
                return Ok(());
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // End drag - handle this ALWAYS, even if outside panel
                self.drag_start_index = None;
                self.drag_mode = None;
                self.dragged_items.clear();
                return Ok(());
            }
            _ => {}
        }

        // Check that click is inside content area (not on borders)
        let inner_area = Rect {
            x: panel_area.x + 1,
            y: panel_area.y + 1,
            width: panel_area.width.saturating_sub(2),
            height: panel_area.height.saturating_sub(2),
        };

        // Check that click is inside inner area
        if mouse.column < inner_area.x
            || mouse.column >= inner_area.x + inner_area.width
            || mouse.row < inner_area.y
            || mouse.row >= inner_area.y + inner_area.height
        {
            return Ok(());
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Determine index of clicked item
                let relative_row = (mouse.row - inner_area.y) as usize;
                let clicked_index = self.scroll_offset + relative_row;

                if clicked_index < self.entries.len() {
                    // Check modifiers
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        // Shift+click - select range from selected to clicked_index
                        let start = self.selected.min(clicked_index);
                        let end = self.selected.max(clicked_index);
                        self.dragged_items.clear();
                        for i in start..=end {
                            self.selected_items.insert(i);
                            self.dragged_items.insert(i);
                        }
                        self.selected = clicked_index;
                        self.drag_start_index = Some(clicked_index);
                        self.drag_mode = Some(DragMode::Select);
                    } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                        // Ctrl+click - toggle selection on clicked element
                        if self.selected_items.contains(&clicked_index) {
                            self.selected_items.remove(&clicked_index);
                        } else {
                            self.selected_items.insert(clicked_index);
                        }
                        self.selected = clicked_index;
                        self.drag_start_index = Some(clicked_index);
                        self.drag_mode = Some(DragMode::Toggle);
                        // Track this item as already processed during drag
                        self.dragged_items.clear();
                        self.dragged_items.insert(clicked_index);
                    } else {
                        // Check for double click
                        let now = std::time::Instant::now();
                        let is_double_click = if let (Some(last_time), Some(last_index)) =
                            (self.last_click_time, self.last_click_index)
                        {
                            // Double click if less than DOUBLE_CLICK_INTERVAL_MS passed and clicked on same item
                            now.duration_since(last_time).as_millis()
                                < crate::constants::DOUBLE_CLICK_INTERVAL_MS
                                && last_index == clicked_index
                        } else {
                            false
                        };

                        if is_double_click {
                            // Double click - open file/directory
                            self.selected = clicked_index;
                            self.enter()?;
                            // Reset click state
                            self.last_click_time = None;
                            self.last_click_index = None;
                        } else {
                            // Single click - select item
                            self.selected = clicked_index;
                            // Save time and index for double-click detection
                            self.last_click_time = Some(now);
                            self.last_click_index = Some(clicked_index);
                        }
                        self.drag_start_index = None;
                        self.drag_mode = None;
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Handle drag only if there's a drag_mode
                if let Some(drag_mode) = self.drag_mode {
                    let relative_row = (mouse.row - inner_area.y) as usize;
                    let current_index = self.scroll_offset + relative_row;

                    if current_index < self.entries.len() {
                        match drag_mode {
                            DragMode::Select => {
                                // Shift+drag - select current item if not already processed
                                if !self.dragged_items.contains(&current_index) {
                                    self.selected_items.insert(current_index);
                                    self.dragged_items.insert(current_index);
                                }
                            }
                            DragMode::Toggle => {
                                // Ctrl+drag - toggle current item if not already processed
                                if !self.dragged_items.contains(&current_index) {
                                    if self.selected_items.contains(&current_index) {
                                        self.selected_items.remove(&current_index);
                                    } else {
                                        self.selected_items.insert(current_index);
                                    }
                                    self.dragged_items.insert(current_index);
                                }
                            }
                        }
                        self.selected = current_index;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        // Reload directory contents (update git statuses)
        self.load_directory()
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        // FileManager doesn't store critical state by itself
        // Pending batch operations are checked in has_panels_requiring_confirmation()
        None
    }
}

impl Default for FileManager {
    fn default() -> Self {
        Self::new()
    }
}
