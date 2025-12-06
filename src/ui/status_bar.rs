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

use crate::i18n;
use crate::panels::editor::EditorInfo;
use crate::panels::file_manager::FileInfo;
use crate::panels::terminal_pty::TerminalInfo;
use crate::state::AppState;
use crate::system_monitor::DiskSpaceInfo;

/// Status bar at the bottom of screen
pub struct StatusBar;

impl StatusBar {
    /// Render status bar
    pub fn render(
        buf: &mut Buffer,
        area: Rect,
        state: &AppState,
        panel_title: &str,
        selected_count: Option<usize>,
        file_info: Option<&FileInfo>,
        disk_space: Option<&DiskSpaceInfo>,
        editor_info: Option<&EditorInfo>,
        terminal_info: Option<&TerminalInfo>,
    ) {
        if area.height == 0 {
            return;
        }

        let status_text = Self::get_status_text(
            state,
            panel_title,
            selected_count,
            file_info,
            disk_space,
            editor_info,
            terminal_info,
            area.width,
        );

        // Fill entire line with background color from theme
        for x in area.left()..area.right() {
            buf[(x, area.top())]
                .set_char(' ')
                .set_style(Style::default().bg(state.theme.accented_bg));
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
        state: &'a AppState,
        panel_title: &'a str,
        selected_count: Option<usize>,
        file_info: Option<&'a FileInfo>,
        disk_space: Option<&'a DiskSpaceInfo>,
        editor_info: Option<&'a EditorInfo>,
        terminal_info: Option<&'a TerminalInfo>,
        total_width: u16,
    ) -> Vec<Span<'a>> {
        let t = i18n::t();

        // If there's an ERROR message, show it with priority
        // Info messages don't block file_info display
        if let Some((ref message, is_error)) = state.ui.status_message {
            if is_error {
                let msg_style = Style::default()
                    .fg(state.theme.error)
                    .add_modifier(Modifier::BOLD);

                return vec![Span::styled(format!(" {} ", message), msg_style)];
            }
        }

        let base_style = Style::default()
            .fg(state.theme.disabled)
            .bg(state.theme.accented_bg);

        let highlight_style = Style::default()
            .fg(state.theme.accented_fg)
            .bg(state.theme.accented_bg)
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

            // If there's disk information, add it on the right
            if let Some(disk) = &info.disk_space {
                let disk_text = format!(" {} ", disk.format_space());
                let disk_color = crate::ui::menu::resource_color(disk.usage_percent(), state.theme);

                // Calculate current spans width considering unicode characters
                let used_width: usize = spans
                    .iter()
                    .map(|s| match &s.content {
                        std::borrow::Cow::Borrowed(s) => s.width(),
                        std::borrow::Cow::Owned(s) => s.width(),
                    })
                    .sum();

                // Add padding between left part and disk info
                let remaining =
                    (total_width as usize).saturating_sub(used_width + disk_text.width());
                if remaining > 0 {
                    spans.push(Span::raw(" ".repeat(remaining)));
                }

                spans.push(Span::styled(
                    disk_text,
                    Style::default().fg(disk_color).bg(state.theme.accented_bg),
                ));
            }

            spans
        } else if let Some(info) = file_info {
            // File manager: show information about current file
            let mut spans = vec![];

            // Format for directories: "Dir: dirname | Mod: 0755 | Owner: nvn:users"
            // Format for files: "File: filename | 12.3MB | Mod: 0755 | Owner: nvn:users"

            if info.file_type == "Directory" {
                spans.push(Span::styled(format!(" {} ", t.status_dir()), base_style));
            } else {
                spans.push(Span::styled(format!(" {} ", t.status_file()), base_style));
            }
            spans.push(Span::styled(info.name.as_str(), highlight_style));

            // For files show size
            if info.file_type != "Directory" {
                spans.push(Span::styled(t.ui_hint_separator(), base_style));
                spans.push(Span::styled(info.size.as_str(), highlight_style));
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
                            .fg(state.theme.success)
                            .bg(state.theme.accented_bg)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }

            // If there's disk information, add it on the right
            if let Some(disk) = disk_space {
                let disk_text = format!(" {} ", disk.format_space());
                let disk_color = crate::ui::menu::resource_color(disk.usage_percent(), state.theme);

                // Calculate current spans width considering unicode characters
                let used_width: usize = spans
                    .iter()
                    .map(|s| match &s.content {
                        std::borrow::Cow::Borrowed(s) => s.width(),
                        std::borrow::Cow::Owned(s) => s.width(),
                    })
                    .sum();

                // Add padding between left part and disk info
                let remaining =
                    (total_width as usize).saturating_sub(used_width + disk_text.width());
                if remaining > 0 {
                    spans.push(Span::raw(" ".repeat(remaining)));
                }

                spans.push(Span::styled(
                    disk_text,
                    Style::default().fg(disk_color).bg(state.theme.accented_bg),
                ));
            }

            spans
        } else if let Some(info) = editor_info {
            // Editor: cursor position, tab size, encoding, file type, modes
            let mut parts = vec![
                format!("{} {}:{}", t.status_pos(), info.line, info.column),
                format!("{} {}", t.status_tab(), info.tab_size),
                info.encoding.clone(),
            ];

            // Add file type only if highlighting is enabled
            if info.syntax_highlighting {
                parts.push(info.file_type.clone());
            } else {
                parts.push(t.status_plain_text().to_string());
            }

            // Add read-only indicator
            if info.read_only {
                parts.push(t.status_readonly().to_string());
            }

            let editor_status = parts.join(t.ui_hint_separator());
            let status_width = editor_status.width();

            // Add left padding to align to right edge
            let padding = (total_width as usize).saturating_sub(status_width + 1);
            let mut spans = vec![];

            if padding > 0 {
                spans.push(Span::raw(" ".repeat(padding)));
            }

            spans.push(Span::styled(format!("{} ", editor_status), highlight_style));

            spans
        } else {
            match panel_title {
                "Terminal" => {
                    // Terminal: current directory
                    vec![
                        Span::styled(format!(" {} ", t.status_cwd()), base_style),
                        Span::styled("~/Documents/Repos", highlight_style),
                        Span::styled(
                            format!("{}{} ", t.ui_hint_separator(), t.status_shell()),
                            base_style,
                        ),
                        Span::styled("bash", highlight_style),
                    ]
                }
                "Debug" => {
                    // Debug: layout mode and dimensions
                    let layout_text = state.get_recommended_layout();
                    let terminal_info =
                        format!("{}x{}", state.terminal.width, state.terminal.height);

                    vec![
                        Span::styled(format!(" {} ", t.status_terminal()), base_style),
                        Span::styled(terminal_info, highlight_style),
                        Span::styled(
                            format!("{}{} ", t.ui_hint_separator(), t.status_layout()),
                            base_style,
                        ),
                        Span::styled(layout_text, highlight_style),
                    ]
                }
                _ => {
                    // Default: general information
                    let panel_info = format!(" {}{}", t.status_panel(), panel_title);
                    let term_info = format!(
                        "{}{}x{}",
                        t.ui_hint_separator(),
                        state.terminal.width,
                        state.terminal.height
                    );

                    vec![
                        Span::styled(panel_info, base_style),
                        Span::styled(term_info, base_style),
                    ]
                }
            }
        }
    }
}
