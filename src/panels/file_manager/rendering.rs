use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{utils, FileManager};
use crate::git::GitStatus;
use crate::state::AppState;
use crate::theme::Theme;

impl FileManager {
    /// Get display title with path
    /// Truncates path from left if it doesn't fit in available width
    pub(super) fn get_display_title(&self, available_width: u16) -> String {
        let path_str = self.current_path.display().to_string();
        // Overhead for borders and padding (no [X] button for FileManager)
        let overhead = 7;
        let max_path_len = available_width.saturating_sub(overhead) as usize;
        let char_count = path_str.chars().count();

        if char_count <= max_path_len {
            path_str
        } else {
            let ellipsis = "...";
            let ellipsis_len = 3;
            let take_chars = max_path_len.saturating_sub(ellipsis_len);
            let trimmed: String = path_str
                .chars()
                .rev()
                .take(take_chars)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{}{}", ellipsis, trimmed)
        }
    }

    /// Get list of lines for display
    pub(super) fn get_items(
        &self,
        height: usize,
        available_width: usize,
        theme: &Theme,
        is_focused: bool,
        state: &AppState,
    ) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let visible_start = self.scroll_offset;
        let visible_end = visible_start + height;

        // Constants for extended mode
        const SIZE_COLUMN_WIDTH: usize = 10;
        const TIME_COLUMN_WIDTH: usize = 19;
        const SEPARATOR: &str = " │ ";
        const SEPARATOR_WIDTH: usize = 3;

        // Determine whether to show extended view with columns
        let show_extended = available_width >= state.config.file_manager.extended_view_width;

        for (i, entry) in self.entries.iter().enumerate() {
            if i < visible_start || i >= visible_end {
                continue;
            }

            let is_selected = self.selected_items.contains(&i);
            let is_cursor = i == self.selected;

            let attr = utils::get_attribute(entry, is_selected);
            let icon = utils::get_icon(entry);
            let attr_width = 1; // always 1 character
            let icon_width = 1; // always 1 character
            let dir_prefix = if entry.is_dir && entry.name != ".." {
                "/"
            } else {
                ""
            };
            let prefix_width = dir_prefix.width();

            // Calculate maximum visual width of name WITHOUT prefix, considering display mode
            let max_name_len = if show_extended {
                // For wide mode: attr + icon + space + prefix + two columns and two separators
                available_width.saturating_sub(
                    attr_width
                        + icon_width
                        + 1
                        + prefix_width
                        + SEPARATOR_WIDTH
                        + SIZE_COLUMN_WIDTH
                        + SEPARATOR_WIDTH
                        + TIME_COLUMN_WIDTH,
                )
            } else {
                // For normal mode: attr + icon + space + prefix
                available_width.saturating_sub(attr_width + icon_width + 1 + prefix_width)
            };

            let name = utils::truncate_name(&entry.name, max_name_len);
            let name_width = name.width();
            let full_name = format!("{}{}", dir_prefix, name);

            let (bg_style, fg_style) = if is_cursor && is_focused {
                // Вычислить нормальный fg_style для этой строки
                let normal_fg_style = match entry.git_status {
                    GitStatus::Ignored => Style::default()
                        .fg(theme.disabled)
                        .add_modifier(Modifier::DIM),
                    GitStatus::Modified => Style::default().fg(theme.warning),
                    GitStatus::Added => Style::default().fg(theme.success),
                    GitStatus::Deleted => Style::default().fg(theme.error),
                    GitStatus::Unmodified => Style::default().fg(theme.fg),
                };

                // Извлечь fg цвет и создать инверсный стиль курсора
                let fg_color = normal_fg_style.fg.unwrap_or(theme.fg);
                let cursor_style = Style::default()
                    .bg(fg_color) // Swap: fg строки становится bg курсора
                    .fg(theme.bg) // Swap: theme bg становится fg курсора
                    .add_modifier(Modifier::BOLD);

                (cursor_style, cursor_style)
            } else {
                let fg_style = match entry.git_status {
                    GitStatus::Ignored => Style::default()
                        .fg(theme.disabled)
                        .add_modifier(Modifier::DIM),
                    GitStatus::Modified => Style::default().fg(theme.warning),
                    GitStatus::Added => Style::default().fg(theme.success),
                    GitStatus::Deleted => Style::default().fg(theme.error),
                    GitStatus::Unmodified => Style::default().fg(theme.fg),
                };
                (Style::default(), fg_style)
            };

            let attr_style = if is_selected {
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                fg_style
            };

            // Переопределить fg_style для выделенных файлов (если не курсор)
            let fg_style = if is_selected && !(is_cursor && is_focused) {
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                fg_style
            };

            let icon_style = fg_style;

            if show_extended {
                // Extended mode with columns
                // Use name_width without prefix, since max_name_len already accounted for prefix_width when subtracting
                let padding_len = max_name_len.saturating_sub(name_width);
                let padding = " ".repeat(padding_len);

                // Format size (or spaces for directories and ".."), right-aligned
                let size_str = if let Some(size) = entry.size {
                    format!("{:>10}", utils::format_size(size))
                } else {
                    "          ".to_string()
                };

                // Format time
                let time_str = utils::format_modified_time(entry.modified);

                lines.push(Line::from(vec![
                    Span::styled(attr, attr_style),
                    Span::styled(icon, icon_style),
                    Span::styled(" ", bg_style),
                    Span::styled(full_name, fg_style),
                    Span::styled(padding, bg_style),
                    Span::styled(SEPARATOR, bg_style.fg(theme.disabled)),
                    Span::styled(size_str, fg_style),
                    Span::styled(SEPARATOR, bg_style.fg(theme.disabled)),
                    Span::styled(time_str, fg_style),
                ]));
            } else {
                // Normal mode without columns
                let content_width = attr_width + icon_width + 1 + prefix_width + name_width;
                let padding_len = available_width.saturating_sub(content_width);
                let padding = " ".repeat(padding_len);

                lines.push(Line::from(vec![
                    Span::styled(attr, attr_style),
                    Span::styled(icon, icon_style),
                    Span::styled(" ", bg_style),
                    Span::styled(full_name, fg_style),
                    Span::styled(padding, bg_style),
                ]));
            }
        }

        // Fill remaining space with empty lines (with separators in extended mode)
        if show_extended && lines.len() < height {
            let name_column_width = available_width.saturating_sub(
                SEPARATOR_WIDTH + SIZE_COLUMN_WIDTH + SEPARATOR_WIDTH + TIME_COLUMN_WIDTH,
            );
            let empty_name = " ".repeat(name_column_width);
            let empty_size = " ".repeat(SIZE_COLUMN_WIDTH);
            let empty_time = " ".repeat(TIME_COLUMN_WIDTH);
            let separator_style = Style::default().fg(theme.disabled);

            for _ in lines.len()..height {
                lines.push(Line::from(vec![
                    Span::raw(empty_name.clone()),
                    Span::styled(SEPARATOR, separator_style),
                    Span::raw(empty_size.clone()),
                    Span::styled(SEPARATOR, separator_style),
                    Span::raw(empty_time.clone()),
                ]));
            }
        }

        lines
    }
}
