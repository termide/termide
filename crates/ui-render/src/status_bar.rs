// Allow clippy lints for status bar
#![allow(clippy::too_many_arguments)]
#![allow(clippy::vec_init_then_push)]

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::borrow::Cow;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
        SegmentKind::Label | SegmentKind::Inactive | SegmentKind::Spacer => base.fg(theme.disabled),
        // Informational value: normal colour, regular weight.
        SegmentKind::Value => base.fg(theme.accented_fg),
        // Clickable / changeable value: normal colour, bold to signal it.
        SegmentKind::Active => base.fg(theme.accented_fg).add_modifier(Modifier::BOLD),
        SegmentKind::Warn => base.fg(theme.warning).add_modifier(Modifier::BOLD),
        SegmentKind::Error => base.fg(theme.error).add_modifier(Modifier::BOLD),
    }
}

/// Lay `left` and `right` out within `width` columns: `left` from column 0,
/// `right` flush with the right edge. When both do not fit, `left` is cut and
/// ends in `…` tagged `ellipsis`; `right` is never cut. Returns each piece
/// with its column.
///
/// The one right-alignment rule of the status bar: the renderer applies it
/// to styled spans, hit-testing to a panel's segments.
fn fit_right<'a, T: Copy>(
    left: Vec<(Cow<'a, str>, T)>,
    right: Vec<(Cow<'a, str>, T)>,
    width: usize,
    ellipsis: T,
) -> Vec<(usize, Cow<'a, str>, T)> {
    let group_width =
        |group: &[(Cow<'a, str>, T)]| group.iter().map(|(text, _)| text.width()).sum::<usize>();
    let budget = width.saturating_sub(group_width(&right));
    let fits = group_width(&left) <= budget;
    let keep = if fits {
        budget
    } else {
        budget.saturating_sub(1)
    };
    let mut placed = Vec::new();
    let mut x = 0;
    for (text, tag) in left {
        let w = text.width();
        if x + w <= keep {
            placed.push((x, text, tag));
            x += w;
            continue;
        }
        let mut cut = String::new();
        let mut cut_width = 0;
        for ch in text.chars() {
            let cw = ch.width().unwrap_or(0);
            if x + cut_width + cw > keep {
                break;
            }
            cut.push(ch);
            cut_width += cw;
        }
        if !cut.is_empty() {
            placed.push((x, Cow::Owned(cut), tag));
        }
        break;
    }
    if !fits && budget > 0 {
        placed.push((keep, Cow::Borrowed("…"), ellipsis));
    }
    let mut x = budget;
    for (text, tag) in right {
        let w = text.width();
        placed.push((x, text, tag));
        x += w;
    }
    placed
}

/// [`fit_right`] for styled spans, the gaps filled with the bar's background.
fn align_right<'a>(
    left: Vec<Span<'a>>,
    right: Vec<Span<'a>>,
    width: u16,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let pieces = |spans: Vec<Span<'a>>| {
        spans
            .into_iter()
            .map(|span| (span.content, span.style))
            .collect()
    };
    let bg = Style::default().bg(theme.accented_bg);
    let mut spans = Vec::new();
    let mut x = 0;
    for (at, text, style) in fit_right(
        pieces(left),
        pieces(right),
        width as usize,
        bg.fg(theme.disabled),
    ) {
        if at > x {
            spans.push(Span::styled(" ".repeat(at - x), bg));
        }
        x = at + text.width();
        spans.push(Span::styled(text, style));
    }
    spans
}

/// A panel's segments split at the first [`SegmentKind::Spacer`]: the ones
/// before it and the ones after it (right-aligned).
fn split_segments(segments: &[StatusSegment]) -> (&[StatusSegment], &[StatusSegment]) {
    match segments.iter().position(|s| s.kind == SegmentKind::Spacer) {
        Some(split) => (&segments[..split], &segments[split + 1..]),
        None => (segments, &[]),
    }
}

/// A panel's segments as [`fit_right`] pieces, each tagged by `tag`.
fn segment_pieces<T>(
    segments: &[StatusSegment],
    tag: impl Fn(&StatusSegment) -> T,
) -> Vec<(Cow<'_, str>, T)> {
    segments
        .iter()
        .map(|seg| (Cow::Borrowed(seg.text.as_str()), tag(seg)))
        .collect()
}

/// Build status-bar spans for a panel's segments.
fn segment_spans<'a>(segments: &'a [StatusSegment], theme: &Theme) -> Vec<Span<'a>> {
    segments
        .iter()
        .map(|seg| Span::styled(seg.text.as_str(), segment_style(seg.kind, theme)))
        .collect()
}

