//! Rendering functions for Git Status Panel.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;
use termide_git::{self as git, truncate_left};
use termide_ui::path_utils::truncate_to_width_str;
use termide_ui::ScrollBar;
use termide_ui_render::{render_simple_dropdown, InlineSelector};

use crate::types::Section;
use crate::GitStatusPanel;

/// Calculate dropdown x and width so that the full item text is visible.
/// Tries to expand rightward first (up to `max_right`), then leftward
/// (clamped to the screen edge). Falls back to the available space if neither
/// direction has enough room.
fn expand_dropdown(x: u16, max_width: u16, max_right: u16, items: &[String]) -> (u16, u16) {
    let max_item_width = items.iter().map(|s| s.width()).max().unwrap_or(10);
    // +4 for borders (2) + inner padding (2)
    let needed = (max_item_width + 4) as u16;
    if needed <= max_width {
        (x, max_width)
    } else {
        let extra = needed.saturating_sub(max_width);
        // Try expanding rightward first
        let space_right = max_right.saturating_sub(x + max_width);
        let right_extra = extra.min(space_right);
        let mut new_x = x;
        let mut new_width = max_width + right_extra;
        // Expand leftward with remaining extra
        let remaining = extra.saturating_sub(right_extra);
        if remaining > 0 {
            new_x = x.saturating_sub(remaining);
            new_width = (x + max_width).saturating_sub(new_x);
        }
        (new_x, new_width)
    }
}

/// Render a selector dropdown with a filter input row on top, shared by the
/// repo and branch selectors.
///
/// The caller supplies the pre-filtered display `items`, the current `filter`
/// text, the highlighted `cursor` position, and `selected_pos` — the index
/// within `items` that is the active selection (if it is in the filtered set).
/// Returns the dropdown's screen `Rect` (for click hit-testing) and the applied
/// vertical scroll offset.
#[allow(clippy::too_many_arguments)]
fn render_filtered_dropdown(
    buf: &mut Buffer,
    theme: &ThemeColors,
    anchor_x: u16,
    max_width: u16,
    max_right: u16,
    dropdown_y: u16,
    max_dropdown_height: usize,
    items: &[String],
    filter: &str,
    cursor: usize,
    selected_pos: Option<usize>,
) -> (Rect, usize) {
    // Reserve 1 row for filter input, 1 for separator.
    let filter_rows: u16 = 2;
    let list_max_height = max_dropdown_height.saturating_sub(filter_rows as usize);
    let visible_count = items.len().min(list_max_height);
    let scroll_offset = if cursor >= visible_count {
        cursor - visible_count + 1
    } else {
        0
    };

    let total_height = (visible_count as u16) + filter_rows + 2; // +2 for borders
    let (dropdown_x, dropdown_width) = expand_dropdown(anchor_x, max_width, max_right, items);
    let area = Rect {
        x: dropdown_x,
        y: dropdown_y,
        width: dropdown_width,
        height: total_height,
    };

    // Draw border and background
    let border_style = Style::default().fg(theme.border_focused);
    let bg_style = Style::default().bg(theme.bg);
    for dy in 0..total_height {
        for dx in 0..dropdown_width {
            let cell = &mut buf[(dropdown_x + dx, dropdown_y + dy)];
            cell.set_style(bg_style);
            if dy == 0 || dy == total_height - 1 {
                if dx == 0 {
                    cell.set_symbol(if dy == 0 { "┌" } else { "└" });
                } else if dx == dropdown_width - 1 {
                    cell.set_symbol(if dy == 0 { "┐" } else { "┘" });
                } else {
                    cell.set_symbol("─");
                }
                cell.set_style(border_style);
            } else if dx == 0 || dx == dropdown_width - 1 {
                cell.set_symbol("│").set_style(border_style);
            } else {
                cell.set_symbol(" ");
            }
        }
    }

    let inner_width = dropdown_width.saturating_sub(2) as usize;

    // Filter row (row 1 inside border)
    let filter_text = format!(" Filter: {}", filter);
    let padding_len = inner_width.saturating_sub(filter_text.width() + 1);
    let filter_style = Style::default().fg(theme.fg);
    let cursor_style = Style::default()
        .fg(theme.bg)
        .bg(theme.fg)
        .add_modifier(Modifier::BOLD);
    let filter_line = Line::from(vec![
        Span::styled(filter_text, filter_style),
        Span::styled("█", cursor_style),
        Span::styled(" ".repeat(padding_len), filter_style),
    ]);
    ratatui::widgets::Paragraph::new(filter_line).render(
        Rect {
            x: dropdown_x + 1,
            y: dropdown_y + 1,
            width: inner_width as u16,
            height: 1,
        },
        buf,
    );

    // Separator row (row 2 inside border)
    let sep_y = dropdown_y + 2;
    for dx in 1..dropdown_width - 1 {
        buf[(dropdown_x + dx, sep_y)]
            .set_symbol("─")
            .set_style(Style::default().fg(theme.border));
    }

    // Item list (below separator)
    let list_y = dropdown_y + filter_rows + 1;
    for (i, item) in items
        .iter()
        .skip(scroll_offset)
        .take(visible_count)
        .enumerate()
    {
        let item_y = list_y + i as u16;
        let item_idx = scroll_offset + i;
        let style = if item_idx == cursor {
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .remove_modifier(Modifier::all())
        } else if selected_pos == Some(item_idx) {
            Style::default()
                .fg(theme.cursor)
                .remove_modifier(Modifier::all())
        } else {
            Style::default()
                .fg(theme.fg)
                .remove_modifier(Modifier::all())
        };

        // Clear line and draw item
        for dx in 1..dropdown_width - 1 {
            buf[(dropdown_x + dx, item_y)]
                .set_symbol(" ")
                .set_style(style);
        }
        buf.set_string(
            dropdown_x + 1,
            item_y,
            truncate_to_width_str(item, inner_width),
            style,
        );
    }

    // Scrollbar
    if visible_count < items.len() {
        ScrollBar::render(
            buf,
            dropdown_x + dropdown_width - 1,
            list_y,
            visible_count as u16,
            scroll_offset,
            visible_count,
            items.len(),
            theme,
            true,
        );
    }

    (area, scroll_offset)
}

