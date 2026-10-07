//! Panel rendering functions.
//!
//! Provides functions to render expanded and collapsed panels.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};
use std::borrow::Cow;
use std::sync::Arc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Braille spinner characters used for loading indicators.
const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Smart title truncation that preserves spinner and status.
///
/// When truncating a title like "⠋ main.rs (indexing)", this function ensures:
/// - Spinner at the start is always preserved
/// - Status in parentheses at the end is always preserved
/// - Main text in the middle is truncated with "…" at the end `cut` names
///
/// Returns the truncated title that fits within `max_width`.
/// Returns a borrowed slice when no truncation is needed, avoiding allocation.
fn smart_truncate_title(title: &str, max_width: usize, cut: TitleCut) -> Cow<'_, str> {
    let title_width = title.width();
    if title_width <= max_width {
        return Cow::Borrowed(title);
    }

    // Parse title parts: [spinner] [main_text] [(status)]
    let chars: Vec<char> = title.chars().collect();
    if chars.is_empty() {
        return Cow::Owned(String::new());
    }

    // Detect spinner prefix (braille char + space)
    let (spinner, rest_start) = if SPINNER_CHARS.contains(&chars[0]) {
        let spinner_end = if chars.len() > 1 && chars[1] == ' ' {
            2
        } else {
            1
        };
        (chars[..spinner_end].iter().collect::<String>(), spinner_end)
    } else {
        (String::new(), 0)
    };

    let rest: String = chars[rest_start..].iter().collect();

    // Detect status suffix: " (something)" at the end
    let (main_text, status) = if let Some(paren_start) = rest.rfind(" (") {
        if rest.ends_with(')') {
            (
                rest[..paren_start].to_string(),
                rest[paren_start..].to_string(),
            )
        } else {
            (rest, String::new())
        }
    } else {
        (rest, String::new())
    };

    let spinner_width = spinner.width();
    let status_width = status.width();
    let fixed_width = spinner_width + status_width;

    // If even spinner + status don't fit, just truncate everything
    if fixed_width >= max_width {
        let mut result = String::new();
        let mut width = 0;
        for ch in title.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if width + ch_width > max_width {
                break;
            }
            result.push(ch);
            width += ch_width;
        }
        return Cow::Owned(result);
    }

    // Available width for main text (with "…" if needed)
    let available_for_main = max_width - fixed_width;

    let truncated_main = cut_to_width(&main_text, available_for_main, cut);

    Cow::Owned(format!("{}{}{}", spinner, truncated_main, status))
}

/// `text` cut to `max_width` cells at the end `cut` names, with "…" marking
/// the cut when there is room for it.
fn cut_to_width(text: &str, max_width: usize, cut: TitleCut) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    // Reserve one cell for "…" unless that would leave nothing of the text.
    let (budget, ellipsis) = if max_width > 1 {
        (max_width - 1, "…")
    } else {
        (max_width, "")
    };
    let mut kept = Vec::new();
    let mut width = 0;
    let mut take = |ch: char| {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > budget {
            return false;
        }
        kept.push(ch);
        width += ch_width;
        true
    };
    match cut {
        TitleCut::Start => {
            for ch in text.chars().rev() {
                if !take(ch) {
                    break;
                }
            }
            kept.reverse();
            format!("{ellipsis}{}", kept.iter().collect::<String>())
        }
        TitleCut::End => {
            for ch in text.chars() {
                if !take(ch) {
                    break;
                }
            }
            let kept: String = kept.iter().collect();
            format!("{}{ellipsis}", kept.trim_end())
        }
    }
}

use termide_config::Config;
use termide_core::{
    attention_mark, use_emoji_icons, Panel, PanelConfig, RenderContext, ThemeColors, TitleCut,
};

