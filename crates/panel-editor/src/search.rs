//! Search and replace operations for the editor.
//!
//! This module provides utilities for searching through text, navigating matches,
//! and performing find-and-replace operations. Both literal and regular-expression
//! matching are supported (`SearchState::use_regex`); in regex mode the
//! replacement string may reference capture groups via `$1` / `${name}`.

use anyhow::Result;
use regex::{Regex, RegexBuilder};
use unicode_segmentation::UnicodeSegmentation;

use termide_buffer::{Cursor, SearchState, Selection, TextBuffer};

/// Build a regex for `query`, honoring case sensitivity. Returns `None` when
/// the pattern is invalid — callers treat that as "no matches".
fn build_regex(query: &str, case_sensitive: bool) -> Option<Regex> {
    RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .build()
        .ok()
}

/// Byte offset of grapheme index `g` within `line`, or `line.len()` if `g` is
/// at/after the end.
fn grapheme_byte(line: &str, g: usize) -> usize {
    line.grapheme_indices(true)
        .nth(g)
        .map(|(b, _)| b)
        .unwrap_or(line.len())
}

/// Perform search through the entire buffer.
///
/// Populates `matches` (start of each hit) and `match_lengths` (grapheme
/// length of each hit, needed because regex matches vary in length).
pub fn perform_search(buffer: &TextBuffer, search: &mut SearchState) {
    search.matches.clear();
    search.match_lengths.clear();

    if search.query.is_empty() {
        return;
    }

    if search.use_regex {
        let Some(re) = build_regex(&search.query, search.case_sensitive) else {
            return;
        };
        for line_idx in 0..buffer.line_count() {
            if let Some(line_text) = buffer.line_cow(line_idx) {
                for m in re.find_iter(&line_text) {
                    // Skip zero-width matches (e.g. `a*` against "") — they
                    // can't be replaced meaningfully and add noise.
                    if m.start() == m.end() {
                        continue;
                    }
                    let column = line_text[..m.start()].graphemes(true).count();
                    let len = line_text[m.start()..m.end()].graphemes(true).count();
                    search.matches.push(Cursor {
                        line: line_idx,
                        column,
                    });
                    search.match_lengths.push(len);
                }
            }
        }
        return;
    }

    // Literal search. Case-insensitivity is done by lowercasing both sides.
    let query = if search.case_sensitive {
        search.query.clone()
    } else {
        search.query.to_lowercase()
    };
    for line_idx in 0..buffer.line_count() {
        if let Some(line_text) = buffer.line_cow(line_idx) {
            let search_text = if search.case_sensitive {
                line_text
            } else {
                std::borrow::Cow::Owned(line_text.to_lowercase())
            };

            let mut byte_col = 0;
            while let Some(byte_pos) = search_text[byte_col..].find(&query) {
                let match_byte_col = byte_col + byte_pos;
                let match_grapheme_col = search_text[..match_byte_col].graphemes(true).count();
                search.matches.push(Cursor {
                    line: line_idx,
                    column: match_grapheme_col,
                });
                // Lengths are in graphemes, the unit cursors and deletions
                // use, so `नमस्ते` or `👍🏽` count as the clusters they are.
                let match_end = match_byte_col + query.len();
                search.match_lengths.push(
                    search_text[match_byte_col..match_end]
                        .graphemes(true)
                        .count(),
                );
                if let Some(first_char) = search_text[match_byte_col..].chars().next() {
                    byte_col = match_byte_col + first_char.len_utf8();
                } else {
                    break;
                }
            }
        }
    }
}

/// Get selection for a search match.
///
/// Returns (Selection, end_cursor) for highlighting a match of `match_len`
/// graphemes starting at `match_cursor`.
pub fn get_match_selection(match_cursor: &Cursor, match_len: usize) -> (Selection, Cursor) {
    let end_cursor = Cursor::at(match_cursor.line, match_cursor.column + match_len);
    (Selection::new(*match_cursor, end_cursor), end_cursor)
}

/// Compute the text to insert for a single match.
///
/// In regex mode the matched text is re-run through the pattern so `$1` /
/// `${name}` references in `replace_with` resolve. In literal mode the
/// replacement is returned verbatim (`$` has no special meaning).
pub fn expand_replacement(
    buffer: &TextBuffer,
    match_cursor: &Cursor,
    match_len: usize,
    search: &SearchState,
) -> String {
    let replace_with = search.replace_with.clone().unwrap_or_default();
    if !search.use_regex {
        return replace_with;
    }
    let Some(re) = build_regex(&search.query, search.case_sensitive) else {
        return replace_with;
    };
    let Some(line) = buffer.line_cow(match_cursor.line) else {
        return replace_with;
    };
    let start = grapheme_byte(&line, match_cursor.column);
    let end = grapheme_byte(&line, match_cursor.column + match_len);
    let matched = &line[start..end];
    re.replace(matched, replace_with.as_str()).into_owned()
}

/// Replace result information.
pub struct ReplaceResult {
    pub new_cursor: Cursor,
    pub start_line: usize,
}

/// Replace `match_len` graphemes at `match_cursor` with `replace_text`.
///
/// `replace_text` must already be the final text to insert (see
/// [`expand_replacement`] for regex capture-group expansion).
pub fn replace_at_position(
    buffer: &mut TextBuffer,
    match_cursor: &Cursor,
    match_len: usize,
    replace_text: &str,
) -> Result<ReplaceResult> {
    let end_cursor = Cursor {
        line: match_cursor.line,
        column: match_cursor.column + match_len,
    };

    buffer.delete_range(match_cursor, &end_cursor)?;
    buffer.insert(match_cursor, replace_text)?;

    let new_cursor = Cursor {
        line: match_cursor.line,
        column: match_cursor.column + replace_text.graphemes(true).count(),
    };

    Ok(ReplaceResult {
        new_cursor,
        start_line: match_cursor.line,
    })
}