impl GitStatusPanel {
    /// Render the main content area
    pub(crate) fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        border_right_x: Option<u16>,
    ) {
        if area.height < 5 {
            return;
        }

        let theme = self.cached_theme;
        let content_area = area;

        // Layout constants
        let selector_height: u16 = 1;
        let separator_height: u16 = 1;
        let buttons_height = self.calc_buttons_height(content_area.width);
        let fixed_height = selector_height + separator_height + buttons_height;
        let files_area_height = content_area.height.saturating_sub(fixed_height) as usize;

        // Cache viewport height for scroll calculations
        self.viewport_height = files_area_height;

        // Virtual content layout
        let unstaged_header_line = 0;
        let unstaged_files_start = 1;
        let unstaged_files_end = unstaged_files_start + self.unstaged_item_count();
        let staged_header_line = unstaged_files_end;
        let staged_files_start = staged_header_line + 1;
        let total_virtual_lines = self.total_virtual_lines();

        // Clamp scroll offset
        let max_scroll = total_virtual_lines.saturating_sub(files_area_height);
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }

        let mut y = content_area.y;

        // === TOP ZONE: Selectors ===
        self.selector_y = y;

        let t = termide_i18n::t();

        let repo_name = self
            .repo_manager
            .current()
            .map(git::get_repo_name)
            .unwrap_or_else(|| t.git_no_repo().to_string());
        let repo_focused = self.current_section == Section::RepoSelector && is_focused;
        let repo_selector =
            InlineSelector::new(&repo_name, self.repo_dropdown_open, repo_focused, &theme);
        let repo_width = repo_selector.render(content_area.x, y, content_area.width / 2, buf);

        let branch_name = self
            .branch
            .clone()
            .unwrap_or_else(|| t.git_branch_detached().to_string());
        let branch_focused = self.current_section == Section::BranchSelector && is_focused;
        let branch_x = content_area.x + repo_width + 2;
        self.branch_selector_x = branch_x;
        let branch_max_width = content_area.width.saturating_sub(repo_width + 2);
        let branch_selector = InlineSelector::new(
            &branch_name,
            self.branch_dropdown_open,
            branch_focused,
            &theme,
        );
        branch_selector.render(branch_x, y, branch_max_width, buf);

        y += selector_height;

        // === MIDDLE ZONE: Files area (unified scroll) ===
        let files_y = y;
        let files_width = content_area.width;

        // Store files area for mouse handling
        self.files_area = Rect {
            x: content_area.x,
            y: files_y,
            width: files_width,
            height: files_area_height as u16,
        };

        let files_active = self.current_section == Section::Files && is_focused;

        // Render visible virtual lines
        for screen_row in 0..files_area_height {
            let vline = self.scroll_offset + screen_row;
            if vline >= total_virtual_lines {
                break;
            }
            let line_y = files_y + screen_row as u16;

            if vline == unstaged_header_line {
                self.render_unstaged_header(
                    self.cursor == vline && files_active,
                    content_area.x,
                    line_y,
                    files_width,
                    buf,
                    &theme,
                );
            } else if vline >= unstaged_files_start && vline < unstaged_files_end {
                let item_idx = vline - unstaged_files_start;
                let is_selected = self.cursor == vline && files_active;
                self.render_tree_node_line(
                    true,
                    item_idx,
                    is_selected,
                    content_area.x,
                    line_y,
                    files_width,
                    buf,
                    &theme,
                    files_active,
                );
            } else if vline == staged_header_line {
                self.render_staged_header(
                    self.cursor == vline && files_active,
                    content_area.x,
                    line_y,
                    files_width,
                    buf,
                    &theme,
                );
            } else if vline >= staged_files_start {
                let item_idx = vline - staged_files_start;
                let is_selected = self.cursor == vline && files_active;
                self.render_tree_node_line(
                    false,
                    item_idx,
                    is_selected,
                    content_area.x,
                    line_y,
                    files_width,
                    buf,
                    &theme,
                    files_active,
                );
            }
        }

        // Single scrollbar for entire files area
        self.scrollbars.vertical = None;
        if let Some(border_x) = border_right_x {
            self.scrollbars.vertical = ScrollBar::render_tracked(
                buf,
                border_x,
                files_y,
                files_area_height as u16,
                self.scroll_offset,
                files_area_height,
                total_virtual_lines,
                &theme,
                files_active,
            );
        }

        // === STICKY HEADERS ===
        // When a section header scrolls out of view, render it at the top of files area
        // so user always knows which section they're viewing

        // Staged header is sticky if we've scrolled past it (into staged files only)
        let staged_sticky =
            self.scroll_offset > staged_header_line && !self.staged_files.is_empty();

        // Unstaged header is sticky if scrolled past line 0, but NOT if staged is sticky
        let unstaged_sticky = self.scroll_offset > unstaged_header_line
            && !self.unstaged_files.is_empty()
            && !staged_sticky;

        if unstaged_sticky {
            self.render_unstaged_header(
                self.cursor == unstaged_header_line && files_active,
                content_area.x,
                files_y,
                files_width,
                buf,
                &theme,
            );
        }

        if staged_sticky {
            self.render_staged_header(
                self.cursor == staged_header_line && files_active,
                content_area.x,
                files_y,
                files_width,
                buf,
                &theme,
            );
        }

        y += files_area_height as u16;

        // Separator before buttons
        self.render_horizontal_line(content_area.x, y, content_area.width, buf, &theme);
        y += separator_height;

        // === BOTTOM ZONE: Buttons ===
        self.buttons_y = y;
        self.cached_buttons_height = buttons_height;
        self.render_buttons(
            content_area.x,
            y,
            content_area.width,
            buf,
            &theme,
            is_focused,
        );

        // === DROPDOWNS (rendered last to overlay) ===
        if self.repo_dropdown_open {
            let dropdown_y = content_area.y + 1;
            let max_dropdown_height = content_area.height.saturating_sub(3) as usize;
            let repo_names: Vec<String> = self
                .repo_manager
                .repos()
                .iter()
                .map(|p| git::get_repo_name(p))
                .collect();

            if self.show_repo_filter {
                // Filtered dropdown: render filter row + filtered repo list
                let filtered_indices = self.filtered_repo_indices();
                let filtered_repos: Vec<String> = filtered_indices
                    .iter()
                    .map(|&i| repo_names[i].clone())
                    .collect();
                let selected_idx = self.repo_manager.selected_index();
                let selected_pos = filtered_indices.iter().position(|&i| i == selected_idx);
                let (area, scroll_offset) = render_filtered_dropdown(
                    buf,
                    &theme,
                    content_area.x,
                    content_area.width / 2,
                    content_area.x + content_area.width,
                    dropdown_y,
                    max_dropdown_height,
                    &filtered_repos,
                    &self.repo_filter,
                    self.dropdown_cursor,
                    selected_pos,
                );
                self.dropdown_scroll = scroll_offset;
                self.repo_dropdown_area = Some(area);
            } else {
                // Normal dropdown (no filter)
                let visible_count = repo_names.len().min(max_dropdown_height);
                let scroll_offset = if self.dropdown_cursor >= visible_count {
                    self.dropdown_cursor - visible_count + 1
                } else {
                    0
                };
                self.dropdown_scroll = scroll_offset;
                let (dropdown_x, dropdown_width) = expand_dropdown(
                    content_area.x,
                    content_area.width / 2,
                    content_area.x + content_area.width,
                    &repo_names,
                );
                self.repo_dropdown_area = Some(Rect {
                    x: dropdown_x,
                    y: dropdown_y,
                    width: dropdown_width,
                    height: visible_count as u16 + 2,
                });
                render_simple_dropdown(
                    &repo_names,
                    self.repo_manager.selected_index(),
                    self.dropdown_cursor,
                    dropdown_x,
                    dropdown_y,
                    dropdown_width,
                    max_dropdown_height as u16,
                    buf,
                    &theme,
                );
            }
        } else {
            self.repo_dropdown_area = None;
        }
        if self.branch_dropdown_open {
            let dropdown_y = content_area.y + 1;
            let max_dropdown_height = content_area.height.saturating_sub(3) as usize;

            if self.show_branch_filter {
                // Filtered dropdown: render filter row + filtered branch list
                let filtered_indices = self.filtered_branch_indices();
                let filtered_branches: Vec<String> = filtered_indices
                    .iter()
                    .map(|&i| self.branches[i].clone())
                    .collect();
                let current_branch_idx = self
                    .branches
                    .iter()
                    .position(|b| Some(b.as_str()) == self.branch.as_deref())
                    .unwrap_or(0);
                let selected_pos = filtered_indices
                    .iter()
                    .position(|&i| i == current_branch_idx);
                let (area, scroll_offset) = render_filtered_dropdown(
                    buf,
                    &theme,
                    branch_x,
                    branch_max_width,
                    content_area.x + content_area.width,
                    dropdown_y,
                    max_dropdown_height,
                    &filtered_branches,
                    &self.branch_filter,
                    self.dropdown_cursor,
                    selected_pos,
                );
                self.dropdown_scroll = scroll_offset;
                self.branch_dropdown_area = Some(area);
            } else {
                // Normal dropdown (no filter)
                let current_branch_idx = self
                    .branches
                    .iter()
                    .position(|b| Some(b.as_str()) == self.branch.as_deref())
                    .unwrap_or(0);
                let visible_count = self.branches.len().min(max_dropdown_height);
                let scroll_offset = if self.dropdown_cursor >= visible_count {
                    self.dropdown_cursor - visible_count + 1
                } else {
                    0
                };
                self.dropdown_scroll = scroll_offset;
                let (dropdown_x, dropdown_width) = expand_dropdown(
                    branch_x,
                    branch_max_width,
                    content_area.x + content_area.width,
                    &self.branches,
                );
                self.branch_dropdown_area = Some(Rect {
                    x: dropdown_x,
                    y: dropdown_y,
                    width: dropdown_width,
                    height: visible_count as u16 + 2,
                });
                render_simple_dropdown(
                    &self.branches,
                    current_branch_idx,
                    self.dropdown_cursor,
                    dropdown_x,
                    dropdown_y,
                    dropdown_width,
                    max_dropdown_height as u16,
                    buf,
                    &theme,
                );
            }
        } else {
            self.branch_dropdown_area = None;
        }
    }

    /// Render section header with optional button selection highlighting
    /// Render the unstaged files section header.
    fn render_unstaged_header(
        &mut self,
        is_selected: bool,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) {
        let t = termide_i18n::t();
        let title = format!(
            "{} ({})",
            t.git_unstaged_header(),
            self.unstaged_files.len()
        );
        self.render_section_header(&title, is_selected, x, y, width, buf, theme);
    }

    /// Render the staged files section header.
    fn render_staged_header(
        &mut self,
        is_selected: bool,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) {
        let t = termide_i18n::t();
        let title = format!("{} ({})", t.git_staged_header(), self.staged_files.len());
        self.render_section_header(&title, is_selected, x, y, width, buf, theme);
    }

    /// Render a section header as a horizontal line with embedded title.
    fn render_section_header(
        &self,
        title: &str,
        _is_selected: bool,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) {
        let header_style = Style::default().fg(theme.disabled);

        let title_with_space = format!(" {} ", title);
        let title_width = title_with_space.width();

        buf.set_string(x, y, "─", header_style);
        buf.set_string(x + 1, y, &title_with_space, header_style);

        let after_title = x + 1 + title_width as u16;
        let remaining = width.saturating_sub(1 + title_width as u16);
        for dx in 0..remaining {
            buf.set_string(after_title + dx, y, "─", header_style);
        }
    }

    /// Get color and modifier for file status
    pub(crate) fn get_file_style(
        status: char,
        untracked: bool,
        theme: &ThemeColors,
    ) -> (Color, Modifier) {
        if untracked {
            (theme.success, Modifier::empty())
        } else {
            match status {
                'M' => (theme.warning, Modifier::empty()),
                'D' => (theme.error, Modifier::CROSSED_OUT),
                'A' | 'R' => (theme.success, Modifier::empty()),
                _ => (theme.fg, Modifier::empty()),
            }
        }
    }

    /// Render a tree node line (directory or file) in tree view mode
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_tree_node_line(
        &self,
        is_unstaged: bool,
        visible_idx: usize,
        is_selected: bool,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
        is_focused: bool,
    ) {
        let ft = if is_unstaged {
            &self.unstaged
        } else {
            &self.staged
        };
        let (tree_nodes, visible, prefixes) = (&ft.tree, &ft.visible, &ft.prefixes);

        let Some(&tree_idx) = visible.get(visible_idx) else {
            return;
        };
        let node = &tree_nodes[tree_idx];
        let prefix = prefixes.get(visible_idx).map(|s| s.as_str()).unwrap_or("");

        // Determine style based on node kind
        let (fg_color, extra_modifier, label) = match node.kind {
            crate::tree::TreeNodeKind::Directory { expanded } => {
                // Aggregate status is precomputed in `recompute_visible`.
                let (status, untracked) = ft
                    .node_status
                    .get(tree_idx)
                    .copied()
                    .unwrap_or(('?', false));
                let (color, _modifier) = Self::get_file_style(status, untracked, theme);
                const DIR_COLLAPSED: &str = if cfg!(windows) { "►" } else { "▶" };
                let arrow = if expanded { "▼" } else { DIR_COLLAPSED };
                (
                    color,
                    Modifier::empty(),
                    format!("{} /{}", arrow, node.label),
                )
            }
            crate::tree::TreeNodeKind::File {
                status, untracked, ..
            } => {
                let (color, modifier) = Self::get_file_style(status, untracked, theme);
                (color, modifier, node.label.clone())
            }
        };

        // Style for the file label (with CROSSED_OUT for deleted files)
        let label_style = if is_selected && is_focused {
            Style::default()
                .fg(theme.bg)
                .bg(fg_color)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg_color).add_modifier(extra_modifier)
        };

        // Style for the tree prefix (no CROSSED_OUT, just color)
        let prefix_style = if is_selected && is_focused {
            label_style // selection style has no CROSSED_OUT
        } else {
            Style::default().fg(fg_color)
        };

        // Fill background when selected
        if is_selected && is_focused {
            for dx in 0..width {
                buf[(x + dx, y)].set_symbol(" ").set_style(label_style);
            }
        }

        // Render prefix and label separately so CROSSED_OUT only applies to file name
        let prefix_part = format!(" {}", prefix);
        let prefix_len = prefix_part.width() as u16;
        buf.set_string(x, y, &prefix_part, prefix_style);

        // File name — with strikethrough if deleted
        let remaining = width.saturating_sub(prefix_len) as usize;
        if remaining > 0 {
            let truncated_label = truncate_left(&label, remaining);
            buf.set_string(x + prefix_len, y, &truncated_label, label_style);
        }
    }

    /// Calculate how many rows the buttons need at the given width.
    pub(crate) fn calc_buttons_height(&self, width: u16) -> u16 {
        let buttons = self.get_visible_buttons();
        if buttons.is_empty() {
            return 1;
        }
        let mut current_x: u16 = 0;
        let mut rows: u16 = 1;
        for button in &buttons {
            let label = format!("[{}]", button.label(self.spinner_frame));
            let w = label.width() as u16;
            if current_x > 0 && current_x + w > width {
                rows += 1;
                current_x = w + 1;
            } else {
                current_x += w + 1;
            }
        }
        rows
    }

    /// Render action buttons
    pub(crate) fn render_buttons(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
        is_focused: bool,
    ) {
        let buttons = self.get_visible_buttons();
        let mut current_x = x;
        let mut current_y = y;
        self.stash_button_area = None;

        for (i, button) in buttons.iter().enumerate() {
            let is_selected = self.current_section == Section::Buttons && i == self.selected_button;
            let label = format!("[{}]", button.label(self.spinner_frame));

            let style = if is_selected && is_focused {
                // Inverted cursor style - only when focused
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let lw = label.width() as u16;
            if current_x > x && current_x + lw > x + width {
                current_y += 1;
                current_x = x;
            }

            // Track stash button area for dropdown anchoring
            if matches!(button, crate::types::Button::Stash(_)) {
                self.stash_button_area = Some(Rect {
                    x: current_x,
                    y: current_y,
                    width: lw,
                    height: 1,
                });
            }

            buf.set_string(current_x, current_y, &label, style);
            current_x += lw + 1;
        }
    }

    /// Render a horizontal line separator
    pub(crate) fn render_horizontal_line(
        &self,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        theme: &ThemeColors,
    ) {
        let style = Style::default().fg(theme.border);
        for i in 0..width {
            buf[(x + i, y)].set_symbol("─").set_style(style);
        }
    }

    /// Check if coordinates are within a rect
    pub(crate) fn is_in_rect(&self, col: u16, row: u16, rect: Rect) -> bool {
        col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
    }
}
