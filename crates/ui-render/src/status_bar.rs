// Allow clippy lints for status bar
#![allow(clippy::too_many_arguments)]
#![allow(clippy::vec_init_then_push)]

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use termide_core::{SegmentKind, StatusSegment};
use termide_i18n as i18n;
use termide_panel_editor::EditorInfo;
use termide_panel_file_manager::FileInfo;
use termide_panel_terminal::TerminalInfo;
use termide_system_monitor::{DiskSpaceInfo, DiskSpaceInfoExt};
use termide_theme::Theme;

use super::menu::resource_color;

/// X-range (status-bar columns) of a clickable [`StatusSegment`], with its
/// action id. Computed identically at render time and on click so hit-testing
/// stays accurate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHit {
    /// First column (inclusive).
    pub start: u16,
    /// One past the last column (exclusive).
    pub end: u16,
    /// Action id routed to the panel's `handle_status_action`.
    pub action: &'static str,
}

/// Style for a panel-contributed status segment.
fn segment_style(kind: SegmentKind, theme: &Theme) -> Style {
    let base = Style::default().bg(theme.accented_bg);
    match kind {
        // Field labels / separators: dimmed.
        SegmentKind::Label | SegmentKind::Inactive => base.fg(theme.disabled),
        // Informational value: normal colour, regular weight.
        SegmentKind::Value => base.fg(theme.accented_fg),
        // Clickable / changeable value: normal colour, bold to signal it.
        SegmentKind::Active => base.fg(theme.accented_fg).add_modifier(Modifier::BOLD),
        SegmentKind::Warn => base.fg(theme.warning).add_modifier(Modifier::BOLD),
        SegmentKind::Error => base.fg(theme.error).add_modifier(Modifier::BOLD),
    }
}

/// Build status-bar spans for a panel's segments.
fn segment_spans<'a>(segments: &'a [StatusSegment], theme: &Theme) -> Vec<Span<'a>> {
    segments
        .iter()
        .map(|seg| Span::styled(seg.text.as_str(), segment_style(seg.kind, theme)))
        .collect()
}

/// Compute clickable hit areas for a panel's segments, starting at `start_x`.
///
/// Pure function of the segment list so the renderer and the mouse handler
/// agree on column ranges.
pub fn segment_hit_areas(segments: &[StatusSegment], start_x: u16) -> Vec<SegmentHit> {
    let mut hits = Vec::new();
    let mut x = start_x;
    for seg in segments {
        let w = seg.text.as_str().width() as u16;
        if let Some(action) = seg.action {
            hits.push(SegmentHit {
                start: x,
                end: x + w,
                action,
            });
        }
        x += w;
    }
    hits
}

/// Summary of background file operations (for status bar display).
#[derive(Debug, Clone, Default)]
pub struct BackgroundOpsSummary {
    /// Whether there are active background operations.
    pub has_operations: bool,
    /// Text to display (e.g., "Copying 45%").
    pub status_text: String,
    /// Whether any operation is paused.
    pub is_paused: bool,
}

/// Status bar rendering parameters (extracted from AppState to avoid cyclic deps)
pub struct StatusBarParams<'a> {
    /// Theme reference
    pub theme: &'a Theme,
    /// Status message (message, is_error)
    pub status_message: Option<&'a (String, bool)>,
    /// Terminal dimensions
    pub terminal_width: u16,
    pub terminal_height: u16,
    /// Recommended layout string (for Debug panel)
    pub recommended_layout: &'a str,
    /// Background file operations summary (if any)
    pub background_ops: Option<BackgroundOpsSummary>,
    /// Whether disk indicator is selected via menu navigation
    pub disk_selected: bool,
}

/// Calculate width of spans accounting for unicode characters.
fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| match &s.content {
            std::borrow::Cow::Borrowed(s) => s.width(),
            std::borrow::Cow::Owned(s) => s.width(),
        })
        .sum()
}

