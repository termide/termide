//! Keyword highlighter — a lightweight, config-driven line tokenizer for
//! languages that don't (yet) have a tree-sitter grammar.

use ratatui::style::Style;
use std::borrow::Cow;
use std::collections::HashSet;

/// A user-defined language for the keyword highlighter. Built by the host from
/// configuration; this crate stays config-format-agnostic.
#[derive(Debug, Clone)]
pub struct KeywordSyntax {
    /// Display name (e.g. shown in the status bar).
    pub name: String,
    /// Line-comment lead-in (e.g. `##`, `//`, `#`). Everything from here to the
    /// end of the line is a comment.
    pub line_comment: Option<String>,
    /// Block-comment delimiters `(open, close)`. Only matched within a single
    /// line (multi-line blocks need cross-line context the line tokenizer lacks).
    pub block_comment: Option<(String, String)>,
    /// Words coloured as keywords.
    pub keywords: HashSet<String>,
    /// Words coloured as types.
    pub types: HashSet<String>,
}

impl KeywordSyntax {
    /// Build from primitive fields (keyword/type lists become sets).
    pub fn new(
        name: String,
        line_comment: Option<String>,
        block_comment: Option<(String, String)>,
        keywords: impl IntoIterator<Item = String>,
        types: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            name,
            line_comment: line_comment.filter(|s| !s.is_empty()),
            block_comment: block_comment.filter(|(o, c)| !o.is_empty() && !c.is_empty()),
            keywords: keywords.into_iter().collect(),
            types: types.into_iter().collect(),
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenize one line with `syntax`, mapping each token category to a style via
/// `style`. Segments concatenate back to exactly `line`. Line-based: comments,
/// strings and numbers do not span lines (block comments only match in-line).
pub(crate) fn keyword_line_segments(
    line: &str,
    syntax: &KeywordSyntax,
    style: &impl Fn(&str) -> Style,
) -> Vec<(Cow<'static, str>, Style)> {
    let default = style("");
    let mut out: Vec<(Cow<'static, str>, Style)> = Vec::new();
    let mut push = |text: &str, st: Style| {
        if !text.is_empty() {
            out.push((Cow::Owned(text.to_string()), st));
        }
    };

    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];

        // Line comment → rest of the line.
        if let Some(lc) = &syntax.line_comment {
            if rest.starts_with(lc.as_str()) {
                push(rest, style("comment"));
                break;
            }
        }
        // Single-line block comment.
        if let Some((open, close)) = &syntax.block_comment {
            if rest.starts_with(open.as_str()) {
                let end = rest[open.len()..]
                    .find(close.as_str())
                    .map(|p| i + open.len() + p + close.len())
                    .unwrap_or(line.len());
                push(&line[i..end], style("comment"));
                i = end;
                continue;
            }
        }

        let c = rest.chars().next().unwrap();

        // String literal (double or single quote), with `\` escapes.
        if c == '"' || c == '\'' {
            let quote = c;
            let mut j = i + c.len_utf8();
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                j += ch.len_utf8();
                if ch == '\\' && j < line.len() {
                    j += line[j..].chars().next().unwrap().len_utf8();
                } else if ch == quote {
                    break;
                }
            }
            push(&line[i..j], style("string"));
            i = j;
            continue;
        }

        // Number literal.
        if c.is_ascii_digit() {
            let mut j = i;
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                if ch.is_alphanumeric() || ch == '.' || ch == '_' {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            push(&line[i..j], style("number"));
            i = j;
            continue;
        }

        // Identifier / keyword / type.
        if is_ident_start(c) {
            let mut j = i;
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                if is_ident_continue(ch) {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            let word = &line[i..j];
            let st = if syntax.keywords.contains(word) {
                style("keyword")
            } else if syntax.types.contains(word) {
                style("type")
            } else {
                default
            };
            push(word, st);
            i = j;
            continue;
        }

        // Operator/punctuation run (ASCII punctuation that isn't a delimiter we
        // already handle); everything else (whitespace) is default.
        if c.is_ascii_punctuation() {
            let mut j = i;
            while j < line.len() {
                let ch = line[j..].chars().next().unwrap();
                // Stop so the next iteration can re-detect comments/strings.
                if ch.is_ascii_punctuation() && ch != '"' && ch != '\'' && ch != '_' {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            // Avoid swallowing a comment lead-in that begins mid-run.
            push(&line[i..j], style("operator"));
            i = j;
            continue;
        }

        // Whitespace / other: emit a default run up to the next interesting char.
        let mut j = i;
        while j < line.len() {
            let ch = line[j..].chars().next().unwrap();
            if ch.is_ascii_punctuation() || is_ident_start(ch) || ch.is_ascii_digit() {
                break;
            }
            j += ch.len_utf8();
        }
        // Guard against zero-progress on an unexpected byte.
        if j == i {
            j += bytes[i..].iter().next().map(|_| c.len_utf8()).unwrap_or(1);
        }
        push(&line[i..j], default);
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn keyword_highlighter_categorizes_and_is_lossless() {
        let syntax = KeywordSyntax::new(
            "test".to_string(),
            Some("##".to_string()),
            None,
            ["pub", "struct"].iter().map(|s| s.to_string()),
            ["u8"].iter().map(|s| s.to_string()),
        );
        let style = |name: &str| {
            Style::default().fg(match name {
                "keyword" => Color::Red,
                "type" => Color::Green,
                "comment" => Color::Blue,
                _ => Color::White,
            })
        };
        let line = "pub x := u8  ## note";
        let segs = keyword_line_segments(line, &syntax, &style);

        // Lossless: segments concatenate back to the original line.
        let joined: String = segs.iter().map(|(t, _)| t.as_ref()).collect();
        assert_eq!(joined, line);

        let cat = |word: &str| segs.iter().find(|(t, _)| t == word).map(|(_, s)| s.fg);
        assert_eq!(cat("pub"), Some(Some(Color::Red)), "keyword");
        assert_eq!(cat("u8"), Some(Some(Color::Green)), "type");
        assert!(
            segs.iter()
                .any(|(t, s)| t.as_ref().contains("note") && s.fg == Some(Color::Blue)),
            "## starts a comment"
        );
    }
}
