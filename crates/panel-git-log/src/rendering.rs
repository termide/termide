//! Rendering for the Git Log panel: header selectors, commit rows, refs, and dropdowns.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;
use termide_git::{self as git, truncate_right, truncate_to_width};
use termide_ui::ScrollBar;
use termide_ui_render::{render_simple_dropdown, InlineSelector};

use crate::{GitLogPanel, Section};

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

impl GitLogPanel {
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
