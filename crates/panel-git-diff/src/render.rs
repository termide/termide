//! Diff rendering (file headers, hunks, line numbers, scrollbar) for the Git Diff Panel.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_width::UnicodeWidthStr;

use termide_ui::{diff_line_bg, ScrollBar};

use crate::{FileStatus, GitDiffPanel, LineKind};

impl GitDiffPanel {
    /// Render the panel content
    pub(crate) fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        border_right_x: Option<u16>,
    ) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        let theme = &self.cached_theme;

        let content_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        self.visible_height = content_area.height as usize;

        // Colors for diff - blend theme colors with background for adaptive styling
        let added_bg = diff_line_bg(theme.success, theme.bg);
        let removed_bg = diff_line_bg(theme.error, theme.bg);
        let added_style = Style::default().fg(theme.success).bg(added_bg);
        let removed_style = Style::default().fg(theme.error).bg(removed_bg);
        let context_style = Style::default().fg(theme.fg);
        let hunk_header_style = Style::default().fg(theme.disabled);
        let file_header_style = Style::default().fg(theme.info);
        let file_header_selected_style = Style::default()
            .fg(theme.selection_fg)
            .bg(theme.selection_bg);
        let line_number_style = Style::default().fg(theme.disabled);

        let line_num_width = 4; // Width for each line number column

        // The cursor's row and the selected lines take the selection's
        // background over their own colours.
        let (cursor, selection) = (self.cursor, self.selection());
        let marked = |row: usize| {
            is_focused && (row == cursor || selection.is_some_and(|(a, b)| (a..=b).contains(&row)))
        };
        let mark = |style: Style, row: usize| {
            if marked(row) {
                style.bg(theme.selection_bg)
            } else {
                style
            }
        };

        let mut current_line = 0usize;
        let mut y = content_area.y;
        let max_y = content_area.y + content_area.height;

        for (file_idx, diff) in self.diffs.iter().enumerate() {
            if y >= max_y {
                break;
            }

            // File header line
            if current_line >= self.scroll && y < max_y {
                let is_selected = is_focused && current_line == cursor;
                let is_collapsed = self.collapsed.contains(&file_idx);

                // Use same style for entire line (text + lines) for uniformity
                // Selected: inverted colors for whole line
                let header_style = if is_selected {
                    file_header_selected_style
                } else {
                    file_header_style
                };

                // Build header components
                const SECTION_COLLAPSED: &str = if cfg!(windows) { "[►]" } else { "[▶]" };
                let collapse_btn = if is_collapsed {
                    SECTION_COLLAPSED
                } else {
                    "[▼]"
                };
                let status_char = match diff.status {
                    FileStatus::Added => "+",
                    FileStatus::Deleted => "-",
                    FileStatus::Modified => "~",
                    FileStatus::Renamed => "R",
                    FileStatus::Untracked => "?",
                };
                let stats = format!("(+{} -{})", diff.additions, diff.deletions);
                let t = termide_i18n::t();
                let staged_marker = if diff.staged {
                    format!(" [{}]", t.git_diff_staged_marker())
                } else {
                    String::new()
                };

                // Format: ─[▼] ~ path (+N -M) [staged] ─────────
                let title_text = format!(
                    "{} {} {} {}{}",
                    collapse_btn, status_char, &diff.path, stats, &staged_marker
                );
                let title_width = title_text.width();

                // Render from edge to edge (panel boundaries, no padding)
                let header_x = area.x;
                let header_end = area.x + area.width;
                let mut x = header_x;

                // Leading line ─
                buf.set_string(x, y, "─", header_style);
                x += 1;

                // Title text
                buf.set_string(x, y, &title_text, header_style);
                x += title_width as u16;

                // Space
                buf.set_string(x, y, " ", header_style);
                x += 1;

                // Trailing line ─────────
                while x < header_end {
                    buf.set_string(x, y, "─", header_style);
                    x += 1;
                }

                y += 1;
            }
            current_line += 1;

            // Skip content if collapsed
            if self.collapsed.contains(&file_idx) {
                continue;
            }

            // Render hunks
            for hunk in &diff.hunks {
                if y >= max_y {
                    break;
                }

                // Hunk header
                if current_line >= self.scroll && y < max_y {
                    // Truncate hunk header to fit
                    let header = if hunk.header.width() > content_area.width as usize {
                        termide_ui::path_utils::truncate_to_width(
                            &hunk.header,
                            content_area.width as usize,
                        )
                    } else {
                        hunk.header.clone()
                    };
                    let style = mark(hunk_header_style, current_line);
                    for x in content_area.x..content_area.x + content_area.width {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(' ');
                            cell.set_style(style);
                        }
                    }
                    buf.set_string(content_area.x, y, &header, style);
                    y += 1;
                }
                current_line += 1;

                // Hunk lines
                for line in &hunk.lines {
                    if y >= max_y {
                        break;
                    }

                    if current_line >= self.scroll && y < max_y {
                        let (style, prefix) = match line.kind {
                            LineKind::Added => (added_style, "+"),
                            LineKind::Removed => (removed_style, "-"),
                            LineKind::Context => (context_style, " "),
                            LineKind::HunkHeader => (hunk_header_style, " "),
                        };
                        let style = mark(style, current_line);
                        let line_number_style = mark(line_number_style, current_line);

                        // Clear line with appropriate background
                        for x in content_area.x..content_area.x + content_area.width {
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                cell.set_char(' ');
                                cell.set_style(style);
                            }
                        }

                        let mut x = content_area.x;

                        // Old line number
                        let old_num = line
                            .old_line
                            .map(|n| format!("{:>width$}", n, width = line_num_width))
                            .unwrap_or_else(|| " ".repeat(line_num_width));
                        buf.set_string(x, y, &old_num, line_number_style);
                        x += line_num_width as u16;

                        buf.set_string(x, y, "|", line_number_style);
                        x += 1;

                        // New line number
                        let new_num = line
                            .new_line
                            .map(|n| format!("{:>width$}", n, width = line_num_width))
                            .unwrap_or_else(|| " ".repeat(line_num_width));
                        buf.set_string(x, y, &new_num, line_number_style);
                        x += line_num_width as u16;

                        buf.set_string(x, y, "|", line_number_style);
                        x += 1;

                        // Prefix (+/-)
                        buf.set_string(x, y, prefix, style);
                        x += 1;

                        // Content. `set_string` drops control characters,
                        // so tabs are expanded first or indentation vanishes.
                        let remaining_width =
                            (content_area.x + content_area.width).saturating_sub(x) as usize;
                        let expanded = termide_ui::expand_tabs(&line.content, self.tab_size);
                        let content = if expanded.width() > remaining_width {
                            termide_ui::path_utils::truncate_to_width(&expanded, remaining_width)
                        } else {
                            expanded.into_owned()
                        };
                        buf.set_string(x, y, &content, style);

                        y += 1;
                    }
                    current_line += 1;
                }
            }
        }

        // Render scrollbar
        let needs_scrollbar = ScrollBar::needs_scrollbar(self.visible_height, self.total_lines);
        self.scrollbars.vertical = None;
        if needs_scrollbar {
            if let Some(border_x) = border_right_x {
                self.scrollbars.vertical = ScrollBar::render_tracked(
                    buf,
                    border_x,
                    content_area.y,
                    content_area.height,
                    self.scroll,
                    self.visible_height,
                    self.total_lines,
                    theme,
                    is_focused,
                );
            }
        }

        // Status message
        if let Some(ref msg) = self.status_message {
            let msg_y = area.y + area.height.saturating_sub(2);
            let style = Style::default().fg(theme.cursor);
            buf.set_string(content_area.x, msg_y, msg, style);
        }
    }
}
