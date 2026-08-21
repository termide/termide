//! Rendering utilities for operations panel.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use termide_ui::path_utils::truncate_left;

use crate::OperationSnapshot;

/// Format bytes to human-readable string
pub fn format_bytes(bytes: u64) -> String {
    let t = termide_i18n::t();
    if bytes >= 1_073_741_824 {
        format!(
            "{:.1}{}",
            bytes as f64 / 1_073_741_824.0,
            t.size_gigabytes()
        )
    } else if bytes >= 1_048_576 {
        format!("{:.1}{}", bytes as f64 / 1_048_576.0, t.size_megabytes())
    } else if bytes >= 1024 {
        format!("{:.1}{}", bytes as f64 / 1024.0, t.size_kilobytes())
    } else {
        format!("{}{}", bytes, t.size_bytes())
    }
}

/// Get icon for operation type
fn op_type_icon(op_type: &termide_state::OperationType) -> &'static str {
    match op_type {
        termide_state::OperationType::Copy => "\u{29C9}", // ⧉
        termide_state::OperationType::Move => "\u{279C}", // ➜
        termide_state::OperationType::Rename => "\u{270E}", // ✎
        termide_state::OperationType::CopyUpload => "\u{2191}", // ↑
        termide_state::OperationType::CopyDownload => "\u{2193}", // ↓
        termide_state::OperationType::MoveUpload => "\u{2191}", // ↑
        termide_state::OperationType::MoveDownload => "\u{2193}", // ↓
        termide_state::OperationType::Delete => "\u{2715}", // ✕
        termide_state::OperationType::CommandBackground => "\u{2699}", // ⚙
        termide_state::OperationType::CommandReport => "\u{1F4CB}", // 📋
    }
}

/// Get localized label for operation type
fn op_type_label(op_type: &termide_state::OperationType) -> &str {
    let t = termide_i18n::t();
    match op_type {
        termide_state::OperationType::Copy => t.progress_copy_title(),
        termide_state::OperationType::Move => t.progress_move_title(),
        termide_state::OperationType::Rename => t.op_type_rename(),
        termide_state::OperationType::CopyUpload => t.op_type_copy_upload(),
        termide_state::OperationType::CopyDownload => t.op_type_copy_download(),
        termide_state::OperationType::MoveUpload => t.op_type_move_upload(),
        termide_state::OperationType::MoveDownload => t.op_type_move_download(),
        termide_state::OperationType::Delete => t.progress_delete_title(),
        termide_state::OperationType::CommandBackground => t.op_type_command(),
        termide_state::OperationType::CommandReport => t.op_type_command(),
    }
}