/// Compute clickable hit areas for a panel's segments laid out within
/// `width` columns, starting at `start_x`.
///
/// Uses the renderer's [`fit_right`], so the renderer and the mouse handler
/// agree on column ranges.
pub fn segment_hit_areas(segments: &[StatusSegment], start_x: u16, width: u16) -> Vec<SegmentHit> {
    let (left, right) = split_segments(segments);
    fit_right(
        segment_pieces(left, |seg| seg.action),
        segment_pieces(right, |seg| seg.action),
        width as usize,
        None,
    )
    .into_iter()
    .filter_map(|(x, text, action)| {
        let start = start_x + x as u16;
        Some(SegmentHit {
            start,
            end: start + text.width() as u16,
            action: action?,
        })
    })
    .collect()
}

/// Columns the indicators right of a panel's segments take — background
/// operations and disk space — so the segments are laid out in the rest.
pub fn status_trailing_width(
    plugin_status: Option<&str>,
    background_ops: Option<&BackgroundOpsSummary>,
    disk_space: Option<&DiskSpaceInfo>,
) -> u16 {
    let plugins = plugin_status.map_or(0, |text| " | ".width() + plugin_text(text).width());
    let ops = background_ops
        .filter(|ops| ops.has_operations)
        .map_or(0, |ops| " | ".width() + background_ops_text(ops).width());
    let disk = disk_space.map_or(0, |disk| disk_text(disk).width());
    (plugins + ops + disk) as u16
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
    /// Plugin status items, joined (if any)
    pub plugin_status: Option<&'a str>,
}

/// The plugin status indicator's text, after its separator.
fn plugin_text(text: &str) -> String {
    format!("{text} ")
}

/// The disk-space indicator's text.
fn disk_text(disk: &DiskSpaceInfo) -> String {
    format!(" {} ", disk.format_space())
}

/// The background-operations indicator's text, after its separator.
fn background_ops_text(ops: &BackgroundOpsSummary) -> String {
    let spinner = if ops.is_paused { "⏸" } else { "⟳" };
    format!("{} {} ", spinner, ops.status_text)
}

