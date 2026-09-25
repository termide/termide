use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{file_search::FileSearchMode, utils, FileManager};
use termide_config::FileManagerSettings;
use termide_git::GitStatus;
use termide_theme::Theme;
use termide_ui::grapheme_utils::{prepare_matched_line, truncate_from_start};

/// Pre-allocated spaces for padding (avoids `.repeat()` allocations in render loops).
const PAD: &str = "                                                                                                                                                                                                        ";

/// Get style for git status (extracted to avoid duplication)
fn git_status_style(status: GitStatus, theme: &Theme) -> Style {
    match status {
        GitStatus::Ignored => Style::default()
            .fg(theme.disabled)
            .add_modifier(Modifier::DIM),
        GitStatus::Modified => Style::default().fg(theme.warning),
        GitStatus::Added => Style::default().fg(theme.success),
        GitStatus::Deleted => Style::default().fg(theme.error),
        GitStatus::Unmodified => Style::default().fg(theme.fg),
    }
}

// On Windows, U+25B6 ▶ and U+25B7 ▷ are outside WGL4 and render as tofu squares.
// U+25BA ► is in WGL4 and displays correctly on all Windows console fonts.
const DIR_COLLAPSED: &str = if cfg!(windows) { "►" } else { "▶" };
const DIR_COLLAPSED_SYMLINK: &str = if cfg!(windows) { "►" } else { "▷" };
const DIR_EXPANDED_SYMLINK: &str = if cfg!(windows) { "▼" } else { "▽" };
// Marker for a remote symlink whose target type couldn't be resolved from
// the directory listing (SFTP/FTP report the link, not its target). Local
// symlinks don't use this — local listings already classify them (dir
// symlinks get the dir-symlink arrow, file symlinks render in italic). A
// plain ASCII `@` (the `ls -F` convention) is used deliberately: it's one
// column on every terminal, unlike ambiguous-width arrows that some
// terminals draw two cells wide and break column alignment.
const SYMLINK: &str = "@";

/// Get icon for entry, accounting for expand/collapse state.
/// `is_remote` marks entries from a remote filesystem, where a symlink's
/// target type is unknown — those get a symlink marker; local symlinks rely
/// on the dir-symlink arrow / italic name styling instead.
fn get_icon(entry: &super::FileEntry, expanded: Option<bool>, is_remote: bool) -> &'static str {
    if entry.git_status == GitStatus::Deleted {
        return "✗";
    }
    if entry.name == ".." {
        return "↑";
    }
    if entry.is_dir {
        return match expanded {
            Some(true) => {
                if entry.is_symlink {
                    DIR_EXPANDED_SYMLINK
                } else {
                    "▼"
                }
            }
            Some(false) | None => {
                if entry.is_symlink {
                    DIR_COLLAPSED_SYMLINK
                } else {
                    DIR_COLLAPSED
                }
            }
        };
    }
    if is_remote && entry.is_symlink {
        return SYMLINK;
    }
    " "
}

