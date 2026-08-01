//! Directory picker modal dialog with tree-view navigation.
//!
//! Supports expand/collapse of subdirectories (Right/Left arrows),
//! cursor rendering matching the file manager panel style.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};

use crate::base::render_modal_block;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use termide_theme::Theme;

use crate::{calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig};

/// Directory entry for tree display
#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    full_path: PathBuf,
    depth: usize,
    /// `Some(true)` = expanded, `Some(false)` = collapsed, `None` = ".." entry
    expanded: Option<bool>,
}

/// Directory picker modal window with tree navigation
#[derive(Debug)]
pub struct DirectoryPickerModal {
    title: String,
    current_dir: PathBuf,
    entries: Vec<DirEntry>,
    visible_indices: Vec<usize>,
    tree_prefixes: Vec<String>,
    /// Cursor position (index into visible_indices)
    cursor: usize,
    scroll_offset: usize,
    button_focused: bool,
    selected_button: usize,
    last_list_area: Option<Rect>,
    last_buttons_area: Option<Rect>,
    create_label: String,
    cancel_label: String,
    cached_width: Option<u16>,
}

const MAX_VISIBLE_ITEMS: usize = 15;

impl DirectoryPickerModal {
    /// Create a new directory picker modal with custom title and confirm button
    pub fn new(initial_dir: PathBuf, title: String, confirm_label: String) -> Self {
        let t = termide_i18n::t();
        let mut modal = Self {
            title,
            current_dir: initial_dir,
            entries: Vec::new(),
            visible_indices: Vec::new(),
            tree_prefixes: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            button_focused: false,
            selected_button: 0,
            last_list_area: None,
            last_buttons_area: None,
            create_label: confirm_label,
            cancel_label: t.directory_picker_cancel().to_string(),
            cached_width: None,
        };
        modal.load_root();
        modal
    }

    /// Get the currently selected path
    fn selected_path(&self) -> PathBuf {
        if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
            let entry = &self.entries[tree_idx];
            if entry.expanded.is_none() {
                // ".." entry — return current dir (going up = selecting parent)
                self.current_dir.clone()
            } else {
                entry.full_path.clone()
            }
        } else {
            self.current_dir.clone()
        }
    }

    /// Load root directory entries (top-level)
    fn load_root(&mut self) {
        self.entries.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.cached_width = None;

        // Add ".." entry if not at filesystem root
        if self.current_dir.parent().is_some() {
            self.entries.push(DirEntry {
                name: "..".to_string(),
                full_path: self.current_dir.clone(),
                depth: 0,
                expanded: None,
            });
        }

        // Read subdirectories
        self.load_children_into(&self.current_dir.clone(), 0, self.entries.len());
        self.recompute_visible();
    }

    /// Read child directories and insert into entries at given position
    fn load_children_into(&mut self, dir: &PathBuf, depth: usize, insert_at: usize) {
        let mut dirs: Vec<DirEntry> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                dirs.push(DirEntry {
                    name,
                    full_path: path,
                    depth,
                    expanded: Some(false),
                });
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Insert children at position (after parent)
        for (i, d) in dirs.into_iter().enumerate() {
            self.entries.insert(insert_at + i, d);
        }
    }

    /// Expand directory at visible index
    fn expand_dir(&mut self, vis_idx: usize) {
        let Some(&tree_idx) = self.visible_indices.get(vis_idx) else {
            return;
        };
        if self.entries[tree_idx].expanded != Some(false) {
            return;
        }

        let dir_path = self.entries[tree_idx].full_path.clone();
        let depth = self.entries[tree_idx].depth;

        self.entries[tree_idx].expanded = Some(true);

        // Check if children already loaded
        let has_children = self
            .entries
            .get(tree_idx + 1)
            .is_some_and(|e| e.depth > depth);

        if !has_children {
            self.load_children_into(&dir_path, depth + 1, tree_idx + 1);
        }

        self.recompute_visible();
    }

    /// Collapse directory at visible index
    fn collapse_dir(&mut self, vis_idx: usize) {
        let Some(&tree_idx) = self.visible_indices.get(vis_idx) else {
            return;
        };
        if self.entries[tree_idx].expanded != Some(true) {
            return;
        }

        self.entries[tree_idx].expanded = Some(false);
        self.recompute_visible();
    }

    /// Jump cursor to parent directory in tree (for Left on non-expanded item)
    fn jump_to_parent(&mut self) {
        let Some(&tree_idx) = self.visible_indices.get(self.cursor) else {
            return;
        };
        let depth = self.entries[tree_idx].depth;
        if depth == 0 {
            return;
        }

        // Search backwards in visible for entry with depth - 1
        for (vi, &ti) in self.visible_indices.iter().enumerate().rev() {
            if self.entries[ti].depth < depth {
                self.cursor = vi;
                self.adjust_scroll();
                return;
            }
        }
    }

    /// Recompute visible indices and tree prefixes
    fn recompute_visible(&mut self) {
        self.visible_indices = compute_visible(&self.entries);
        self.tree_prefixes = compute_prefixes(&self.entries, &self.visible_indices);
    }

    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 4;

        let max_entry_display = self
            .entries
            .iter()
            .map(|e| e.depth * 3 + e.name.len() + 4) // prefix + icon + name
            .max()
            .unwrap_or(0);
        let path_width = self.current_dir.to_string_lossy().len() as u16 + 5;

        let buttons_width = self.create_label.len() as u16 + self.cancel_label.len() as u16 + 12;

        calculate_modal_width(
            [
                title_width,
                path_width,
                max_entry_display as u16 + 2,
                buttons_width,
            ]
            .into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }

    fn get_modal_width(&mut self, screen_width: u16) -> u16 {
        if let Some(width) = self.cached_width {
            return width;
        }
        let width = self.calculate_modal_width(screen_width);
        self.cached_width = Some(width);
        width
    }

    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    fn cursor_down(&mut self) {
        if self.cursor < self.visible_indices.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    fn cursor_end(&mut self) {
        self.cursor = self.visible_indices.len().saturating_sub(1);
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, self.cursor, MAX_VISIBLE_ITEMS);
    }

    /// Go to parent directory (reload tree root)
    fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.load_root();
        }
    }
}