/// The indicators every status bar ends with, flush right: background
/// operations, then disk space.
fn trailing_spans(
    params: &StatusBarParams<'_>,
    disk_space: Option<&DiskSpaceInfo>,
) -> Vec<Span<'static>> {
    let theme = params.theme;
    let mut spans = Vec::new();
    if let Some(text) = params.plugin_status {
        let style = Style::default().fg(theme.disabled).bg(theme.accented_bg);
        spans.push(Span::styled(" | ", style));
        spans.push(Span::styled(
            plugin_text(text),
            Style::default().fg(theme.accented_fg).bg(theme.accented_bg),
        ));
    }
    if let Some(ops) = params
        .background_ops
        .as_ref()
        .filter(|ops| ops.has_operations)
    {
        spans.push(Span::styled(
            " | ",
            Style::default().fg(theme.disabled).bg(theme.accented_bg),
        ));
        let color = if ops.is_paused {
            theme.warning
        } else {
            theme.accented_fg
        };
        spans.push(Span::styled(
            background_ops_text(ops),
            Style::default()
                .fg(color)
                .bg(theme.accented_bg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(disk) = disk_space {
        let disk_color = resource_color(disk.usage_percent(), theme);
        let style = if params.disk_selected {
            // Inverted colors when selected via menu navigation
            Style::default().fg(theme.accented_bg).bg(disk_color)
        } else {
            Style::default().fg(disk_color).bg(theme.accented_bg)
        };
        spans.push(Span::styled(disk_text(disk), style));
    }
    spans
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

        // Cells in display columns, as the layout above measured them: a wide
        // character drawn into one cell would push the rest of the bottom row
        // past its right edge on the host terminal, and a control character
        // would move its cursor off the row.
        let mut current_x = x;
        for span in line.spans {
            if current_x >= area.right() {
                break;
            }
            current_x += termide_ui::render_text_cells(
                buf,
                current_x,
                y,
                &span.content,
                area.right() - current_x,
                span.style,
            );
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

        // Every layout ends with the background-ops and disk indicators flush
        // right; a narrow bar cuts the panel's own text instead of them.
        let finish = |spans: Vec<Span<'a>>| {
            align_right(
                spans,
                trailing_spans(params, disk_space),
                total_width,
                theme,
            )
        };

        // Generic path: a focused panel that contributes its own segments takes
        // precedence over the typed editor/FM/terminal layouts.
        if let Some(segs) = segments.filter(|s| !s.is_empty()) {
            let (left, right) = split_segments(segs);
            let mut right = segment_spans(right, theme);
            right.extend(trailing_spans(params, disk_space));
            return align_right(segment_spans(left, theme), right, total_width, theme);
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

            finish(spans)
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

            finish(spans)
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

            finish(spans)
        } else {
            // No panel-specific info - check for info messages (e.g., VFS connection status)
            if let Some((message, _is_error)) = params.status_message {
                return vec![Span::styled(format!(" {} ", message), highlight_style)];
            }

            // Fall through to disk_space or default handling
            if disk_space.is_some() {
                // Panels with disk space info (like git status): show title + disk info
                return finish(vec![Span::styled(
                    format!(" {}", panel_title),
                    highlight_style,
                )]);
            }

            // Default: simple title display
            match panel_title {
                "Debug" => {
                    // Debug: layout mode and dimensions
                    let terminal_info =
                        format!("{}x{}", params.terminal_width, params.terminal_height);

                    finish(vec![
                        Span::styled(format!(" {} ", t.status_terminal()), base_style),
                        Span::styled(terminal_info, highlight_style),
                        Span::styled(
                            format!("{}{} ", t.ui_hint_separator(), t.status_layout()),
                            base_style,
                        ),
                        Span::styled(params.recommended_layout.to_string(), highlight_style),
                    ])
                }
                _ => {
                    // Default: simple title display
                    finish(vec![Span::styled(
                        format!(" {}", panel_title),
                        highlight_style,
                    )])
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
        let hits = segment_hit_areas(&segs, 0, 80);
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
            segment_hit_areas(&segs, 5, 80),
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
        assert!(segment_hit_areas(&segs, 0, 80).is_empty());
    }

    /// Each segment's column and drawn text, as hit-testing lays them out.
    fn texts(segs: &[StatusSegment], width: u16) -> Vec<(usize, String)> {
        let (left, right) = split_segments(segs);
        fit_right(
            segment_pieces(left, |_| ()),
            segment_pieces(right, |_| ()),
            width as usize,
            (),
        )
        .into_iter()
        .map(|(x, text, ())| (x, text.into_owned()))
        .collect()
    }

    fn line(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_spacer_pushes_the_rest_to_the_right_edge() {
        let segs = vec![
            StatusSegment::new("left", SegmentKind::Value),
            StatusSegment::spacer(),
            StatusSegment::clickable("R", SegmentKind::Active, "r"),
            StatusSegment::new("ight", SegmentKind::Value),
        ];
        assert_eq!(
            texts(&segs, 20),
            vec![(0, "left".into()), (15, "R".into()), (16, "ight".into())]
        );
        assert_eq!(
            segment_hit_areas(&segs, 0, 20),
            vec![SegmentHit {
                start: 15,
                end: 16,
                action: "r"
            }]
        );
    }

    #[test]
    fn a_narrow_bar_cuts_the_segments_before_the_spacer() {
        let segs = vec![
            StatusSegment::new("ab", SegmentKind::Value),
            StatusSegment::clickable("cdef", SegmentKind::Active, "c"),
            StatusSegment::spacer(),
            StatusSegment::new("xyz", SegmentKind::Value),
        ];
        // 8 columns: 3 for the right group, 4 kept on the left, then `…`.
        assert_eq!(
            texts(&segs, 8),
            vec![
                (0, "ab".into()),
                (2, "cd".into()),
                (4, "…".into()),
                (5, "xyz".into())
            ]
        );
        // The cut chip is clickable only where it is drawn.
        assert_eq!(
            segment_hit_areas(&segs, 0, 8),
            vec![SegmentHit {
                start: 2,
                end: 4,
                action: "c"
            }]
        );
    }

    #[test]
    fn an_exact_fit_is_not_cut() {
        let segs = vec![
            StatusSegment::new("abc", SegmentKind::Value),
            StatusSegment::spacer(),
            StatusSegment::new("xy", SegmentKind::Value),
        ];
        assert_eq!(texts(&segs, 5), vec![(0, "abc".into()), (3, "xy".into())]);
    }

    #[test]
    fn a_narrow_bar_keeps_the_trailing_indicators_and_cuts_the_text() {
        let theme = Theme::default();
        let spans = align_right(
            vec![Span::raw(" /a/long/path")],
            vec![Span::raw(" | "), Span::raw("⟳ Copy ")],
            16,
            &theme,
        );
        assert_eq!(line(&spans), " /a/l… | ⟳ Copy ");
        let spans = align_right(vec![Span::raw(" p")], vec![Span::raw(" 1G ")], 10, &theme);
        assert_eq!(line(&spans), " p     1G ");
    }

    #[test]
    fn plugin_status_sits_before_the_other_indicators() {
        let theme = Theme::default();
        let params = StatusBarParams {
            theme: &theme,
            status_message: None,
            terminal_width: 30,
            terminal_height: 10,
            recommended_layout: "",
            background_ops: Some(BackgroundOpsSummary {
                has_operations: true,
                status_text: "Copy".into(),
                is_paused: false,
            }),
            disk_selected: false,
            plugin_status: Some("PROBE"),
        };
        let segs = vec![StatusSegment::new("left", SegmentKind::Value)];
        let spans =
            StatusBar::get_status_text(&params, "", None, None, None, None, None, Some(&segs), 30);
        let drawn = line(&spans);
        assert!(drawn.ends_with(" | PROBE  | ⟳ Copy "), "{drawn:?}");
        let trailing = status_trailing_width(Some("PROBE"), params.background_ops.as_ref(), None);
        assert_eq!(usize::from(trailing), " | PROBE  | ⟳ Copy ".width());
    }

    #[test]
    fn segments_render_where_their_hit_areas_are() {
        let theme = Theme::default();
        let params = StatusBarParams {
            theme: &theme,
            status_message: None,
            terminal_width: 25,
            terminal_height: 10,
            recommended_layout: "",
            background_ops: Some(BackgroundOpsSummary {
                has_operations: true,
                status_text: "Copy".into(),
                is_paused: false,
            }),
            disk_selected: false,
            plugin_status: None,
        };
        let segs = vec![
            StatusSegment::clickable("Agent: default", SegmentKind::Active, "a"),
            StatusSegment::spacer(),
            StatusSegment::clickable("12k", SegmentKind::Value, "k"),
        ];
        let spans =
            StatusBar::get_status_text(&params, "", None, None, None, None, None, Some(&segs), 25);
        let drawn = line(&spans);
        assert_eq!(drawn, "Agent: defa…12k | ⟳ Copy ");
        let width = 25 - status_trailing_width(None, params.background_ops.as_ref(), None);
        for hit in segment_hit_areas(&segs, 0, width) {
            let text: String = drawn
                .chars()
                .skip(hit.start as usize)
                .take((hit.end - hit.start) as usize)
                .collect();
            let expected = if hit.action == "a" {
                "Agent: defa"
            } else {
                "12k"
            };
            assert_eq!(text, expected);
        }
    }

    /// Wide characters take two cells and the row ends at its right edge:
    /// the right-aligned segment's last character lands in the last column.
    #[test]
    fn wide_text_is_drawn_by_display_width() {
        use ratatui::{buffer::Buffer, layout::Rect};

        let theme = Theme::default();
        let params = StatusBarParams {
            theme: &theme,
            status_message: None,
            terminal_width: 20,
            terminal_height: 5,
            recommended_layout: "",
            background_ops: None,
            disk_selected: false,
            plugin_status: None,
        };
        let segs = vec![
            StatusSegment::new("漢字\tx", SegmentKind::Value),
            StatusSegment::spacer(),
            StatusSegment::new("end", SegmentKind::Value),
        ];
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        StatusBar::render(
            &mut buf,
            area,
            &params,
            "",
            None,
            None,
            None,
            None,
            None,
            Some(&segs),
        );

        assert_eq!(buf[(0, 0)].symbol(), "漢");
        assert_eq!(buf[(2, 0)].symbol(), "字");
        assert_eq!(buf[(4, 0)].symbol(), " ", "the tab is a blank");
        assert_eq!(buf[(5, 0)].symbol(), "x");
        let row: String = (17..20).map(|x| buf[(x, 0)].symbol()).collect();
        assert_eq!(row, "end");
    }
}
