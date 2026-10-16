//! Pure text helpers for the document preview panels: character slicing,
//! display-column geometry, substring search, and URL inspection; and the
//! horizontal scroll of single-line input fields.
//!
//! The HTML and Markdown previews were carrying byte-identical copies of
//! these. They hold no state, so they live here beside the other shared
//! helper modules rather than once per panel.

use unicode_width::UnicodeWidthChar;

/// The `#fragment` part of a URL, if present and non-empty.
pub fn url_fragment(url: &str) -> Option<String> {
    url.split_once('#')
        .map(|(_, f)| f.to_string())
        .filter(|f| !f.is_empty())
}

/// Whether `path` (or URL) ends in a known raster-image extension.
pub fn is_image_path(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif"
    )
}

/// Substring of `s` between character indices `[start, end)`.
pub fn slice_chars(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Display column at character index `col` (sum of preceding char widths).
pub fn char_col_to_display(s: &str, col: usize) -> usize {
    s.chars().take(col).map(|c| c.width().unwrap_or(0)).sum()
}

/// Character index at (or just past) display column `disp`.
pub fn display_to_char_col(s: &str, disp: u16) -> usize {
    let target = disp as usize;
    let mut acc = 0usize;
    for (i, c) in s.chars().enumerate() {
        if acc >= target {
            return i;
        }
        acc += c.width().unwrap_or(0);
    }
    s.chars().count()
}

/// Character indices where `needle` occurs in `line` (case-insensitive when `ci`).
pub fn find_in_line(line: &str, needle: &str, ci: bool) -> Vec<usize> {
    let hay: Vec<char> = line.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if pat.is_empty() || pat.len() > hay.len() {
        return out;
    }
    let eq = |a: char, b: char| {
        if ci {
            a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase())
        } else {
            a == b
        }
    };
    for i in 0..=hay.len() - pat.len() {
        if (0..pat.len()).all(|j| eq(hay[i + j], pat[j])) {
            out.push(i);
        }
    }
    out
}

/// Display width of `c` in an input field: a character with no width of its
/// own still takes a cell.
fn field_char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(1)
}

/// The character of `text` under display column `x`, counting wide
/// characters as two; past the end it is the text length.
pub fn char_at_x(text: &str, x: usize) -> usize {
    let mut width = 0;
    for (i, c) in text.chars().enumerate() {
        let cw = field_char_width(c);
        if width + cw > x {
            return i;
        }
        width += cw;
    }
    text.chars().count()
}

/// How many characters of `text` a field `width` cells wide scrolls past to
/// keep the cursor at `cursor_pos` in view: none while it fits, else just
/// enough to put it at the right edge.
pub fn input_scroll_offset(text: &str, cursor_pos: usize, width: usize) -> usize {
    let widths: Vec<usize> = text.chars().map(field_char_width).collect();
    if widths.iter().sum::<usize>() < width {
        return 0;
    }
    let cursor_x: usize = widths.iter().take(cursor_pos).sum();
    if cursor_x < width {
        return 0;
    }
    let mut skipped = 0;
    for (index, cw) in widths.iter().enumerate() {
        if cursor_x - skipped < width {
            return index;
        }
        skipped += cw;
    }
    0
}

/// The scroll of a field `width` cells wide after its cursor moved to
/// `cursor_pos`, from the `scroll` it had: kept while the cursor stays in view,
/// else moved just enough to bring it back, and never past what shows the end
/// of the text. A field that keeps its scroll this way does not jump when a
/// click places the cursor.
pub fn follow_input_scroll(text: &str, cursor_pos: usize, width: usize, scroll: usize) -> usize {
    let to_cursor = input_scroll_offset(text, cursor_pos, width);
    let to_end = input_scroll_offset(text, text.chars().count(), width);
    let scroll = if cursor_pos < scroll {
        cursor_pos
    } else {
        scroll.max(to_cursor)
    };
    scroll.min(to_end)
}

