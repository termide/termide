use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::git::GitStatus;
use crate::theme::Theme;
use super::{FileEntry, FileManager, utils};

impl FileManager {
    /// Get display title with path
    /// Truncates path from left if it doesn't fit in available width
    pub(super) fn get_display_title(&self, available_width: u16, can_close: bool) -> String {
        let path_str = self.current_path.display().to_string();
        let overhead = if can_close { 10 } else { 7 };
        let max_path_len = available_width.saturating_sub(overhead) as usize;
        let char_count = path_str.chars().count();

        if char_count <= max_path_len {
            path_str
        } else {
            let ellipsis = "...";
            let ellipsis_len = 3;
            let take_chars = max_path_len.saturating_sub(ellipsis_len);
            let trimmed: String = path_str.chars().rev().take(take_chars).collect::<Vec<_>>().into_iter().rev().collect();
            format!("{}{}", ellipsis, trimmed)
        }
    }

    /// Get list of lines for display
    pub(super) fn get_items(&self, height: usize, available_width: usize, theme: &Theme, is_focused: bool) -> Vec<Line> {
        let mut lines = Vec::new();
        let visible_start = self.scroll_offset;
        let visible_end = visible_start + height;

        // Constants for extended mode
        const SIZE_COLUMN_WIDTH: usize = 10;
        const TIME_COLUMN_WIDTH: usize = 19;
        const SEPARATOR: &str = " │ ";
        const SEPARATOR_WIDTH: usize = 3;

        // Determine whether to show extended view with columns
        let show_extended = available_width >= crate::constants::MIN_WIDTH_FOR_EXTENDED_VIEW;

        for (i, entry) in self.entries.iter().enumerate() {
            if i < visible_start || i >= visible_end {
                continue;
            }

            let is_selected = self.selected_items.contains(&i);
            let is_cursor = i == self.selected;

            let icon = if is_selected {
                utils::normalize_icon("✓")
            } else {
                utils::normalize_icon(utils::get_icon(entry))
            };
            let icon_width = icon.width();
            let dir_prefix = if entry.is_dir && entry.name != ".." { "/" } else { "" };
            let prefix_width = dir_prefix.width();

            // Calculate maximum visual width of name WITHOUT prefix, considering display mode
            let max_name_len = if show_extended {
                // For wide mode: subtract icon, space, prefix, two columns and two separators
                available_width.saturating_sub(icon_width + 1 + prefix_width + SEPARATOR_WIDTH + SIZE_COLUMN_WIDTH + SEPARATOR_WIDTH + TIME_COLUMN_WIDTH)
            } else {
                // For normal mode: only icon, space and prefix
                available_width.saturating_sub(icon_width + 1 + prefix_width)
            };

            let name = utils::truncate_name(&entry.name, max_name_len);
            let name_width = name.width();
            let full_name = format!("{}{}", dir_prefix, name);

            let (bg_style, fg_style) = if is_cursor && is_focused {
                // Show cursor only when panel is focused
                let bg = Style::default().bg(theme.selection_bg).fg(theme.selection_fg).add_modifier(Modifier::BOLD);
                (bg, bg)
            } else {
                let fg_color = match entry.git_status {
                    GitStatus::Ignored => theme.text_secondary,
                    GitStatus::Modified => theme.git_modified,
                    GitStatus::Added => theme.git_added,
                    GitStatus::Deleted => theme.git_deleted,
                    GitStatus::Unmodified => {
                        if entry.is_hidden { theme.text_secondary } else { theme.text_primary }
                    }
                };
                (Style::default(), Style::default().fg(fg_color))
            };

            let icon_style = if is_selected {
                Style::default().fg(theme.selected_item).add_modifier(Modifier::BOLD)
            } else {
                fg_style
            };

            if show_extended {
                // Extended mode with columns
                // Use name_width without prefix, since max_name_len already accounted for prefix_width when subtracting
                let padding_len = max_name_len.saturating_sub(name_width);
                let padding = " ".repeat(padding_len);

                // Format size (or spaces for directories and "..")
                let size_str = if let Some(size) = entry.size {
                    format!("{:<10}", utils::format_size(size))
                } else {
                    "          ".to_string()
                };

                // Format time
                let time_str = utils::format_modified_time(entry.modified);

                lines.push(Line::from(vec![
                    Span::styled(icon, icon_style),
                    Span::styled(" ", bg_style),
                    Span::styled(full_name, fg_style),
                    Span::styled(padding, bg_style),
                    Span::styled(SEPARATOR, bg_style.fg(theme.accent_secondary)),
                    Span::styled(size_str, fg_style),
                    Span::styled(SEPARATOR, bg_style.fg(theme.accent_secondary)),
                    Span::styled(time_str, fg_style),
                ]));
            } else {
                // Normal mode without columns
                // Use name_width + prefix_width instead of full_name.width() for consistency
                let content_width = icon_width + 1 + prefix_width + name_width;
                let padding_len = available_width.saturating_sub(content_width);
                let padding = " ".repeat(padding_len);

                lines.push(Line::from(vec![
                    Span::styled(icon, icon_style),
                    Span::styled(" ", bg_style),
                    Span::styled(full_name, fg_style),
                    Span::styled(padding, bg_style),
                ]));
            }
        }

        lines
    }
}