/// Render a single operation card from a snapshot.
fn render_snapshot_card(
    op: &OperationSnapshot,
    area: Rect,
    buf: &mut Buffer,
    is_selected: bool,
    fg_color: Color,
    accent_color: Color,
    disabled_color: Color,
) {
    let border_color = if is_selected {
        accent_color
    } else {
        disabled_color
    };

    let t = termide_i18n::t();
    let is_command = op.op_type.is_command();
    let is_scanning = op.is_scanning;
    let has_data = !is_scanning && op.op_type.has_data_progress();

    // Build border title: " icon Label " or " icon Label ── 45% "
    let icon = if is_scanning {
        "\u{25CE}" // ◎
    } else {
        op_type_icon(&op.op_type)
    };
    let type_label = if is_scanning {
        t.op_type_scanning()
    } else {
        op_type_label(&op.op_type)
    };

    let title_style = Style::default()
        .fg(if is_selected { accent_color } else { fg_color })
        .add_modifier(Modifier::BOLD);

    // ⏸ goes before the label so it lines up with the bracketed icon —
    // e.g. `[↑] ⏸ Copy (upload)`. The trailing space is part of the
    // prefix so the unpaused form collapses cleanly to `[↑] Copy …`.
    let pause_prefix = if op.is_paused { "\u{23F8} " } else { "" }; // ⏸

    // Bracket the type icon to mirror the panel `[≡]` menu button —
    // signals "this is clickable / opens a context menu".
    let icon_button = format!("[{}]", icon);

    let title = if is_command {
        // Command: show command name in title instead of generic "Command" label
        Line::from(Span::styled(
            format!(" {} {}{} ", icon_button, pause_prefix, op.source),
            title_style,
        ))
    } else {
        let percent = format!("{}%", op.progress.percent());
        Line::from(vec![
            Span::styled(
                format!(" {} {}{} ", icon_button, pause_prefix, type_label),
                title_style,
            ),
            Span::styled(
                format!("{} ", percent),
                Style::default()
                    .fg(accent_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height < 1 || inner.width < 10 {
        return;
    }

    let content_width = inner.width as usize;

    // Static buffers for progress bar (avoids per-frame allocation)
    const FILLED: &str = "████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████████";
    const EMPTY: &str = "░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░";

    let mut y = inner.y;

    // Line 1: Progress bar (file operations only)
    if !is_command {
        let bar_width = content_width;
        let percent_val = op.progress.percent() as usize;
        let filled = (bar_width * percent_val) / 100;
        let empty = bar_width.saturating_sub(filled);
        let filled_part = &FILLED[..filled.min(FILLED.len() / 3) * 3]; // █ is 3 bytes
        let empty_part = &EMPTY[..empty.min(EMPTY.len() / 3) * 3]; // ░ is 3 bytes
        let bar_color = if op.is_paused {
            accent_color
        } else {
            Color::Green
        };
        let bar_line = Line::from(vec![
            Span::styled(filled_part, Style::default().fg(bar_color)),
            Span::styled(empty_part, Style::default().fg(bar_color)),
        ]);
        buf.set_line(inner.x, y, &bar_line, inner.width);
        y += 1;
    }

    // Source path (file ops only — command name is in border title)
    if !is_command {
        let source = truncate_left(&op.source, content_width);
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(source, Style::default().fg(disabled_color))),
            inner.width,
        );
        y += 1;
    }

    // Destination path (truncate left) — only if present
    if !op.dest.is_empty() {
        let dest = truncate_left(&op.dest, content_width);
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(dest, Style::default().fg(disabled_color))),
            inner.width,
        );
        y += 1;
    }

    // Files count (skip for commands, during scanning show "Found: N")
    if !is_command {
        let files = if is_scanning {
            t.op_found_count(op.progress.total_files)
        } else {
            t.op_files_progress(op.progress.files_completed, op.progress.total_files)
        };
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(files, Style::default().fg(fg_color))),
            inner.width,
        );
        y += 1;
    }

    // Data (only for transfer operations, not during scanning)
    if has_data {
        let data = t.op_data_progress(
            &format_bytes(op.progress.bytes_transferred),
            &format_bytes(op.progress.total_bytes),
        );
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(data, Style::default().fg(fg_color))),
            inner.width,
        );
        y += 1;
    }

    // Speed (only for transfer operations, not during scanning)
    if has_data {
        let speed = t.op_speed_rate(&format_bytes(op.speed as u64));
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(speed, Style::default().fg(fg_color))),
            inner.width,
        );
        y += 1;
    }

    // Elapsed time (for all operations)
    {
        let elapsed = op.started_at.elapsed().as_secs();
        let elapsed_display = t.op_elapsed(&termide_i18n::compact_duration(elapsed));
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(elapsed_display, Style::default().fg(fg_color))),
            inner.width,
        );
    }
}

/// Render the full operations panel using operation snapshots.
/// This version uses OperationSnapshot instead of ActiveOperation references.
pub fn render_operations_panel_snapshots(
    operations: &[OperationSnapshot],
    selected_index: usize,
    scroll_offset: usize,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
    fg_color: Color,
    accent_color: Color,
    disabled_color: Color,
) -> Vec<(usize, Rect)> {
    let mut card_areas = Vec::new();

    if operations.is_empty() {
        // Render "No active operations" message
        let t = termide_i18n::t();
        let text = ratatui::widgets::Paragraph::new(Line::from(t.no_active_operations()))
            .style(Style::default().fg(disabled_color))
            .alignment(ratatui::layout::Alignment::Center);
        text.render(area, buf);
        return card_areas;
    }

    // Calculate how many cards fit by summing their individual heights
    let mut y_offset: u16 = 0;

    for (i, op) in operations.iter().skip(scroll_offset).enumerate() {
        let card_h = op.card_height();
        if y_offset + card_h > area.height {
            break;
        }

        let card_area = Rect {
            x: area.x,
            y: area.y + y_offset,
            width: area.width,
            height: card_h.min(area.height - y_offset),
        };

        let op_index = scroll_offset + i;
        let is_selected = is_focused && op_index == selected_index;

        render_snapshot_card(
            op,
            card_area,
            buf,
            is_selected,
            fg_color,
            accent_color,
            disabled_color,
        );

        card_areas.push((op_index, card_area));
        y_offset += card_h;
    }

    let visible_cards = card_areas.len();

    // Render scroll indicators if needed
    if scroll_offset > 0 {
        // Show "↑" indicator at top
        let indicator = Span::styled("\u{25B2}", Style::default().fg(accent_color)); // ▲
        buf.set_span(area.x + area.width - 3, area.y, &indicator, 1);
    }

    if scroll_offset + visible_cards < operations.len() {
        // Show "↓" indicator at bottom
        let indicator = Span::styled("\u{25BC}", Style::default().fg(accent_color)); // ▼
        buf.set_span(
            area.x + area.width - 3,
            area.y + area.height - 1,
            &indicator,
            1,
        );
    }

    card_areas
}
