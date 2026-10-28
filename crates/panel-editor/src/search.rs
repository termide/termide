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

    // Literal search, case-insensitive by folding both sides (see
    // [`CaseFolded`]); match bytes are then mapped back to the line.
    let query = if search.case_sensitive {
        search.query.clone()
    } else {
        CaseFolded::new(&search.query).text
    };
    for line_idx in 0..buffer.line_count() {
        let Some(line_text) = buffer.line_cow(line_idx) else {
            continue;
        };
        let folded = (!search.case_sensitive).then(|| CaseFolded::new(&line_text));
        let haystack = folded.as_ref().map_or(&*line_text, |f| f.text.as_str());

        let mut byte_col = 0;
        while let Some(byte_pos) = haystack[byte_col..].find(&query) {
            let match_start = byte_col + byte_pos;
            let match_end = match_start + query.len();
            let range = match &folded {
                Some(folded) => folded.original_range(match_start, match_end),
                None => Some((match_start, match_end)),
            };
            if let Some((start, end)) = range {
                // Columns and lengths are in graphemes of the line itself,
                // the unit cursors and deletions use, so `नमस्ते` or `👍🏽`
                // count as the clusters they are.
                search.matches.push(Cursor {
                    line: line_idx,
                    column: line_text[..start].graphemes(true).count(),
                });
                search
                    .match_lengths
                    .push(line_text[start..end].graphemes(true).count());
            }
            match haystack[match_start..].chars().next() {
                Some(first_char) => byte_col = match_start + first_char.len_utf8(),
                None => break,
            }
        }
    }
}

/// A line folded for case-insensitive comparison, char by char, with where
/// each original char's folding starts.
///
/// Folding can change a char's length: `İ` becomes `i̇` (two chars), `ẞ`
/// becomes `ß` (one byte fewer). Positions found in the folded text are
/// therefore mapped back through `starts` rather than used on the line.
/// Folding is per char, unlike `str::to_lowercase`, so a final `ς` and `σ`
/// fold alike wherever they stand.
struct CaseFolded {
    text: String,
    /// `(folded byte, original byte)` at the start of every original char,
    /// and of the end of both texts.
    starts: Vec<(usize, usize)>,
}

impl CaseFolded {
    fn new(original: &str) -> Self {
        let mut text = String::with_capacity(original.len());
        let mut starts = Vec::with_capacity(original.len() + 1);
        for (byte, c) in original.char_indices() {
            starts.push((text.len(), byte));
            for lower in c.to_lowercase() {
                text.push(if lower == 'ς' { 'σ' } else { lower });
            }
        }
        starts.push((text.len(), original.len()));
        Self { text, starts }
    }

    /// The original byte range of the folded range `start..end`, or `None`
    /// when it begins or ends inside the folding of one char, like the `i`
    /// of `İ`'s `i̇`: that is no match for the original text.
    fn original_range(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let original = |folded: usize| {
            self.starts
                .binary_search_by_key(&folded, |&(f, _)| f)
                .ok()
                .map(|i| self.starts[i].1)
        };
        Some((original(start)?, original(end)?))
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

    /// Case-insensitive matches of `query` in `text`: (column, length).
    fn found(text: &str, query: &str) -> Vec<(usize, usize)> {
        let mut s = SearchState::new(query.to_string(), false);
        perform_search(&buf(text), &mut s);
        s.matches
            .iter()
            .map(|m| m.column)
            .zip(s.match_lengths)
            .collect()
    }

    /// `İ` folds to two chars, `i̇`: a plain `i` is not found inside it,
    /// and what follows it is found where it stands.
    #[test]
    fn case_insensitive_search_maps_a_dotted_capital_i_back() {
        assert_eq!(found("İi İ", "i"), [(1, 1)]);
        assert_eq!(found("İi İ", "İ"), [(0, 1), (3, 1)]);
        assert_eq!(found("İi İ", "i\u{307}"), [(0, 1), (3, 1)]);
        assert_eq!(found("İİx İX", "x"), [(2, 1), (5, 1)]);
    }

    /// `ẞ` folds to `ß`, a byte shorter, which must not shift what follows.
    #[test]
    fn case_insensitive_search_maps_a_capital_sharp_s_back() {
        let text = "STRAẞE straße Straße";
        assert_eq!(found(text, "straße"), [(0, 6), (7, 6), (14, 6)]);
        assert_eq!(found(text, "STRAẞE"), [(0, 6), (7, 6), (14, 6)]);
        assert_eq!(found(text, "e"), [(5, 1), (12, 1), (19, 1)]);
    }

    /// Greek, Cyrillic and Latin on one line; a final `ς` and `σ` fold
    /// alike wherever they stand.
    #[test]
    fn case_insensitive_search_across_scripts_and_final_sigma() {
        let text = "ΟΔΟΣ Улица οδος ΣΟΦΙΑ";
        assert_eq!(found(text, "οδοσ"), [(0, 4), (11, 4)]);
        assert_eq!(found(text, "ΟΔΟΣ"), [(0, 4), (11, 4)]);
        assert_eq!(found(text, "σ"), [(3, 1), (14, 1), (16, 1)]);
        assert_eq!(found(text, "УЛИЦА"), [(5, 5)]);
    }

    #[test]
    fn case_insensitive_replace_all_keeps_the_text_around_folded_chars() {
        for (query, expected) in [
            ("İSTANBUL", "X istanbul X ẞ"),
            ("istanbul", "İstanbul X İSTANBUL ẞ"),
            ("ß", "İstanbul istanbul İSTANBUL X"),
            ("i", "İstanbul Xstanbul İSTANBUL ẞ"),
        ] {
            let mut b = buf("İstanbul istanbul İSTANBUL ẞ");
            let mut s = SearchState::new_with_replace(query.to_string(), "X".to_string(), false);
            perform_search(&b, &mut s);
            replace_all_matches(&mut b, &s).unwrap();
            assert_eq!(b.line_cow(0).unwrap(), expected, "{query}");
        }
    }
}