/// Get emoji icon for a panel type.
///
/// Each icon must be classified as 2-cell wide by the workspace's
/// `unicode-width` fork; otherwise the title alignment after the icon
/// drifts by one column and visually swallows the trailing space (the
/// fork does not yet recognise Emoji_Presentation sequences with
/// `U+FE0F`, so emoji such as `⚙️` / `⚠️` / `🗂️` / `🖼️` falsely
/// report width 1 and must be avoided here).
pub fn panel_icon(name: &str) -> &'static str {
    match name {
        "terminal" => "💻",
        "file_manager" => "📁",
        "editor" => "📝",
        "git_status" => "📊",
        "git_log" => "📜",
        "git_diff" => "🔀",
        "image" => "🎨",
        "diagnostics" => "🚧",
        "outline" => "📑",
        "operations" => "🔄",
        "agent" => "🤖",
        _ => "📋",
    }
}
use termide_theme::Theme;

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // smart_truncate_title tests
    // =========================================================================

    #[test]
    fn test_truncate_short_title_unchanged() {
        assert_eq!(
            smart_truncate_title("main.rs", 20, TitleCut::Start),
            "main.rs"
        );
    }

    #[test]
    fn test_truncate_empty_title() {
        assert_eq!(smart_truncate_title("", 10, TitleCut::Start), "");
    }

    #[test]
    fn test_truncate_exact_fit() {
        let title = "abcde";
        assert_eq!(smart_truncate_title(title, 5, TitleCut::Start), "abcde");
    }

    #[test]
    fn test_truncate_with_spinner_prefix() {
        // Spinner char + space + title
        let title = "\u{280b} main.rs";
        let result = smart_truncate_title(title, 50, TitleCut::Start);
        assert_eq!(result, title);
    }

    #[test]
    fn test_truncate_with_status_suffix() {
        let title = "main.rs (indexing)";
        let result = smart_truncate_title(title, 50, TitleCut::Start);
        assert_eq!(result, title);
    }

    #[test]
    fn test_truncate_preserves_spinner_and_status() {
        // When title is too long, spinner and status should survive
        let title = "\u{280b} very_long_filename_that_needs_truncation.rs (indexing)";
        let result = smart_truncate_title(title, 30, TitleCut::Start);
        // Spinner should be at start
        assert!(result.starts_with('\u{280b}'));
        // Status should be at end
        assert!(result.ends_with("(indexing)"));
    }

    #[test]
    fn test_truncate_long_title_gets_ellipsis() {
        let title = "a_very_long_filename_that_exceeds_width.rs";
        let result = smart_truncate_title(title, 15, TitleCut::Start);
        assert!(result.contains('…'));
        assert!(result.len() <= title.len());
    }

    #[test]
    fn a_title_cut_at_the_end_keeps_its_beginning() {
        let title = "Agent: make the timeout configurable";
        assert_eq!(
            smart_truncate_title(title, 20, TitleCut::End),
            "Agent: make the tim…"
        );
        assert_eq!(
            smart_truncate_title(title, 20, TitleCut::Start),
            "…imeout configurable"
        );
        // A spinner and a status survive a cut at the end too.
        let busy = "\u{280b} Agent: make the timeout configurable (3)";
        let result = smart_truncate_title(busy, 20, TitleCut::End);
        assert!(result.starts_with("\u{280b} Agent"), "{result}");
        assert!(result.ends_with("… (3)"), "{result}");
        assert!(result.width() <= 20);
    }

    #[test]
    fn test_truncate_very_narrow_width() {
        let title = "main.rs";
        let result = smart_truncate_title(title, 3, TitleCut::Start);
        // Should not panic, should produce something <= 3 chars wide
        assert!(result.width() <= 3);
    }

    #[test]
    fn test_truncate_width_1() {
        let title = "main.rs";
        let result = smart_truncate_title(title, 1, TitleCut::Start);
        assert!(result.width() <= 1);
    }

    #[test]
    fn test_truncate_unicode_cjk() {
        // CJK chars are typically 2 cells wide
        let title = "\u{4f60}\u{597d}\u{4e16}\u{754c}"; // "你好世界"
        let result = smart_truncate_title(title, 4, TitleCut::Start);
        // Should fit within 4 cells (2 CJK chars)
        assert!(result.width() <= 4);
    }

    struct Waiting(bool);

    impl Panel for Waiting {
        fn name(&self) -> &'static str {
            "waiting"
        }
        fn title(&self) -> String {
            "waiting".to_string()
        }
        fn render(&mut self, _: Rect, _: &mut Buffer, _: &termide_core::RenderContext) {}
        fn handle_key(&mut self, _: termide_core::KeyChord) -> Vec<termide_core::PanelEvent> {
            vec![]
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
        fn needs_attention(&self) -> bool {
            self.0
        }
    }

    #[test]
    fn an_unfocused_panel_that_waits_shows_the_mark_in_its_icon_slot() {
        // Tests run without emoji icons: no icon unless the panel waits.
        assert_eq!(
            header_icon(&Waiting(true), false),
            Some((attention_mark(), true))
        );
        // Focused, or with nothing waiting, the slot holds no mark.
        assert_eq!(header_icon(&Waiting(true), true), None);
        assert_eq!(header_icon(&Waiting(false), false), None);
    }

    #[test]
    fn only_the_mark_takes_the_warning_colour() {
        let theme = Theme::default();
        let style = border_style(false, &theme);
        let (spans, width) = header_buttons(&Waiting(true), false, style, &theme, " ");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, format!("[≡] {} ", attention_mark()));
        assert_eq!(width, text.width());
        for span in &spans {
            let expected = if span.content == attention_mark() {
                Some(theme.warning)
            } else {
                style.fg
            };
            assert_eq!(span.style.fg, expected, "span {:?}", span.content);
        }
        // Without the mark the unicode header keeps its plain shape.
        let (spans, _) = header_buttons(&Waiting(false), false, style, &theme, " ");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[≡] ");
        let (spans, _) = header_buttons(&Waiting(false), false, style, &theme, "");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[≡]");
    }

    #[test]
    fn test_truncate_only_status_no_main() {
        // Edge case: title that's mostly status
        let title = "x (very long status message here)";
        let result = smart_truncate_title(title, 10, TitleCut::Start);
        // Should not panic
        assert!(result.width() <= 10);
    }
}

