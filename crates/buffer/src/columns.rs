//! Conversions between the column units text is addressed in.
//!
//! Cursor columns count grapheme clusters, Rust strings index by byte, ropes
//! by `char`, and LSP positions count UTF-16 code units. The four agree for
//! ASCII only: `🇩🇪` is one grapheme, two chars and four code units; `é`
//! written as `e` + U+0301 is one grapheme and two chars. Every place that
//! turns one unit into another goes through these helpers, so that what a
//! cursor column points at, what gets copied and what gets deleted are the
//! same text.
//!
//! Columns past the end of the text clamp to its end.

use unicode_segmentation::UnicodeSegmentation;

/// `line` without its line ending (`\n`, `\r\n` or a lone `\r`).
pub fn trim_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// Byte offset of grapheme column `column` in `text`.
pub fn grapheme_to_byte(text: &str, column: usize) -> usize {
    text.grapheme_indices(true)
        .nth(column)
        .map_or(text.len(), |(byte, _)| byte)
}

/// Char offset of grapheme column `column` in `text`.
pub fn grapheme_to_char(text: &str, column: usize) -> usize {
    text[..grapheme_to_byte(text, column)].chars().count()
}

/// Grapheme column of byte offset `byte` in `text`; an offset inside a
/// grapheme rounds down to that grapheme's column.
pub fn byte_to_grapheme(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text.grapheme_indices(true)
        .take_while(|&(start, g)| start + g.len() <= byte)
        .count()
}

/// Grapheme column of char offset `char_idx` in `text`; an offset inside a
/// grapheme rounds down to that grapheme's column.
pub fn char_to_grapheme(text: &str, char_idx: usize) -> usize {
    let byte = text
        .char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(byte, _)| byte);
    byte_to_grapheme(text, byte)
}

/// The text between grapheme columns `start` and `end` of `text`.
pub fn grapheme_slice(text: &str, start: usize, end: usize) -> &str {
    let start = grapheme_to_byte(text, start);
    let end = grapheme_to_byte(text, end).max(start);
    &text[start..end]
}

/// Char offset of the UTF-16 offset `utf16` in `text`, the unit LSP
/// positions count. An offset inside a surrogate pair rounds down to the
/// char's start.
pub fn utf16_to_char(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (idx, c) in text.chars().enumerate() {
        let next = units + c.len_utf16();
        if next > utf16 {
            return idx;
        }
        units = next;
    }
    text.chars().count()
}

/// The grapheme just before grapheme column `column` of `text`, if any.
pub fn grapheme_before(text: &str, column: usize) -> Option<&str> {
    column.checked_sub(1).and_then(|c| grapheme_at(text, c))
}

/// The grapheme at grapheme column `column` of `text`, if any.
pub fn grapheme_at(text: &str, column: usize) -> Option<&str> {
    text.graphemes(true).nth(column)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAG: &str = "🇩🇪";
    const ACCENT: &str = "e\u{301}";

    #[test]
    fn line_endings_of_every_kind_are_trimmed() {
        assert_eq!(trim_line_ending("ab\n"), "ab");
        assert_eq!(trim_line_ending("ab\r\n"), "ab");
        assert_eq!(trim_line_ending("ab\r"), "ab");
        assert_eq!(trim_line_ending("ab"), "ab");
        assert_eq!(trim_line_ending("a\nb\n"), "a\nb");
    }

    #[test]
    fn grapheme_columns_map_to_bytes_and_chars_past_clusters() {
        let text = format!("{FLAG}x{ACCENT}y");
        assert_eq!(grapheme_to_byte(&text, 1), FLAG.len());
        assert_eq!(grapheme_to_char(&text, 1), 2);
        assert_eq!(grapheme_to_char(&text, 3), 5);
        assert_eq!(grapheme_to_char(&text, 99), 6);
        assert_eq!(grapheme_slice(&text, 1, 3), format!("x{ACCENT}"));
        assert_eq!(grapheme_slice(&text, 3, 1), "");
        assert_eq!(grapheme_at(&text, 2), Some(ACCENT));
        assert_eq!(grapheme_before(&text, 1), Some(FLAG));
        assert_eq!(grapheme_before(&text, 0), None);
    }

    #[test]
    fn offsets_inside_a_cluster_round_down() {
        let text = format!("a{FLAG}b");
        assert_eq!(char_to_grapheme(&text, 2), 1);
        assert_eq!(char_to_grapheme(&text, 3), 2);
        assert_eq!(byte_to_grapheme(&text, 2), 1);
        assert_eq!(byte_to_grapheme(&text, text.len()), 3);
    }

    #[test]
    fn utf16_offsets_map_to_chars() {
        // 'a' 1 unit, '😀' 2 units, 'ж' 1 unit.
        let text = "a😀жb";
        assert_eq!(utf16_to_char(text, 0), 0);
        assert_eq!(utf16_to_char(text, 1), 1);
        assert_eq!(utf16_to_char(text, 2), 1);
        assert_eq!(utf16_to_char(text, 3), 2);
        assert_eq!(utf16_to_char(text, 4), 3);
        assert_eq!(utf16_to_char(text, 99), 4);
    }
}