/// Append disk space info to spans, right-aligned.
fn append_disk_space(
    spans: &mut Vec<Span<'_>>,
    disk: &DiskSpaceInfo,
    theme: &Theme,
    total_width: u16,
    selected: bool,
) {
    let disk_text = format!(" {} ", disk.format_space());
    let disk_color = resource_color(disk.usage_percent(), theme);

    // Add padding between left part and disk info
    let used_width = spans_width(spans);
    let remaining = (total_width as usize).saturating_sub(used_width + disk_text.width());
    if remaining > 0 {
        spans.push(Span::raw(" ".repeat(remaining)));
    }

    let style = if selected {
        // Inverted colors when selected via menu navigation
        Style::default().fg(theme.accented_bg).bg(disk_color)
    } else {
        Style::default().fg(disk_color).bg(theme.accented_bg)
    };

    spans.push(Span::styled(disk_text, style));
}

/// Append background operations indicator to spans.
fn append_background_ops(spans: &mut Vec<Span<'_>>, ops: &BackgroundOpsSummary, theme: &Theme) {
    if !ops.has_operations {
        return;
    }

    // Add separator
    spans.push(Span::styled(
        " | ",
        Style::default().fg(theme.disabled).bg(theme.accented_bg),
    ));

    // Spinner character (alternates based on time)
    let spinner = if ops.is_paused { "⏸" } else { "⟳" };

    let color = if ops.is_paused {
        theme.warning
    } else {
        theme.accented_fg
    };

    spans.push(Span::styled(
        format!("{} {} ", spinner, ops.status_text),
        Style::default()
            .fg(color)
            .bg(theme.accented_bg)
            .add_modifier(Modifier::BOLD),
    ));
}

/// Status bar at the bottom of screen
pub struct StatusBar;

impl StatusBar {
    /// Render status bar
    pub fn render(
        buf: &mut Buffer,
        area: Rect,
        params: &StatusBarParams<'_>,
        panel_title: &str,
        selected_count: Option<usize>,
        file_info: Option<&FileInfo>,
        disk_space: Option<&DiskSpaceInfo>,
        editor_info: Option<&EditorInfo>,
        terminal_info: Option<&TerminalInfo>,
        segments: Option<&[StatusSegment]>,
    ) {
        if area.height == 0 {
            return;
        }

        let status_text = Self::get_status_text(
            params,
            panel_title,
            selected_count,
            file_info,
            disk_space,
            editor_info,
            terminal_info,
            segments,
            area.width,
        );

        // Fill entire line with background color from theme
        // Pre-compute style outside loop to avoid per-pixel allocation
        let bg_style = Style::default().bg(params.theme.accented_bg);
        for x in area.left()..area.right() {
            buf[(x, area.top())].set_char(' ').set_style(bg_style);
        }

        // Render status bar text
        let line = Line::from(status_text);
        let x = area.left();
        let y = area.top();

        let mut current_x = x;
        for span in line.spans {
            // Use span.content directly without allocating String
            for ch in span.content.chars() {
                if current_x >= area.right() {
                    break;
                }
                buf[(current_x, y)].set_char(ch).set_style(span.style);
                current_x += 1;
            }
        }
    }