/// Render active divider during drag operation.
///
/// Only draws when a divider is being actively dragged.
/// Replaces both adjacent panel borders (right border of left panel
/// and left border of right panel) with double-line style `║`.
pub fn render_dividers(
    buf: &mut Buffer,
    divider_positions: &[(usize, u16)], // (group_idx, x_position)
    active_divider: Option<usize>,
    ghost_x: Option<u16>,
    terminal_height: u16,
    theme: &Theme,
) {
    // Only draw when actively dragging
    let Some(active_idx) = active_divider else {
        return;
    };

    // Draw from below menu (y=1) to above status bar (y=height-2)
    let start_y = 1u16;
    let end_y = terminal_height.saturating_sub(1);
    let style = Style::default().fg(theme.accented_fg);

    // If a ghost position is provided, draw the ghost divider there instead.
    // This allows visual feedback during drag without resizing panels.
    if let Some(gx) = ghost_x {
        let positions = [gx.saturating_sub(1), gx];
        for y in start_y..end_y {
            for &pos in &positions {
                if let Some(cell) = buf.cell_mut((pos, y)) {
                    cell.set_symbol("║");
                    cell.set_style(style);
                }
            }
        }
        return;
    }

    // Find and draw only the active divider at its current position
    for &(group_idx, x) in divider_positions {
        if group_idx == active_idx {
            let positions = [x.saturating_sub(1), x];
            for y in start_y..end_y {
                for &pos in &positions {
                    if let Some(cell) = buf.cell_mut((pos, y)) {
                        cell.set_symbol("║");
                        cell.set_style(style);
                    }
                }
            }
            break;
        }
    }
}

/// Render a horizontal ghost line for an in-group vertical-divider drag.
///
/// Draws a single accent-coloured `━` row at `ghost_y` spanning
/// `[start_x, end_x)`. Lightweight overlay — actual panel-height
/// resize is applied on drag-end.
pub fn render_v_divider_ghost(
    buf: &mut Buffer,
    ghost_y: u16,
    start_x: u16,
    end_x: u16,
    theme: &Theme,
) {
    let style = Style::default().fg(theme.accented_fg);
    for x in start_x..end_x {
        if let Some(cell) = buf.cell_mut((x, ghost_y)) {
            cell.set_symbol("━");
            cell.set_style(style);
        }
    }
}

/// Parameters for rendering expanded panels.
#[derive(Clone, Copy)]
pub struct ExpandedPanelParams {
    pub tab_size: usize,
    pub word_wrap: bool,
    pub terminal_width: u16,
    pub terminal_height: u16,
    /// Skip drawing the bottom border row. Used in Split mode for every
    /// panel except the last in its group: the next panel's top border
    /// (with its title) acts as a visual separator, saving one row that
    /// would otherwise be wasted on the duplicated divider line.
    pub omit_bottom_border: bool,
}

/// Style of a panel's border: accented when focused, dimmed otherwise.
fn border_style(is_focused: bool, theme: &Theme) -> Style {
    if is_focused {
        Style::default()
            .fg(theme.accented_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.disabled)
    }
}

/// The icon slot of a panel header: the attention mark while an unfocused
/// panel waits for the user, else the panel's own icon (emoji mode only).
/// The mark takes the icon's place, so the header keeps its width when an
/// emoji icon is shown; colour alone would read as focus.
fn header_icon(panel: &dyn Panel, is_focused: bool) -> Option<(&str, bool)> {
    if !is_focused && panel.needs_attention() {
        Some((attention_mark(), true))
    } else if use_emoji_icons() {
        Some((
            panel.icon().unwrap_or_else(|| panel_icon(panel.name())),
            false,
        ))
    } else {
        None
    }
}

