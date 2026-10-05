//! `edit`: search/replace with a unique anchor.
//!
//! Matching runs in three passes of decreasing strictness: exact text, then
//! lines compared without trailing whitespace, then lines compared without
//! any surrounding whitespace (the replacement is re-indented to the file).
//! Each pass only runs when the previous one found nothing, so an exact
//! anchor always behaves exactly. Line endings and a UTF-8 BOM are preserved.

use std::ops::Range;

use serde_json::{json, Value};
use similar::{DiffOp, TextDiff};
use termide_agent_core::ToolText;
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate};

use crate::args::{optional_bool, required_str, resolve_path};

/// Diffs longer than this stay in `details` only.
const INLINE_DIFF_LIMIT: usize = 4 * 1024;

pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        &ToolText::seed("edit").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path, absolute or relative to the working directory" },
                "old_string": { "type": "string", "description": "Text to find; must be unique in the file unless replace_all is set" },
                "new_string": { "type": "string", "description": "Replacement text" },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)" }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("edit").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("edit").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        match edit(call, ctx) {
            Ok((text, details)) => ToolResultMessage::text(call, text).with_details(details),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

fn edit(call: &ToolCall, ctx: &ToolContext) -> Result<(String, Value), String> {
    let raw_path = required_str(call, "path")?;
    let path = resolve_path(ctx, raw_path)?;
    let old = required_str(call, "old_string")?;
    let new = required_str(call, "new_string")?;
    let replace_all = optional_bool(call, "replace_all")?;

    if old.is_empty() {
        return Err("`old_string` is empty; use `write` to create or replace a file".into());
    }
    if old == new {
        return Err("`old_string` and `new_string` are identical".into());
    }

    let original = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "cannot read {}: {error}. `edit` needs an existing text file; use `write` for new files",
            path.display()
        )
    })?;

    let (bom, body) = match original.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", original.as_str()),
    };
    let crlf = body.contains("\r\n");
    let haystack = normalize(body, crlf);
    let needle = normalize(old, crlf);
    let replacement = normalize(new, crlf);

    let matches = find_matches(&haystack, &needle);
    if matches.is_empty() {
        return Err(format!(
            "`old_string` was not found in {}. Read the file again and copy the text exactly.",
            path.display()
        ));
    }
    if matches.len() > 1 && !replace_all {
        return Err(format!(
            "`old_string` matches {} places in {}. Add surrounding lines to make it unique, or set replace_all.",
            matches.len(),
            path.display()
        ));
    }

    let mut updated = haystack.clone();
    for found in matches.iter().rev() {
        let text = found.replacement(&haystack, &needle, &replacement);
        updated.replace_range(found.range.clone(), &text);
    }

    let diff = TextDiff::from_lines(haystack.as_str(), updated.as_str());
    let first_changed_line = diff
        .ops()
        .iter()
        .find(|op| !matches!(op, DiffOp::Equal { .. }))
        .map(|op| op.old_range().start + 1);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{raw_path}"), &format!("b/{raw_path}"))
        .to_string();

    let mut output = String::from(bom);
    output.push_str(&if crlf {
        updated.replace('\n', "\r\n")
    } else {
        updated
    });
    std::fs::write(&path, output)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;

    let strategy = matches[0].strategy.label();
    let mut text = format!(
        "Edited {} ({} replacement{}{}).",
        path.display(),
        matches.len(),
        if matches.len() == 1 { "" } else { "s" },
        if strategy.is_empty() {
            String::new()
        } else {
            format!(", matched {strategy}")
        }
    );
    if unified.len() <= INLINE_DIFF_LIMIT {
        text.push_str("\n\n");
        text.push_str(&unified);
    } else {
        text.push_str(" The diff is too large to show inline.");
    }

    Ok((
        text,
        json!({
            "path": path,
            "replacements": matches.len(),
            "strategy": strategy,
            "first_changed_line": first_changed_line,
            "diff": unified
        }),
    ))
}

fn normalize(text: &str, crlf: bool) -> String {
    if crlf || text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Exact,
    TrailingWhitespace,
    Indentation,
}

impl Strategy {
    fn label(self) -> &'static str {
        match self {
            Self::Exact => "",
            Self::TrailingWhitespace => "ignoring trailing whitespace",
            Self::Indentation => "ignoring indentation",
        }
    }
}

#[derive(Debug)]
struct Found {
    range: Range<usize>,
    strategy: Strategy,
}

impl Found {
    fn replacement(&self, haystack: &str, needle: &str, replacement: &str) -> String {
        match self.strategy {
            Strategy::Exact => replacement.to_string(),
            Strategy::TrailingWhitespace | Strategy::Indentation => {
                // Line-based passes drop a trailing newline from the needle,
                // so the replacement must drop it as well.
                let replacement = if needle.ends_with('\n') {
                    replacement.strip_suffix('\n').unwrap_or(replacement)
                } else {
                    replacement
                };
                if self.strategy == Strategy::TrailingWhitespace {
                    return replacement.to_string();
                }
                let matched = &haystack[self.range.clone()];
                let file_indent = leading_whitespace(matched);
                let needle_indent = leading_whitespace(needle);
                reindent(replacement, needle_indent, file_indent)
            }
        }
    }
}

fn leading_whitespace(text: &str) -> &str {
    let end = text
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(text.len());
    &text[..end]
}

/// Swap `from` for `to` at the start of every non-blank line of `text`.
fn reindent(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.trim().is_empty() {
            out.push_str(line);
        } else if let Some(rest) = line.strip_prefix(from) {
            out.push_str(to);
            out.push_str(rest);
        } else {
            out.push_str(to);
            out.push_str(line.trim_start_matches([' ', '\t']));
        }
    }
    out
}