impl FileManager {
    /// Get list of lines for display
    pub(crate) fn get_items(
        &self,
        height: usize,
        available_width: usize,
        theme: &Theme,
        is_focused: bool,
        config: &FileManagerSettings,
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
        let show_extended = available_width >= config.extended_view_width;

        for (vis_i, &tree_idx) in self.visible_indices.iter().enumerate() {
            if vis_i < visible_start || vis_i >= visible_end {
                continue;
            }

            let tree_entry = &self.tree_entries[tree_idx];
            let entry = &tree_entry.file_entry;
            let tree_prefix = &self.tree_prefixes[vis_i];
            let tree_prefix_width = tree_prefix.width();

            let is_selected = self.selection.is_selected(vis_i);
            let is_cursor = vis_i == self.selected;

            let attr = utils::get_attribute(entry, is_selected);
            let icon = get_icon(entry, tree_entry.expanded, self.is_remote());
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
                available_width.saturating_sub(
                    tree_prefix_width
                        + attr_width
                        + icon_width
                        + 1
                        + prefix_width
                        + SEPARATOR_WIDTH
                        + SIZE_COLUMN_WIDTH
                        + SEPARATOR_WIDTH
                        + TIME_COLUMN_WIDTH,
                )
            } else {
                available_width
                    .saturating_sub(tree_prefix_width + attr_width + icon_width + 1 + prefix_width)
            };

            let name = utils::truncate_name(&entry.name, max_name_len);
            let name_width = name.width();
            let full_name = format!("{}{}", dir_prefix, name);

            let (bg_style, fg_style) = if is_cursor && is_focused {
                let normal_fg_style = git_status_style(entry.git_status, theme);
                let fg_color = normal_fg_style.fg.unwrap_or(theme.fg);
                let cursor_style = Style::default()
                    .bg(fg_color)
                    .fg(theme.bg)
                    .add_modifier(Modifier::BOLD);
                (cursor_style, cursor_style)
            } else {
                let fg_style = git_status_style(entry.git_status, theme);
                (Style::default(), fg_style)
            };

            let attr_style = if is_selected {
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else if attr == "R" {
                Style::default().fg(theme.disabled)
            } else {
                fg_style
            };

            let fg_style = if is_selected && !(is_cursor && is_focused) {
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                fg_style
            };

            let icon_style = if entry.git_status == GitStatus::Deleted {
                Style::default().fg(theme.error)
            } else {
                fg_style
            };

            let name_style = {
                let mut style = fg_style;
                if entry.git_status == GitStatus::Deleted && !(is_cursor && is_focused) {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }
                if !entry.is_dir && entry.is_symlink {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                // Symlinks carry the target's permission bits (typically
                // 0o777), so the executable bit is meaningless for them —
                // don't render them bold like real executables.
                if !entry.is_dir && entry.is_executable && !entry.is_symlink {
                    style = style.add_modifier(Modifier::BOLD);
                }
                style
            };

            // Tree prefix style: dimmed connectors
            let prefix_style = if is_cursor && is_focused {
                bg_style
            } else {
                Style::default().fg(theme.disabled)
            };

            if show_extended {
                let padding_len = max_name_len.saturating_sub(name_width);
                let padding = &PAD[..padding_len.min(PAD.len())];

                let size_str: std::borrow::Cow<'static, str> = if let Some(size) = entry.size {
                    format!("{:>10}", utils::format_size_compact(size)).into()
                } else if entry.is_dir
                    && entry.name != ".."
                    && !self.is_remote()
                    && config.dir_size_in_wide_view
                    && config.dir_size_budget_ms > 0
                {
                    match utils::shared_dir_size_cache().get(&tree_entry.full_path) {
                        Some(outcome) if outcome.overflowed => {
                            std::borrow::Cow::Borrowed("         -")
                        }
                        Some(outcome) => {
                            format!("{:>10}", utils::format_size_compact(outcome.size)).into()
                        }
                        None => std::borrow::Cow::Borrowed("          "),
                    }
                } else {
                    std::borrow::Cow::Borrowed("          ")
                };

                let time_str = utils::format_modified_time(entry.modified);

                // Attribute gutter (selection/read-only marker) sits in the
                // leftmost column; the tree connector then leads straight into
                // the icon with no gap between the two.
                let mut spans = Vec::with_capacity(10);
                spans.push(Span::styled(attr, attr_style));
                if !tree_prefix.is_empty() {
                    spans.push(Span::styled(tree_prefix.as_str(), prefix_style));
                }
                spans.extend([
                    Span::styled(icon, icon_style),
                    Span::styled(" ", bg_style),
                    Span::styled(full_name, name_style),
                    Span::styled(padding, bg_style),
                    Span::styled(SEPARATOR, bg_style.fg(theme.disabled)),
                    Span::styled(size_str, fg_style),
                    Span::styled(SEPARATOR, bg_style.fg(theme.disabled)),
                    Span::styled(time_str, fg_style),
                ]);
                lines.push(Line::from(spans));
            } else {
                let content_width =
                    tree_prefix_width + attr_width + icon_width + 1 + prefix_width + name_width;
                let padding_len = available_width.saturating_sub(content_width);
                let padding = &PAD[..padding_len.min(PAD.len())];

                let mut spans = Vec::with_capacity(6);
                spans.push(Span::styled(attr, attr_style));
                if !tree_prefix.is_empty() {
                    spans.push(Span::styled(tree_prefix.as_str(), prefix_style));
                }
                spans.extend([
                    Span::styled(icon, icon_style),
                    Span::styled(" ", bg_style),
                    Span::styled(full_name, name_style),
                    Span::styled(padding, bg_style),
                ]);
                lines.push(Line::from(spans));
            }
        }

        // Fill remaining space with empty lines (with separators in extended mode)
        if show_extended && lines.len() < height {
            let name_column_width = available_width.saturating_sub(
                SEPARATOR_WIDTH + SIZE_COLUMN_WIDTH + SEPARATOR_WIDTH + TIME_COLUMN_WIDTH,
            );
            let empty_name = &PAD[..name_column_width.min(PAD.len())];
            let empty_size = &PAD[..SIZE_COLUMN_WIDTH.min(PAD.len())];
            let empty_time = &PAD[..TIME_COLUMN_WIDTH.min(PAD.len())];
            let separator_style = Style::default().fg(theme.disabled);

            for _ in lines.len()..height {
                lines.push(Line::from(vec![
                    Span::raw(empty_name),
                    Span::styled(SEPARATOR, separator_style),
                    Span::raw(empty_size),
                    Span::styled(SEPARATOR, separator_style),
                    Span::raw(empty_time),
                ]));
            }
        }

        lines
    }