impl Modal for DirectoryPickerModal {
    type Result = PathBuf;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.get_modal_width(area.width);

        let visible_count = self.visible_indices.len().clamp(3, MAX_VISIBLE_ITEMS);
        let list_height = visible_count as u16;
        let modal_height = 6 + list_height;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Path display (truncate left if too wide)
        let selected = self.selected_path();
        let path_str = selected.to_string_lossy();
        let display_path = termide_ui::path_utils::truncate_left(&path_str, inner.width as usize);
        let path_style = Style::default().fg(theme.fg);
        buf.set_string(inner.x, inner.y, &display_path, path_style);

        // Separator
        let sep_y = inner.y + 1;
        let sep_style = Style::default().fg(theme.accented_bg);
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep_y)].set_char('─').set_style(sep_style);
        }

        // Tree list
        let list_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: list_height,
        };

        let prefix_style = Style::default().fg(theme.disabled);
        let mut list_items: Vec<ListItem> = Vec::new();

        for (vis_pos, &tree_idx) in self
            .visible_indices
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ITEMS)
        {
            let entry = &self.entries[tree_idx];
            let is_selected = vis_pos == self.cursor && !self.button_focused;

            // Tree prefix (├─, └─, etc.)
            let tree_prefix = self
                .tree_prefixes
                .get(vis_pos)
                .map(|s| s.as_str())
                .unwrap_or("");

            // Icon: ".." = special, expanded = ▼, collapsed = ▶
            let icon = if entry.expanded.is_none() {
                " "
            } else if entry.expanded == Some(true) {
                "▼"
            } else {
                "▶"
            };

            // Cursor style: invert fg/bg like file manager
            let (row_style, prefix_row_style) = if is_selected {
                (
                    Style::default()
                        .fg(theme.bg)
                        .bg(theme.fg)
                        .add_modifier(Modifier::BOLD),
                    Style::default()
                        .fg(theme.bg)
                        .bg(theme.fg)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (Style::default().fg(theme.fg), prefix_style)
            };

            let text = format!("{} {}", icon, entry.name);
            let line_width = tree_prefix.width() + text.width();
            let padding = " ".repeat((inner.width as usize).saturating_sub(line_width));

            let line = Line::from(vec![
                Span::styled(tree_prefix, prefix_row_style),
                Span::styled(text, row_style),
                Span::styled(padding, row_style),
            ]);

            list_items.push(ListItem::new(line));
        }

        let list = List::new(list_items).style(Style::default().bg(theme.bg));
        list.render(list_area, buf);
        self.last_list_area = Some(list_area);

        // Separator before buttons
        let sep2_y = list_area.y + list_area.height;
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep2_y)].set_char('─').set_style(sep_style);
        }

        // Buttons
        let buttons_y = sep2_y + 1;
        let buttons_area = Rect {
            x: inner.x,
            y: buttons_y,
            width: inner.width,
            height: 1,
        };
        self.last_buttons_area = Some(buttons_area);

        let create_style = if self.button_focused && self.selected_button == 0 {
            Style::default()
                .fg(theme.bg)
                .bg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        let cancel_style = if self.button_focused && self.selected_button == 1 {
            Style::default()
                .fg(theme.bg)
                .bg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        let create_btn = format!("[ {} ]", self.create_label);
        let cancel_btn = format!("[ {} ]", self.cancel_label);

        let total_btn_width = create_btn.width() + 2 + cancel_btn.width();
        let btn_start_x = inner.x + (inner.width.saturating_sub(total_btn_width as u16)) / 2;

        buf.set_string(btn_start_x, buttons_y, &create_btn, create_style);
        buf.set_string(
            btn_start_x + create_btn.width() as u16 + 2,
            buttons_y,
            &cancel_btn,
            cancel_style,
        );
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Tab | KeyCode::BackTab => {
                self.button_focused = !self.button_focused;
                Ok(None)
            }
            // List navigation
            KeyCode::Up | KeyCode::Char('k') if !self.button_focused => {
                self.cursor_up();
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') if !self.button_focused => {
                self.cursor_down();
                Ok(None)
            }
            KeyCode::Home if !self.button_focused => {
                self.cursor_home();
                Ok(None)
            }
            KeyCode::End if !self.button_focused => {
                self.cursor_end();
                Ok(None)
            }
            // Expand directory
            KeyCode::Right | KeyCode::Char('l') if !self.button_focused => {
                self.expand_dir(self.cursor);
                Ok(None)
            }
            // Collapse directory or jump to parent in tree
            KeyCode::Left | KeyCode::Char('h') if !self.button_focused => {
                if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
                    if self.entries[tree_idx].expanded == Some(true) {
                        self.collapse_dir(self.cursor);
                    } else if self.entries[tree_idx].depth > 0 {
                        self.jump_to_parent();
                    }
                }
                Ok(None)
            }
            // Button navigation
            KeyCode::Left if self.button_focused => {
                self.selected_button = 0;
                Ok(None)
            }
            KeyCode::Right if self.button_focused => {
                self.selected_button = 1;
                Ok(None)
            }
            // Go to parent directory (reload tree root)
            KeyCode::Backspace if !self.button_focused => {
                self.go_parent();
                Ok(None)
            }
            // Ctrl+Enter to confirm from anywhere
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Ok(Some(ModalResult::Confirmed(self.selected_path())))
            }
            KeyCode::Enter => {
                if self.button_focused {
                    match self.selected_button {
                        0 => Ok(Some(ModalResult::Confirmed(self.selected_path()))),
                        _ => Ok(Some(ModalResult::Cancelled)),
                    }
                } else {
                    // Enter = cd into directory (reload tree from selected dir)
                    if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
                        if self.entries[tree_idx].expanded.is_none() {
                            // ".." — go to parent
                            self.go_parent();
                        } else {
                            // Navigate into: make selected dir the new root
                            self.current_dir = self.entries[tree_idx].full_path.clone();
                            self.load_root();
                        }
                    }
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crate::{check_mouse_click, MouseClickResult};

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                for _ in 0..3 {
                    self.cursor_up();
                }
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.cursor_down();
                }
                return Ok(None);
            }
            _ => {}
        }

        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        // Check buttons
        if let Some(buttons_area) = self.last_buttons_area {
            if mouse.row == buttons_area.y
                && mouse.column >= buttons_area.x
                && mouse.column < buttons_area.x + buttons_area.width
            {
                let create_btn_width = self.create_label.len() + 4;
                let total_btn_width = create_btn_width + 2 + self.cancel_label.len() + 4;
                let btn_start_x = buttons_area.x
                    + (buttons_area.width.saturating_sub(total_btn_width as u16)) / 2;

                if mouse.column >= btn_start_x
                    && mouse.column < btn_start_x + create_btn_width as u16
                {
                    return Ok(Some(ModalResult::Confirmed(self.selected_path())));
                } else if mouse.column >= btn_start_x + create_btn_width as u16 + 2 {
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
        }

        // Check list click
        match check_mouse_click(
            mouse.column,
            mouse.row,
            None,
            self.last_list_area,
            self.scroll_offset,
        ) {
            MouseClickResult::OnListItem(clicked_vis_idx) => {
                if clicked_vis_idx < self.visible_indices.len() {
                    self.button_focused = false;
                    if self.cursor == clicked_vis_idx {
                        // Click on already-selected item: toggle expand/collapse
                        if let Some(&tree_idx) = self.visible_indices.get(clicked_vis_idx) {
                            match self.entries[tree_idx].expanded {
                                Some(false) => self.expand_dir(clicked_vis_idx),
                                Some(true) => self.collapse_dir(clicked_vis_idx),
                                None => {
                                    // ".." — go parent
                                    self.go_parent();
                                }
                            }
                        }
                    } else {
                        self.cursor = clicked_vis_idx;
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

// === Tree utilities (same algorithm as panel-file-manager/src/tree.rs) ===

/// Compute indices of visible nodes, skipping children of collapsed directories.
fn compute_visible(entries: &[DirEntry]) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut skip_below_depth: Option<usize> = None;

    for (i, entry) in entries.iter().enumerate() {
        if let Some(max_depth) = skip_below_depth {
            if entry.depth > max_depth {
                continue;
            }
            skip_below_depth = None;
        }

        visible.push(i);

        if entry.expanded == Some(false) {
            skip_below_depth = Some(entry.depth);
        }
    }

    visible
}

/// Compute tree-drawing prefixes for visible nodes.
fn compute_prefixes(entries: &[DirEntry], visible: &[usize]) -> Vec<String> {
    if visible.is_empty() {
        return Vec::new();
    }

    let max_depth = visible
        .iter()
        .map(|&idx| entries[idx].depth)
        .max()
        .unwrap_or(0);

    if max_depth == 0 {
        return vec![String::new(); visible.len()];
    }

    let mut has_next_at_level = vec![false; max_depth + 1];
    let mut prefixes: Vec<String> = Vec::with_capacity(visible.len());

    for &tree_idx in visible.iter().rev() {
        let depth = entries[tree_idx].depth;

        if depth == 0 {
            has_next_at_level.fill(false);
            has_next_at_level[0] = true;
            prefixes.push(String::new());
            continue;
        }

        let mut prefix = String::with_capacity(depth * 3);
        for (lvl, has_next) in has_next_at_level[1..=depth].iter().enumerate() {
            let lvl = lvl + 1;
            if lvl == depth {
                if *has_next {
                    prefix.push_str(" ├─");
                } else {
                    prefix.push_str(" └─");
                }
            } else if *has_next {
                prefix.push_str(" │ ");
            } else {
                prefix.push_str("   ");
            }
        }
        prefixes.push(prefix);

        for val in &mut has_next_at_level[(depth + 1)..] {
            *val = false;
        }
        has_next_at_level[depth] = true;
    }

    prefixes.reverse();
    prefixes
}
