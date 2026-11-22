//! Grapheme-level text utilities for search result rendering.
//!
//! Shared helpers for truncating and windowing text with proper
//! Unicode grapheme cluster awareness.

use ratatui::{buffer::Buffer, style::Style};
use unicode_segmentation::UnicodeSegmentation;

/// Truncate text from the start to fit within `max_chars` graphemes.
/// Adds "…" prefix if truncated.
pub fn truncate_from_start(line: &str, max_chars: usize) -> String {
    let graphemes: Vec<&str> = line.graphemes(true).collect();
    if graphemes.len() > max_chars {
        let truncated: String = graphemes[..max_chars.saturating_sub(1)].concat();
        format!("{}…", truncated)
    } else {
        line.to_string()
    }
}

/// Convert byte positions to grapheme indices.
pub fn byte_to_grapheme_indices(line: &str, byte_start: usize, byte_end: usize) -> (usize, usize) {
    let mut grapheme_start = 0;
    let mut grapheme_end = 0;
    let mut byte_pos = 0;

    for (idx, grapheme) in line.graphemes(true).enumerate() {
        if byte_pos <= byte_start {
            grapheme_start = idx;
        }
        byte_pos += grapheme.len();
        if byte_pos <= byte_end {
            grapheme_end = idx + 1;
        }
    }

    (grapheme_start, grapheme_end)
}

/// Prepare a matched line for display — center the match if the line is too long.
///
/// Returns `(display_text, match_start_grapheme, match_end_grapheme)`.
pub fn prepare_matched_line(
    line: &str,
    match_start: usize,
    match_end: usize,
    max_width: usize,
) -> (String, usize, usize) {
    let graphemes: Vec<&str> = line.graphemes(true).collect();
    let (match_start_g, match_end_g) = byte_to_grapheme_indices(line, match_start, match_end);

    if graphemes.len() <= max_width {
        return (line.to_string(), match_start_g, match_end_g);
    }

    let ellipsis_len = 1;
    let match_center = (match_start_g + match_end_g) / 2;
    let available_for_text = max_width.saturating_sub(ellipsis_len * 2);
    let half_window = available_for_text / 2;
    let window_start = match_center.saturating_sub(half_window).min(match_start_g);
    let window_end = (window_start + available_for_text).min(graphemes.len());
    let window_start = if window_end == graphemes.len() {
        graphemes.len().saturating_sub(available_for_text)
    } else {
        window_start
    };

    let needs_left_ellipsis = window_start > 0;
    let needs_right_ellipsis = window_end < graphemes.len();

    let mut result = String::new();
    let mut new_match_start = match_start_g.saturating_sub(window_start);
    let mut new_match_end = match_end_g.saturating_sub(window_start);

    if needs_left_ellipsis {
        result.push('…');
        new_match_start += ellipsis_len;
        new_match_end += ellipsis_len;
    }

    result.push_str(&graphemes[window_start..window_end].concat());

    if needs_right_ellipsis {
        result.push('…');
    }

    let result_len = result.graphemes(true).count();
    (result, new_match_start, new_match_end.min(result_len))
}

/// Display width of a single grapheme cluster in terminal columns.
///
/// Uses the sum of codepoint widths (UnicodeWidthStr), which matches what
/// the terminal's wcwidth() returns for Spacing Combining Marks (Mc).
/// Mc characters like Devanagari ा (U+093E) have wcwidth=1 and advance the
/// terminal cursor — so a cluster like "भा" occupies 2 terminal columns.
/// Non-spacing marks (Mn) like ु (U+0941) have wcwidth=0 and are truly
/// zero-width. CJK wide characters (e.g. 漢) return 2 as expected.
/// Falls back to 1 for clusters that consist entirely of zero-width codepoints.
///
/// This is the measure of single-line widgets (menus, dropdowns) drawn with
/// [`render_text_cells`]. The editor measures with
/// `termide_buffer::grapheme_columns` instead, which expands tabs to tab
/// stops and keeps zero-width clusters at zero.
pub fn grapheme_display_width(g: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    let w = g.width();
    if w > 0 {
        w
    } else {
        1
    }
}