fn find_matches(haystack: &str, needle: &str) -> Vec<Found> {
    let exact = find_exact(haystack, needle);
    if !exact.is_empty() {
        return exact;
    }
    let lenient = find_by_lines(haystack, needle, Strategy::TrailingWhitespace);
    if !lenient.is_empty() {
        return lenient;
    }
    find_by_lines(haystack, needle, Strategy::Indentation)
}

fn find_exact(haystack: &str, needle: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(index) = haystack[from..].find(needle) {
        let start = from + index;
        found.push(Found {
            range: start..start + needle.len(),
            strategy: Strategy::Exact,
        });
        from = start + needle.len();
    }
    found
}

fn find_by_lines(haystack: &str, needle: &str, strategy: Strategy) -> Vec<Found> {
    let compare = |a: &str, b: &str| match strategy {
        Strategy::Exact => a == b,
        Strategy::TrailingWhitespace => a.trim_end() == b.trim_end(),
        Strategy::Indentation => a.trim() == b.trim(),
    };
    let needle = needle.strip_suffix('\n').unwrap_or(needle);
    let needle_lines: Vec<&str> = needle.split('\n').collect();
    if needle_lines.iter().all(|line| line.trim().is_empty()) {
        return Vec::new();
    }

    let mut starts = Vec::new();
    let mut lines = Vec::new();
    let mut position = 0;
    for line in haystack.split('\n') {
        starts.push(position);
        lines.push(line);
        position += line.len() + 1;
    }

    let mut found = Vec::new();
    let window = needle_lines.len();
    let mut index = 0;
    while index + window <= lines.len() {
        let hit = (0..window).all(|offset| compare(lines[index + offset], needle_lines[offset]));
        if hit {
            let last = index + window - 1;
            found.push(Found {
                range: starts[index]..starts[last] + lines[last].len(),
                strategy,
            });
            index += window;
        } else {
            index += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &std::path::Path, args: Value) -> ToolResultMessage {
        let call = ToolCall {
            id: "e".into(),
            name: "edit".into(),
            arguments: args,
            extra_content: None,
        };
        let ctx = ToolContext::new(dir.to_path_buf());
        EditTool.execute(&call, &ctx, &mut |_| {}, &CancelToken::new())
    }

    fn file(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("f.rs");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn exact_unique_match_is_replaced_and_diffed() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "fn a() {}\nfn b() {}\n");
        let result = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "fn b() {}", "new_string": "fn c() {}" }),
        );
        assert!(!result.is_error, "{}", result.plain_text());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "fn a() {}\nfn c() {}\n"
        );
        let text = result.plain_text();
        assert!(text.starts_with("Edited"));
        assert!(text.contains("-fn b() {}\n+fn c() {}"));
        let details = result.details.unwrap();
        assert_eq!(details["first_changed_line"], 2);
        assert_eq!(details["strategy"], "");
    }

    #[test]
    fn ambiguous_anchor_is_refused_unless_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "x = 1\ny = 1\n");
        let refused = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "= 1", "new_string": "= 2" }),
        );
        assert!(refused.is_error);
        assert!(refused.plain_text().contains("matches 2 places"));

        let all = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "= 1", "new_string": "= 2", "replace_all": true }),
        );
        assert!(!all.is_error);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "x = 2\ny = 2\n");
        assert_eq!(all.details.unwrap()["replacements"], 2);
    }

    #[test]
    fn missing_anchor_and_degenerate_arguments_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "hello\n");
        let missing = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "bye", "new_string": "x" }),
        );
        assert!(missing.plain_text().contains("was not found"));
        let same = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "hello", "new_string": "hello" }),
        );
        assert!(same.plain_text().contains("identical"));
        let empty = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "", "new_string": "x" }),
        );
        assert!(empty.plain_text().contains("`write`"));
        let absent = run(
            dir.path(),
            json!({ "path": "missing.rs", "old_string": "a", "new_string": "b" }),
        );
        assert!(absent.plain_text().contains("cannot read"));
    }

    #[test]
    fn trailing_whitespace_differences_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "let a = 1;   \nlet b = 2;\n");
        let result = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "let a = 1;\nlet b = 2;\n", "new_string": "let a = 10;\nlet b = 2;\n" }),
        );
        assert!(!result.is_error, "{}", result.plain_text());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "let a = 10;\nlet b = 2;\n"
        );
        assert_eq!(
            result.details.unwrap()["strategy"],
            "ignoring trailing whitespace"
        );
    }

    #[test]
    fn indentation_differences_are_tolerated_and_reindented() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(
            dir.path(),
            "fn main() {\n        if x {\n            go();\n        }\n}\n",
        );
        let result = run(
            dir.path(),
            json!({
                "path": "f.rs",
                "old_string": "if x {\n    go();\n}",
                "new_string": "if x {\n    go();\n    stop();\n}"
            }),
        );
        assert!(!result.is_error, "{}", result.plain_text());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "fn main() {\n        if x {\n            go();\n            stop();\n        }\n}\n"
        );
        assert_eq!(result.details.unwrap()["strategy"], "ignoring indentation");
    }

    #[test]
    fn crlf_and_bom_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "\u{feff}one\r\ntwo\r\nthree\r\n");
        let result = run(
            dir.path(),
            json!({ "path": "f.rs", "old_string": "two\n", "new_string": "2\n" }),
        );
        assert!(!result.is_error, "{}", result.plain_text());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "\u{feff}one\r\n2\r\nthree\r\n"
        );
    }
}