/// Whether `c` belongs to a word for a double click: letters and digits of any
/// script and `_`, the editor's rule (`termide_buffer::is_word_boundary`), so
/// a double click takes all of a `snake_case` name in every field.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word of `chars` around character `col`, as a `(start, end)` character
/// range, for a double click. A column past the end means the last character.
/// `None` when that character is not part of a word (a space, a dot) or the
/// text is empty, the editor's rule too.
pub fn word_range_at(chars: &[char], col: usize) -> Option<(usize, usize)> {
    let col = col.min(chars.len().checked_sub(1)?);
    if !is_word_char(chars[col]) {
        return None;
    }
    let start = chars[..col]
        .iter()
        .rposition(|&c| !is_word_char(c))
        .map_or(0, |i| i + 1);
    let end = chars[col..]
        .iter()
        .position(|&c| !is_word_char(c))
        .map_or(chars.len(), |i| col + i);
    Some((start, end))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_field_scroll_holds_while_the_cursor_stays_in_view() {
        let text = "x".repeat(30);
        // The cursor at the end of 30 characters in a 10-cell field: 21
        // scrolled out, the cursor cell last.
        assert_eq!(input_scroll_offset(&text, 30, 10), 21);
        assert_eq!(follow_input_scroll(&text, 30, 10, 0), 21);
        // Moved within view, the scroll stays.
        assert_eq!(follow_input_scroll(&text, 25, 10, 21), 21);
        // Left of view, the cursor becomes the first character shown.
        assert_eq!(follow_input_scroll(&text, 5, 10, 21), 5);
        // Never past what shows the end of the text.
        assert_eq!(follow_input_scroll(&text[..15], 15, 10, 21), 6);
        // Wide characters count two cells.
        assert_eq!(char_at_x("ab世c", 3), 2);
        assert_eq!(char_at_x("ab世c", 4), 3);
    }
    use super::*;

    #[test]
    fn find_in_line_case_insensitive() {
        assert_eq!(find_in_line("Foo foo FOO", "foo", true), vec![0, 4, 8]);
        assert_eq!(find_in_line("Foo foo FOO", "foo", false), vec![4]);
    }

    #[test]
    fn find_in_line_handles_degenerate_input() {
        assert!(find_in_line("abc", "", false).is_empty());
        assert!(find_in_line("ab", "abc", false).is_empty());
        assert!(find_in_line("", "a", false).is_empty());
    }

    #[test]
    fn find_in_line_matches_non_ascii_case_insensitively() {
        // The ASCII fast path cannot fold these, so the fallback has to.
        assert_eq!(find_in_line("Привет привет", "ПРИВЕТ", true), vec![0, 7]);
    }

    #[test]
    fn slicing_counts_characters_not_bytes() {
        assert_eq!(slice_chars("привет", 1, 4), "рив");
        // Out-of-range and inverted ranges clamp instead of panicking.
        assert_eq!(slice_chars("ab", 1, 99), "b");
        assert_eq!(slice_chars("ab", 2, 1), "");
    }

    #[test]
    fn display_columns_account_for_wide_characters() {
        // A CJK character occupies two display columns but one char index.
        assert_eq!(char_col_to_display("日本語", 2), 4);
        assert_eq!(display_to_char_col("日本語", 4), 2);
        // A column landing inside a wide character resolves past it, which is
        // what a click on the glyph's right half should do.
        assert_eq!(display_to_char_col("日本語", 3), 2);
        // Past the end clamps to the character count.
        assert_eq!(display_to_char_col("ab", 99), 2);
        assert_eq!(char_col_to_display("ab", 99), 2);
    }

    #[test]
    fn url_fragment_ignores_a_bare_hash() {
        assert_eq!(url_fragment("page.html#top"), Some("top".to_string()));
        assert_eq!(url_fragment("page.html#"), None);
        assert_eq!(url_fragment("page.html"), None);
    }

    #[test]
    fn image_paths_are_matched_by_extension_case_insensitively() {
        assert!(is_image_path("a/b/c.PNG"));
        assert!(is_image_path("https://host/x.jpeg"));
        assert!(!is_image_path("notes.md"));
        assert!(!is_image_path("noextension"));
    }
}