    /// Render search results instead of normal file tree
    pub(crate) fn render_search_results(
        &self,
        area: Rect,
        buf: &mut Buffer,
        search: &super::file_search::FileSearchState,
        theme: &Theme,
    ) {
        if search.is_searching {
            let style = Style::default()
                .fg(theme.accented_bg)
                .add_modifier(Modifier::DIM);
            buf.set_string(area.x, area.y, "Searching…", style);
            return;
        }

        if search.tree_nodes.is_empty() {
            let style = Style::default()
                .fg(theme.accented_bg)
                .add_modifier(Modifier::DIM);
            buf.set_string(area.x, area.y, "No matches found", style);
            return;
        }

        match search.mode {
            FileSearchMode::FileGlob => {
                self.render_file_search_results(area, buf, search, theme);
            }
            FileSearchMode::Content => {
                self.render_content_search_results(area, buf, search, theme);
            }
        }
    }

    fn render_file_search_results(
        &self,
        area: Rect,
        buf: &mut Buffer,
        search: &super::file_search::FileSearchState,
        theme: &Theme,
    ) {
        let max_lines = area.height as usize;
        let content_width = area.width as usize;

        // Directory rows show a collapse marker; children of a collapsed
        // directory are skipped (node_display_lines == 0).
        let dir_marker = |collapsed: bool| -> &'static str {
            match (collapsed, cfg!(windows)) {
                (true, true) => "► ",
                (true, false) => "▶ ",
                (false, true) => "▼ ",
                (false, false) => "▼ ",
            }
        };

