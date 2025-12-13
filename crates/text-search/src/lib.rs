//! Text search and replace for termide.
//!
//! Provides text search functionality with regex support.

use regex::Regex;

/// Search direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchDirection {
    #[default]
    Forward,
    Backward,
}

/// A match location in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Line number (0-indexed).
    pub line: usize,
    /// Column (character offset, 0-indexed).
    pub col: usize,
    /// Match length in characters.
    pub len: usize,
}

/// Search options.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Case-sensitive search.
    pub case_sensitive: bool,
    /// Use regex pattern.
    pub regex: bool,
    /// Whole word only.
    pub whole_word: bool,
}

/// Search in text and return all matches.
pub fn find_all(text: &str, pattern: &str, options: &SearchOptions) -> Vec<Match> {
    if pattern.is_empty() {
        return vec![];
    }

    let mut matches = Vec::new();

    // Build search pattern
    let search_pattern = if options.regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };

    let search_pattern = if options.whole_word {
        format!(r"\b{}\b", search_pattern)
    } else {
        search_pattern
    };

    let regex = if options.case_sensitive {
        Regex::new(&search_pattern)
    } else {
        Regex::new(&format!("(?i){}", search_pattern))
    };

    let regex = match regex {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    for (line_idx, line) in text.lines().enumerate() {
        for mat in regex.find_iter(line) {
            // Convert byte offset to char offset
            let col = line[..mat.start()].chars().count();
            let len = mat.as_str().chars().count();

            matches.push(Match {
                line: line_idx,
                col,
                len,
            });
        }
    }

    matches
}

/// Find closest match to given position.
pub fn find_closest(
    matches: &[Match],
    line: usize,
    col: usize,
    direction: SearchDirection,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }

    match direction {
        SearchDirection::Forward => {
            // Find first match at or after position
            matches
                .iter()
                .position(|m| m.line > line || (m.line == line && m.col >= col))
                .or(Some(0)) // Wrap to first match
        }
        SearchDirection::Backward => {
            // Find last match at or before position
            matches
                .iter()
                .rposition(|m| m.line < line || (m.line == line && m.col <= col))
                .or(Some(matches.len() - 1)) // Wrap to last match
        }
    }
}

/// Replace text at match position.
pub fn replace_at(text: &mut String, mat: &Match, replacement: &str) {
    let lines: Vec<&str> = text.lines().collect();
    if mat.line >= lines.len() {
        return;
    }

    let line = lines[mat.line];
    let char_indices: Vec<(usize, char)> = line.char_indices().collect();

    if mat.col >= char_indices.len() {
        return;
    }

    let start_byte = char_indices[mat.col].0;
    let end_byte = if mat.col + mat.len < char_indices.len() {
        char_indices[mat.col + mat.len].0
    } else {
        line.len()
    };

    // Calculate absolute byte position
    let mut abs_start = 0;
    for (i, l) in lines.iter().enumerate() {
        if i == mat.line {
            abs_start += start_byte;
            break;
        }
        abs_start += l.len() + 1; // +1 for newline
    }
    let abs_end = abs_start + (end_byte - start_byte);

    text.replace_range(abs_start..abs_end, replacement);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_all_simple() {
        let text = "hello world\nhello there";
        let matches = find_all(text, "hello", &SearchOptions::default());
        assert_eq!(matches.len(), 2);
        assert_eq!(
            matches[0],
            Match {
                line: 0,
                col: 0,
                len: 5
            }
        );
        assert_eq!(
            matches[1],
            Match {
                line: 1,
                col: 0,
                len: 5
            }
        );
    }

    #[test]
    fn test_find_all_case_insensitive() {
        let text = "Hello HELLO hello";
        let matches = find_all(text, "hello", &SearchOptions::default());
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn test_find_all_case_sensitive() {
        let text = "Hello HELLO hello";
        let opts = SearchOptions {
            case_sensitive: true,
            ..Default::default()
        };
        let matches = find_all(text, "hello", &opts);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_find_closest() {
        let matches = vec![
            Match {
                line: 0,
                col: 5,
                len: 3,
            },
            Match {
                line: 2,
                col: 10,
                len: 3,
            },
            Match {
                line: 5,
                col: 0,
                len: 3,
            },
        ];

        assert_eq!(
            find_closest(&matches, 1, 0, SearchDirection::Forward),
            Some(1)
        );
        assert_eq!(
            find_closest(&matches, 3, 0, SearchDirection::Forward),
            Some(2)
        );
        assert_eq!(
            find_closest(&matches, 6, 0, SearchDirection::Forward),
            Some(0)
        );
    }
}
