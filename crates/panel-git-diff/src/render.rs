//! Diff rendering (file headers, hunks, line numbers, scrollbar) for the Git Diff Panel.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_git::{self as git};
use termide_ui::ScrollBar;

use crate::{FileStatus, GitDiffPanel, LineKind};

/// Blend two colors together.
/// `ratio` 0.0 = all color1, 1.0 = all color2
fn blend_colors(color1: Color, color2: Color, ratio: f32) -> Color {
    let (r1, g1, b1) = match color1 {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::White => (255, 255, 255),
        Color::Black => (0, 0, 0),
        Color::Gray => (128, 128, 128),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Yellow => (255, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        _ => (128, 128, 128),
    };
    let (r2, g2, b2) = match color2 {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::White => (255, 255, 255),
        Color::Black => (0, 0, 0),
        Color::Gray => (128, 128, 128),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Yellow => (255, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        _ => (128, 128, 128),
    };

    let ratio = ratio.clamp(0.0, 1.0);
    let inv = 1.0 - ratio;

    Color::Rgb(
        (r1 as f32 * inv + r2 as f32 * ratio) as u8,
        (g1 as f32 * inv + g2 as f32 * ratio) as u8,
        (b1 as f32 * inv + b2 as f32 * ratio) as u8,
    )
}

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
        let added_bg = blend_colors(theme.success, theme.bg, 0.85);
        let removed_bg = blend_colors(theme.error, theme.bg, 0.85);
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

        let mut current_line = 0usize;
        let mut y = content_area.y;
        let max_y = content_area.y + content_area.height;

        for (file_idx, diff) in self.diffs.iter().enumerate() {
            if y >= max_y {
                break;
            }

            // File header line
            if current_line >= self.scroll && y < max_y {
                let is_selected = is_focused && file_idx == self.selected_file;
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
                        git::truncate_to_width(&hunk.header, content_area.width as usize)
                    } else {
                        hunk.header.clone()
                    };
                    buf.set_string(content_area.x, y, &header, hunk_header_style);
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

                        // Content
                        let remaining_width =
                            (content_area.x + content_area.width).saturating_sub(x) as usize;
                        let content = if line.content.width() > remaining_width {
                            git::truncate_to_width(&line.content, remaining_width)
                        } else {
                            line.content.clone()
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