/// Spans of the header's `[≡] icon` part, followed by `trailing` when it
/// has an icon, and their width. The attention mark is drawn in the warning
/// colour, the rest in `style`.
fn header_buttons(
    panel: &dyn Panel,
    is_focused: bool,
    style: Style,
    theme: &Theme,
    trailing: &'static str,
) -> (Vec<Span<'static>>, usize) {
    let mut spans = vec![Span::styled("[≡]", style)];
    match header_icon(panel, is_focused) {
        Some((icon, attention)) => {
            let icon_style = if attention {
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                style
            };
            spans.push(Span::styled(" ", style));
            spans.push(Span::styled(icon.to_string(), icon_style));
            spans.push(Span::styled(trailing, style));
        }
        // Without emoji the header has no icon; keep the space before the
        // title the expanded header always had.
        None => spans.push(Span::styled(
            if trailing.is_empty() { "" } else { " " },
            style,
        )),
    }
    let width = spans.iter().map(|span| span.content.width()).sum();
    (spans, width)
}

/// Render collapsed panel (header only, 1 line).
pub fn render_collapsed_panel(
    panel: &dyn Panel,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
    theme: &Theme,
    _group_size: usize,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let title = panel.title();
    let style = border_style(is_focused, theme);
    let title_style = style;

    let y = area.y;

    // Left edge
    if area.width > 0 {
        buf[(area.x, y)].set_symbol("─").set_style(style);
    }

    // Buttons: [≡] and the icon slot
    let (buttons, buttons_width) = header_buttons(panel, is_focused, style, theme, "");
    let buttons_width = buttons_width as u16;

    if area.width > 1 + buttons_width {
        buf.set_line(area.x + 1, y, &Line::from(buttons), buttons_width);
    }

    // Title (smart truncation preserving spinner and status)
    let title_start = area.x + 1 + buttons_width;
    let available_width = area.right().saturating_sub(title_start + 1) as usize;

    // Reserve 2 chars for padding spaces around title
    let content_width = available_width.saturating_sub(2);
    let truncated_title = smart_truncate_title(&title, content_width, panel.title_cut());
    let display_title = format!(" {} ", truncated_title);
    let title_width = display_title.width();

    if !display_title.is_empty() {
        buf.set_string(title_start, y, &display_title, title_style);
    }

    // Fill remaining with horizontal line
    let fill_start = title_start + title_width as u16;
    for x in fill_start..area.right() {
        buf[(x, y)].set_symbol("─").set_style(style);
    }
}

/// Render expanded panel (full border with content).
#[allow(clippy::too_many_arguments)]
pub fn render_expanded_panel(
    panel: &mut Box<dyn Panel>,
    area: Rect,
    buf: &mut Buffer,
    is_focused: bool,
    panel_index: usize,
    theme: &Theme,
    config: &Arc<Config>,
    params: ExpandedPanelParams,
    _group_size: usize,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let title = panel.title();
    let style = border_style(is_focused, theme);
    let title_style = style;

    // Create title: [≡] icon Title (with emoji) or [≡] Title (unicode mode)
    // Smart truncate title to fit within panel width
    let (mut title_spans, buttons_width) = header_buttons(&**panel, is_focused, style, theme, " ");
    // Available width: panel width - 2 (borders) - buttons - 1 (trailing space)
    let available_for_title = (area.width as usize).saturating_sub(2 + buttons_width + 1);
    let truncated_title = smart_truncate_title(&title, available_for_title, panel.title_cut());
    let title_line = panel.colorize_title(&truncated_title, title_style);
    title_spans.extend(title_line.spans);
    title_spans.push(Span::styled(" ", title_style));

    let borders = if params.omit_bottom_border {
        Borders::TOP | Borders::LEFT | Borders::RIGHT
    } else {
        Borders::ALL
    };
    let block = Block::default()
        .borders(borders)
        .border_style(style)
        .title(Line::from(title_spans));

    let inner = block.inner(area);
    block.render(area, buf);

    // Clear inner area before rendering content
    // Optimization: Single operation per cell instead of reset() + set_style()
    let clear_style = Style::default().bg(theme.bg);
    for y in inner.y..inner.y + inner.height {
        for x in inner.x..inner.x + inner.width {
            let cell = buf.cell_mut((x, y)).expect("cell in bounds");
            cell.set_char(' ');
            cell.set_style(clear_style);
        }
    }

    // Create RenderContext
    let colors = ThemeColors::from(theme);
    let panel_config = PanelConfig {
        tab_size: params.tab_size,
        word_wrap: params.word_wrap,
        show_line_numbers: true,
        show_hidden_files: false,
    };
    let ctx = RenderContext {
        theme: &colors,
        config: &panel_config,
        is_focused,
        panel_index,
        terminal_width: params.terminal_width,
        terminal_height: params.terminal_height,
        border_right_x: Some(area.x + area.width - 1),
        border_bottom_y: if params.omit_bottom_border {
            None
        } else {
            Some(area.y + area.height - 1)
        },
    };

    // Prepare panel for rendering (update cached theme/config)
    panel.prepare_render(theme, config);

    // Render panel content
    panel.render(inner, buf, &ctx);
}