/// The symbol a buffer cell may hold for grapheme cluster `g`.
///
/// A control character must never reach the host terminal inside a cell:
/// a TAB jumps the cursor to the next tab stop, CR returns it to column 0
/// and LF moves it down (scrolling the whole screen on the last row), so
/// everything written after it on that frame lands elsewhere than ratatui's
/// diff assumes, and the diff never repaints the displaced cells. The width
/// tables give a control character one column, so it keeps that column here
/// as a blank.
pub fn cell_symbol(g: &str) -> &str {
    if g.contains(char::is_control) {
        " "
    } else {
        g
    }
}

/// `text` with every TAB expanded to blanks up to the next multiple of
/// `tab_size`, columns counted from the start of `text` in display widths.
///
/// For views that draw text through `Buffer::set_string` or a `Span`: those
/// drop control characters, so a tab-indented line would lose its
/// indentation. The tab stop rule is the editor's
/// (`termide_buffer::grapheme_columns`); `tab_size` 0 is treated as 1.
/// Borrows `text` when it holds no tab.
pub fn expand_tabs(text: &str, tab_size: usize) -> std::borrow::Cow<'_, str> {
    use unicode_width::UnicodeWidthStr;
    if !text.contains('\t') {
        return std::borrow::Cow::Borrowed(text);
    }
    let tab_size = tab_size.max(1);
    let mut out = String::with_capacity(text.len() + tab_size);
    let mut col = 0;
    for g in text.graphemes(true) {
        if g == "\t" {
            let n = tab_size - col % tab_size;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push_str(g);
            col += g.width();
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Display width of a string, counting each grapheme cluster via `grapheme_display_width`.
pub fn str_display_width(s: &str) -> usize {
    s.graphemes(true).map(grapheme_display_width).sum()
}

/// Render text into buffer cells with correct complex-script support.
///
/// Iterates Unicode codepoints rather than grapheme clusters:
/// - Spacing characters (wcwidth ≥ 1) each occupy their own cell.
/// - Zero-width characters (Mn combining marks, wcwidth = 0) are appended
///   to the previous cell's symbol so the terminal font can shape them
///   together with the base character without creating a skip cell.
/// - Wide characters (CJK, wcwidth = 2) write a space to the following cell.
///
/// This avoids the ratatui diff `to_skip` bug where a continuation cell is
/// never output even when it held non-empty content in the previous frame,
/// which caused persistent Mc-symbol artifacts during redraws in Indic locales.
///
/// Returns the number of columns consumed.
pub fn render_text_cells(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    text: &str,
    max_cols: u16,
    style: Style,
) -> u16 {
    use unicode_width::UnicodeWidthChar;
    let mut col = 0u16;
    for ch in text.chars() {
        // A control character takes the one blank column `str_display_width`
        // counts for it (see `cell_symbol`), never the previous cell's symbol.
        let control = ch.is_control();
        let cw = if control {
            1
        } else {
            UnicodeWidthChar::width(ch).unwrap_or(0) as u16
        };
        if cw == 0 {
            // Zero-width (Mn combining marks, etc.): append to the last written cell
            // so the terminal font can shape them with their base character.
            if col > 0 {
                let cell = &mut buf[(x + col - 1, y)];
                let mut sym = cell.symbol().to_string();
                sym.push(ch);
                cell.set_symbol(&sym);
            }
        } else {
            if col + cw > max_cols {
                break;
            }
            let cell = &mut buf[(x + col, y)];
            // Stack-encode the char instead of allocating a String per glyph.
            let mut enc = [0u8; 4];
            cell.set_symbol(if control {
                " "
            } else {
                ch.encode_utf8(&mut enc)
            });
            cell.set_style(style);
            if cw == 2 && col + 1 < max_cols {
                // Fill second cell of CJK wide char so it doesn't bleed
                let next = &mut buf[(x + col + 1, y)];
                next.set_symbol(" ");
                next.set_style(style);
            }
            col += cw;
        }
    }
    col
}

/// Draw a single-line input field's `text` from character `scroll` on, in at
/// most `width` cells from `(x, y)`, one grapheme cluster at a time as
/// [`crate::text_utils::field_clusters`] measures them: a letter with its
/// combining accents is one cell, never a blank one beside it. `style(at,
/// end)` gives the style of the cluster of characters `[at, end)`, for the
/// cursor and the selection. Returns the columns drawn.
pub fn render_field_text(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    scroll: usize,
    style: impl Fn(usize, usize) -> Style,
) -> u16 {
    let mut col = 0u16;
    for (at, g, w) in crate::text_utils::field_clusters(text) {
        if at < scroll {
            continue;
        }
        let w = w as u16;
        if col + w > width {
            break;
        }
        let style = style(at, at + g.chars().count());
        // Clear the cells first: a cluster of marks alone draws nothing of
        // its own, and its cell must not keep what was there.
        for dx in 0..w {
            buf[(x + col + dx, y)].set_symbol(" ").set_style(style);
        }
        render_text_cells(buf, x + col, y, g, w, style);
        col += w;
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_draws_a_letter_and_its_accent_in_one_cell() {
        use ratatui::layout::Rect;
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        // "e" + COMBINING ACUTE, as a macOS file name spells é.
        let cols = render_field_text(&mut buf, 0, 0, 6, "ce\u{301}x", 0, |_, _| Style::default());
        assert_eq!(cols, 3);
        assert_eq!(buf[(0, 0)].symbol(), "c");
        assert_eq!(buf[(1, 0)].symbol(), "e\u{301}");
        assert_eq!(buf[(2, 0)].symbol(), "x");
    }

    #[test]
    fn test_devanagari_grapheme_width() {
        // Mc (Spacing Combining Marks) have wcwidth=1 in the terminal — they advance the
        // cursor just like base consonants. So "थी" cluster = थ(1) + ी(Mc,1) = 2 terminal cols.
        assert_eq!(str_display_width("थीम"), 3); // थी(Mc→2) + म(1) = 3
        assert_eq!(str_display_width("भाषा"), 4); // भा(Mc→2) + षा(Mc→2) = 4
                                                  // Mn (Non-spacing marks) have wcwidth=0 — truly zero-width, no cursor advance.
                                                  // े (U+0947) and ै (U+0948) are Mn, so the cluster "ह+े" = 1 terminal col.
        assert_eq!(str_display_width("है"), 1); // ह(1) + े(Mn,0) = 1
        assert_eq!(str_display_width("हैं"), 1); // ह(1) + ै(Mn,0) + ं(Mn,0) = 1
        assert_eq!(str_display_width("करें"), 2); // क(1) + र+े+ं cluster(1, all Mn) = 2
                                                // Bengali: U+09BE (া) has Grapheme_Extend but wcwidth=1 — must be treated as width 1.
        assert_eq!(str_display_width("ভাষা"), 4); // ভা(Mc→2) + ষা(Mc→2) = 4
        assert_eq!(str_display_width("বাংলা"), 5); // ব(1)+া(Mc,1)+ং(Mc,1)+ল(1)+া(Mc,1) = 5
                                                 // CJK must remain 2-wide
        assert_eq!(str_display_width("漢"), 2);
        assert_eq!(str_display_width("日本"), 4);
        // Basic ASCII unaffected
        assert_eq!(str_display_width("abc"), 3);
    }

    /// Control characters are blanks of one column, both counted and drawn,
    /// never glued onto the previous cell (#55).
    #[test]
    fn control_characters_render_as_one_blank_cell() {
        assert_eq!(cell_symbol("\t"), " ");
        assert_eq!(cell_symbol("\r\n"), " ");
        assert_eq!(cell_symbol("a"), "a");
        assert_eq!(str_display_width("a\tb"), 3);

        let area = ratatui::layout::Rect::new(0, 0, 6, 1);
        let mut buf = Buffer::empty(area);
        let cols = render_text_cells(&mut buf, 0, 0, "a\tb\nc", 6, Style::default());
        assert_eq!(cols, 5);
        let row: String = (0..5).map(|x| buf[(x, 0)].symbol()).collect();
        assert_eq!(row, "a b c");
    }

    #[test]
    fn tabs_expand_to_the_next_tab_stop() {
        assert_eq!(expand_tabs("\tx", 4), "    x");
        assert_eq!(expand_tabs("ab\tx", 4), "ab  x");
        assert_eq!(expand_tabs("漢\tx", 4), "漢  x");
        assert_eq!(expand_tabs("a\t\tb", 8), format!("a{}b", " ".repeat(15)));
        assert!(matches!(
            expand_tabs("no tabs", 4),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