    /// Get text for status bar depending on active panel
    fn get_status_text<'a>(
        params: &'a StatusBarParams<'a>,
        panel_title: &'a str,
        selected_count: Option<usize>,
        file_info: Option<&'a FileInfo>,
        disk_space: Option<&'a DiskSpaceInfo>,
        editor_info: Option<&'a EditorInfo>,
        terminal_info: Option<&'a TerminalInfo>,
        segments: Option<&'a [StatusSegment]>,
        total_width: u16,
    ) -> Vec<Span<'a>> {
        let t = i18n::t();
        let theme = params.theme;

        // If there's an ERROR message, show it with priority
        // Info messages don't block file_info display (unless git operation in progress)
        if let Some((message, is_error)) = params.status_message {
            if *is_error {
                let msg_style = Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD);

                return vec![Span::styled(format!(" {} ", message), msg_style)];
            }
        }

        // Generic path: a focused panel that contributes its own segments takes
        // precedence over the typed editor/FM/terminal layouts. Disk space and
        // background ops still render on the right.
        if let Some(segs) = segments.filter(|s| !s.is_empty()) {
            let mut spans = segment_spans(segs, theme);
            if let Some(ref ops) = params.background_ops {
                append_background_ops(&mut spans, ops, theme);
            }
            if let Some(disk) = disk_space {
                append_disk_space(&mut spans, disk, theme, total_width, params.disk_selected);
            }
            return spans;
        }

        let base_style = Style::default().fg(theme.disabled).bg(theme.accented_bg);

        let highlight_style = Style::default()
            .fg(theme.accented_fg)
            .bg(theme.accented_bg)
            .add_modifier(Modifier::BOLD);

        // Show different information depending on panel type
        // If terminal_info is passed, this is Terminal
        if let Some(info) = terminal_info {
            // Terminal: user@host | /path on the left, disk space on the right
            let mut spans = vec![];

            spans.push(Span::styled(" ", base_style));
            spans.push(Span::styled(info.user_host.as_str(), highlight_style));
            spans.push(Span::styled(" | ", base_style));
            spans.push(Span::styled(info.cwd.as_str(), highlight_style));

            // Add background operations indicator if any
            if let Some(ref ops) = params.background_ops {
                append_background_ops(&mut spans, ops, theme);
            }

            // If there's disk information, add it on the right
            if let Some(disk) = disk_space {
                append_disk_space(&mut spans, disk, theme, total_width, params.disk_selected);
            }

            spans
        } else if let Some(info) = file_info {
            // File manager: show information about current file
            let mut spans = vec![];

            // Layout: "Dir:/File: name [→ target] | Mod: 0755 | Owner: nvn:users | Size: …"
            // where Size is the byte size for files and "<size> (N items)" for
            // directories.

            if info.file_type == "Directory" || (info.file_type == "Symlink" && info.target_is_dir)
            {
                spans.push(Span::styled(format!(" {} ", t.status_dir()), base_style));
            } else {
                spans.push(Span::styled(format!(" {} ", t.status_file()), base_style));
            }
            spans.push(Span::styled(info.name.as_str(), highlight_style));

            if let Some(ref target) = info.symlink_target {
                spans.push(Span::styled(" → ", base_style));
                spans.push(Span::styled(target.as_str(), highlight_style));
            }

            spans.push(Span::styled(
                format!("{}{} ", t.ui_hint_separator(), t.status_mod()),
                base_style,
            ));
            spans.push(Span::styled(info.mode.as_str(), highlight_style));

            spans.push(Span::styled(
                format!("{}{} ", t.ui_hint_separator(), t.status_owner()),
                base_style,
            ));
            spans.push(Span::styled(
                format!("{}:{}", info.owner, info.group),
                highlight_style,
            ));

            // Size, to the right of Owner. Files: byte size. Directories:
            // "<recursive size> (N items)" (same as the info modal).
            spans.push(Span::styled(
                format!("{}{} ", t.ui_hint_separator(), t.status_size()),
                base_style,
            ));
            spans.push(Span::styled(info.size.as_str(), highlight_style));

            // If there are selected files, add their count
            if let Some(count) = selected_count {
                if count > 0 {
                    spans.push(Span::styled(
                        format!("{}{} ", t.ui_hint_separator(), t.status_selected()),
                        base_style,
                    ));
                    spans.push(Span::styled(
                        format!("{}", count),
                        Style::default()
                            .fg(theme.success)
                            .bg(theme.accented_bg)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }

            // Add background operations indicator if any
            if let Some(ref ops) = params.background_ops {
                append_background_ops(&mut spans, ops, theme);
            }

            // If there's disk information, add it on the right
            if let Some(disk) = disk_space {
                append_disk_space(&mut spans, disk, theme, total_width, params.disk_selected);
            }

            spans
        } else if let Some(info) = editor_info {
            // Editor: cursor position, tab size, encoding, file type, modes on the left
            // disk space on the right
            let mut spans = vec![];

            // Position
            spans.push(Span::styled(format!(" {} ", t.status_pos()), base_style));
            spans.push(Span::styled(
                format!("{}:{}", info.line, info.column),
                highlight_style,
            ));

            // Tab size
            spans.push(Span::styled(
                format!("{}{} ", t.ui_hint_separator(), t.status_tab()),
                base_style,
            ));
            spans.push(Span::styled(format!("{}", info.tab_size), highlight_style));

            // Line ending
            spans.push(Span::styled(t.ui_hint_separator(), base_style));
            spans.push(Span::styled(info.line_ending.as_str(), highlight_style));

            // Encoding
            spans.push(Span::styled(t.ui_hint_separator(), base_style));
            spans.push(Span::styled(info.encoding.as_str(), highlight_style));

            // File type
            spans.push(Span::styled(t.ui_hint_separator(), base_style));
            if info.syntax_highlighting {
                spans.push(Span::styled(info.file_type.as_str(), highlight_style));
            } else {
                spans.push(Span::styled(t.status_plain_text(), highlight_style));
            }

            // Read-only indicator
            if info.read_only {
                spans.push(Span::styled(t.ui_hint_separator(), base_style));
                spans.push(Span::styled(t.status_readonly(), highlight_style));
            }

            // Vim mode indicator
            if let Some(mode) = info.vim_mode {
                spans.push(Span::styled(t.ui_hint_separator(), base_style));
                spans.push(Span::styled(
                    mode,
                    Style::default()
                        .fg(theme.warning)
                        .bg(theme.accented_bg)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            // Add background operations indicator if any
            if let Some(ref ops) = params.background_ops {
                append_background_ops(&mut spans, ops, theme);
            }

            // If there's disk information, add it on the right
            if let Some(disk) = disk_space {
                append_disk_space(&mut spans, disk, theme, total_width, params.disk_selected);
            }

            spans
        } else {
            // No panel-specific info - check for info messages (e.g., VFS connection status)
            if let Some((message, _is_error)) = params.status_message {
                return vec![Span::styled(format!(" {} ", message), highlight_style)];
            }

            // Fall through to disk_space or default handling
            if let Some(disk) = disk_space {
                // Panels with disk space info (like git status): show title + disk info
                let mut spans = vec![Span::styled(format!(" {}", panel_title), highlight_style)];
                // Add background operations indicator if any
                if let Some(ref ops) = params.background_ops {
                    append_background_ops(&mut spans, ops, theme);
                }
                append_disk_space(&mut spans, disk, theme, total_width, params.disk_selected);
                return spans;
            }

            // Default: simple title display
            match panel_title {
                "Debug" => {
                    // Debug: layout mode and dimensions
                    let terminal_info =
                        format!("{}x{}", params.terminal_width, params.terminal_height);

                    let mut spans = vec![
                        Span::styled(format!(" {} ", t.status_terminal()), base_style),
                        Span::styled(terminal_info, highlight_style),
                        Span::styled(
                            format!("{}{} ", t.ui_hint_separator(), t.status_layout()),
                            base_style,
                        ),
                        Span::styled(params.recommended_layout.to_string(), highlight_style),
                    ];
                    // Add background operations indicator if any
                    if let Some(ref ops) = params.background_ops {
                        append_background_ops(&mut spans, ops, theme);
                    }
                    spans
                }
                _ => {
                    // Default: simple title display
                    let mut spans =
                        vec![Span::styled(format!(" {}", panel_title), highlight_style)];
                    // Add background operations indicator if any
                    if let Some(ref ops) = params.background_ops {
                        append_background_ops(&mut spans, ops, theme);
                    }
                    spans
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_core::{SegmentKind, StatusSegment};

    #[test]
    fn hit_areas_track_clickable_segments() {
        // " 0x10 " (6) | "Hex" (3) | "│" (1) | "Text" (4)
        let segs = vec![
            StatusSegment::new(" 0x10 ", SegmentKind::Value),
            StatusSegment::clickable("Hex", SegmentKind::Active, "toggle_hex"),
            StatusSegment::new("│", SegmentKind::Label),
            StatusSegment::clickable("Text", SegmentKind::Inactive, "toggle_hex"),
        ];
        let hits = segment_hit_areas(&segs, 0);
        assert_eq!(
            hits,
            vec![
                SegmentHit {
                    start: 6,
                    end: 9,
                    action: "toggle_hex"
                },
                SegmentHit {
                    start: 10,
                    end: 14,
                    action: "toggle_hex"
                },
            ]
        );
        // A click inside the Hex chip lands in the first hit area.
        assert!(hits.iter().any(|h| (h.start..h.end).contains(&7)));
    }

    #[test]
    fn hit_areas_respect_start_offset() {
        let segs = vec![StatusSegment::clickable("X", SegmentKind::Active, "a")];
        assert_eq!(
            segment_hit_areas(&segs, 5),
            vec![SegmentHit {
                start: 5,
                end: 6,
                action: "a"
            }]
        );
    }

    #[test]
    fn non_clickable_segments_produce_no_hits() {
        let segs = vec![StatusSegment::new("plain", SegmentKind::Value)];
        assert!(segment_hit_areas(&segs, 0).is_empty());
    }
}