/// Update match positions after a replacement.
///
/// Adjusts the positions of matches that come after the replacement point
/// on the same line by the length delta `inserted_len - match_len`.
pub fn update_match_positions_after_replace(
    matches: &mut [Cursor],
    match_cursor: &Cursor,
    match_len: usize,
    inserted_len: usize,
) {
    let replacement_offset = inserted_len as isize - match_len as isize;
    if replacement_offset != 0 {
        for match_pos in matches.iter_mut() {
            if match_pos.line == match_cursor.line && match_pos.column > match_cursor.column {
                match_pos.column = (match_pos.column as isize + replacement_offset).max(0) as usize;
            }
        }
    }
}

/// Replace all matches in `search` in reverse order.
///
/// Reverse order keeps earlier positions valid as later ones change.
/// Per-match lengths and regex capture-group expansion are honored.
/// Returns the number of replacements made.
pub fn replace_all_matches(buffer: &mut TextBuffer, search: &SearchState) -> Result<usize> {
    let mut count = 0;

    for idx in (0..search.matches.len()).rev() {
        let match_cursor = search.matches[idx];
        let match_len = search.match_len_at(idx);
        let replace_text = expand_replacement(buffer, &match_cursor, match_len, search);

        let end_cursor = Cursor {
            line: match_cursor.line,
            column: match_cursor.column + match_len,
        };
        buffer.delete_range(&match_cursor, &end_cursor)?;
        buffer.insert(&match_cursor, &replace_text)?;

        count += 1;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_buffer::TextBuffer;

    fn buf(text: &str) -> TextBuffer {
        let mut b = TextBuffer::new();
        b.insert(&Cursor { line: 0, column: 0 }, text).unwrap();
        b
    }

    #[test]
    fn literal_search_records_lengths() {
        let b = buf("foo bar foo");
        let mut s = SearchState::new("foo".to_string(), true);
        perform_search(&b, &mut s);
        assert_eq!(s.matches.len(), 2);
        assert_eq!(s.match_lengths, vec![3, 3]);
    }

    #[test]
    fn regex_search_varies_lengths() {
        let b = buf("ab abc abcd");
        let mut s = SearchState::new("abc?d?".to_string(), true).with_regex(true);
        perform_search(&b, &mut s);
        // "ab", "abc", "abcd"
        assert_eq!(s.matches.len(), 3);
        assert_eq!(s.match_lengths, vec![2, 3, 4]);
    }

    #[test]
    fn regex_replace_expands_capture_groups() {
        let mut b = buf("get_user(id)");
        let mut s =
            SearchState::new_with_replace(r"get_(\w+)".to_string(), "fetch_$1".to_string(), true)
                .with_regex(true);
        perform_search(&b, &mut s);
        assert_eq!(s.matches.len(), 1);
        replace_all_matches(&mut b, &s).unwrap();
        assert_eq!(b.line_cow(0).unwrap(), "fetch_user(id)");
    }

    #[test]
    fn regex_case_insensitive() {
        let b = buf("FOO foo Foo");
        let mut s = SearchState::new("foo".to_string(), false).with_regex(true);
        perform_search(&b, &mut s);
        assert_eq!(s.matches.len(), 3);
    }

    #[test]
    fn literal_replacement_is_verbatim() {
        let mut b = buf("a.b");
        // In literal mode, '.' matches a literal dot and '$1' is verbatim.
        let mut s = SearchState::new_with_replace(".".to_string(), "$1".to_string(), true);
        perform_search(&b, &mut s);
        assert_eq!(s.matches.len(), 1);
        replace_all_matches(&mut b, &s).unwrap();
        assert_eq!(b.line_cow(0).unwrap(), "a$1b");
    }

    #[test]
    fn literal_match_lengths_count_graphemes() {
        let b = buf("x नमस्ते y 👍🏽 z");
        for query in ["नमस्ते", "👍🏽"] {
            let graphemes = query.graphemes(true).count();
            assert!(graphemes < query.chars().count());
            let mut s = SearchState::new(query.to_string(), true);
            perform_search(&b, &mut s);
            assert_eq!(s.match_lengths, vec![graphemes], "{query}");
        }
    }

    #[test]
    fn literal_replace_keeps_the_text_after_a_cluster_match() {
        for (query, expected) in [("नमस्ते", "x hi y 👍🏽 z"), ("👍🏽", "x नमस्ते y hi z")]
        {
            let mut b = buf("x नमस्ते y 👍🏽 z");
            let mut s = SearchState::new_with_replace(query.to_string(), "hi".to_string(), true);
            perform_search(&b, &mut s);
            replace_all_matches(&mut b, &s).unwrap();
            assert_eq!(b.line_cow(0).unwrap(), expected);

            let mut b = buf("x नमस्ते y 👍🏽 z");
            let r = replace_at_position(&mut b, &s.matches[0], s.match_len_at(0), "hi").unwrap();
            assert_eq!(b.line_cow(0).unwrap(), expected);
            assert_eq!(r.new_cursor.column, s.matches[0].column + 2);
        }
    }
}