        let mut vis_idx = 0usize;
        for idx in search.scroll_offset..search.tree_nodes.len() {
            if vis_idx >= max_lines {
                break;
            }
            if search.node_display_lines(idx) == 0 {
                continue; // hidden under a collapsed directory
            }
            let node = &search.tree_nodes[idx];

            let is_selected = idx == search.cursor;
            let prefix = &search.tree_prefixes[idx];

            let style = if is_selected {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                git_status_style(node.git_status, theme)
            };

            let prefix_style = if is_selected {
                style
            } else {
                Style::default().fg(theme.disabled)
            };

            let icon_text = if node.is_dir {
                dir_marker(node.collapsed)
            } else {
                ""
            };

            let mut spans = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(prefix.as_str(), prefix_style));
            }
            spans.push(Span::styled(icon_text, style));
            if node.is_dir {
                spans.push(Span::styled("/", style));
            }
            spans.push(Span::styled(node.name.as_str(), style));

            let text_width: usize = spans.iter().map(|s| s.content.width()).sum();
            let padding_len = content_width.saturating_sub(text_width);
            if padding_len > 0 {
                spans.push(Span::styled(&PAD[..padding_len.min(PAD.len())], style));
            }

            let y = area.y + vis_idx as u16;
            buf.set_line(area.x, y, &Line::from(spans), area.width);
            vis_idx += 1;
        }
    }

    fn render_content_search_results(
        &self,
        area: Rect,
        buf: &mut Buffer,
        search: &super::file_search::FileSearchState,
        theme: &Theme,
    ) {
        let max_lines = area.height as usize;
        let content_width = area.width as usize;

        let line_num_width = 5usize;
        let separator = " \u{2502} ";
        let separator_len = 3usize;
        let indent = 1usize;
        let max_text_width = content_width.saturating_sub(indent + line_num_width + separator_len);

        let dim = Style::default().fg(theme.disabled).bg(theme.bg);
        let fg = Style::default().fg(theme.fg).bg(theme.bg);
        let highlight = Style::default().bg(theme.selected_bg).fg(theme.selected_fg);
        let header_sel = Style::default()
            .fg(theme.bg)
            .bg(theme.accented_fg)
            .add_modifier(Modifier::BOLD);
        let removed = Style::default().fg(theme.error).bg(theme.bg);
        let added = Style::default().fg(theme.success).bg(theme.bg);

        let mut line = 0usize;
        for (idx, node) in search
            .tree_nodes
            .iter()
            .enumerate()
            .skip(search.scroll_offset)
        {
            let node_lines = search.node_display_lines(idx);
            if node_lines == 0 {
                continue;
            }
            if line + node_lines > max_lines {
                break;
            }
            let y = area.y + line as u16;
            let is_selected = idx == search.cursor;

            if node.is_file_header {
                let base = if is_selected {
                    header_sel
                } else {
                    fg.add_modifier(Modifier::BOLD)
                };
                // Bracketed triangle like the git-diff panel: [▶] collapsed /
                // [▼] expanded (► on Windows, outside WGL4 otherwise).
                let marker = if node.collapsed {
                    if cfg!(windows) {
                        "[►] "
                    } else {
                        "[▶] "
                    }
                } else {
                    "[▼] "
                };
                // Per-file selection checkbox (content replace mode only).
                let checkbox = if search.show_checkboxes {
                    if search.is_header_selected(idx) {
                        "[✓] "
                    } else {
                        "[ ] "
                    }
                } else {
                    ""
                };
                let count_text = format!(" {}", node.match_count);
                let avail = content_width
                    .saturating_sub(marker.width() + checkbox.width() + count_text.width());
                let name = truncate_from_start(&node.name, avail);

                let mut x = area.x;
                buf.set_string(x, y, marker, base);
                x += marker.width() as u16;
                if !checkbox.is_empty() {
                    let cb_style = if is_selected {
                        base
                    } else {
                        Style::default().fg(theme.accented_fg)
                    };
                    buf.set_string(x, y, checkbox, cb_style);
                    x += checkbox.width() as u16;
                }
                buf.set_string(x, y, &name, base);
                x += name.width() as u16;

                let used = (x - area.x) as usize;
                let pad_len = content_width.saturating_sub(used + count_text.width());
                let fill = if is_selected { base } else { fg };
                if pad_len > 0 {
                    buf.set_string(x, y, &PAD[..pad_len.min(PAD.len())], fill);
                    x += pad_len as u16;
                }
                let count_style = if is_selected { base } else { dim };
                buf.set_string(x, y, &count_text, count_style);
            } else if let Some(cm) = &node.content_match {
                if node_lines == 2 {
                    // -old / +new preview for the cursor match.
                    let new_line = search
                        .preview_replacement(&cm.matched_line)
                        .unwrap_or_else(|| cm.matched_line.clone());
                    for (off, (gutter, text, style)) in [
                        ("-", cm.matched_line.as_str(), removed),
                        ("+", new_line.as_str(), added),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let ry = y + off as u16;
                        let mut x = area.x + indent as u16;
                        buf.set_string(x, ry, gutter, style);
                        x += 1;
                        buf.set_string(x, ry, " ", style);
                        x += 1;
                        let shown =
                            truncate_from_start(text, content_width.saturating_sub(indent + 2));
                        buf.set_string(x, ry, &shown, style);
                    }
                } else {
                    let row_bg = if is_selected {
                        Style::default().bg(theme.selected_bg)
                    } else {
                        Style::default().bg(theme.bg)
                    };
                    for col in 0..content_width {
                        buf.set_string(area.x + col as u16, y, " ", row_bg);
                    }

                    let line_num = format!("{:>width$}", cm.line_number, width = line_num_width);
                    let lnum_style = if is_selected {
                        row_bg.fg(theme.fg)
                    } else {
                        dim
                    };
                    let sep_style = if is_selected {
                        row_bg.fg(theme.disabled)
                    } else {
                        dim
                    };
                    let text_style = if is_selected { row_bg.fg(theme.fg) } else { fg };

                    let mut x = area.x + indent as u16;
                    buf.set_string(x, y, &line_num, lnum_style);
                    x += line_num_width as u16;
                    buf.set_string(x, y, separator, sep_style);
                    x += separator_len as u16;

                    let (display_line, match_start_g, match_end_g) = prepare_matched_line(
                        &cm.matched_line,
                        cm.match_start,
                        cm.match_end,
                        max_text_width,
                    );
                    for (grapheme_idx, grapheme) in display_line.graphemes(true).enumerate() {
                        let style = if grapheme_idx >= match_start_g && grapheme_idx < match_end_g {
                            highlight
                        } else {
                            text_style
                        };
                        buf.set_string(x, y, grapheme, style);
                        x += grapheme.width() as u16;
                    }
                }
            } else {
                // "+ N more" overflow row (no content match): dim, aligned at
                // the line-number column.
                let style = if is_selected { fg } else { dim };
                let shown = truncate_from_start(&node.name, content_width.saturating_sub(indent));
                buf.set_string(area.x + indent as u16, y, &shown, style);
            }

            line += node_lines;
        }
    }
}
