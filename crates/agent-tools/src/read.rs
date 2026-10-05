//! `read`: a file as numbered lines, paged with `offset` and `limit`.

use serde_json::{json, Value};
use termide_agent_core::ToolText;
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate};

use crate::args::{optional_u64, required_str, resolve_path};
use crate::truncate::{READ_MAX_BYTES, READ_MAX_LINES};

pub struct ReadTool;

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        &ToolText::seed("read").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path, absolute or relative to the working directory" },
                "offset": { "type": "integer", "minimum": 1, "description": "1-based line number to start from (default 1)" },
                "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines to return (default 2000)" }
            },
            "required": ["path"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("read").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("read").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        match read(call, ctx) {
            Ok((text, details)) => ToolResultMessage::text(call, text).with_details(details),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

fn read(call: &ToolCall, ctx: &ToolContext) -> Result<(String, Value), String> {
    let raw_path = required_str(call, "path")?;
    let path = resolve_path(ctx, raw_path)?;
    let offset = optional_u64(call, "offset")?.unwrap_or(1).max(1) as usize;
    let limit = optional_u64(call, "limit")?
        .map(|value| value.max(1) as usize)
        .unwrap_or(READ_MAX_LINES)
        .min(READ_MAX_LINES);

    let bytes = std::fs::read(&path).map_err(|error| {
        if path.is_dir() {
            format!(
                "{} is a directory; use `bash` with `ls` to list it",
                path.display()
            )
        } else {
            format!("cannot read {}: {error}", path.display())
        }
    })?;
    if bytes.iter().take(8192).any(|&byte| byte == 0) {
        return Err(format!(
            "{} looks like a binary file; `read` only handles text",
            path.display()
        ));
    }
    let content = String::from_utf8_lossy(&bytes);
    if content.is_empty() {
        return Ok((
            "[empty file]".to_string(),
            json!({ "path": path, "total_lines": 0 }),
        ));
    }

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if offset > total {
        return Err(format!(
            "offset {offset} is past the end of {} ({total} lines)",
            path.display()
        ));
    }

    let mut out = String::new();
    let mut shown_end = offset - 1;
    for (index, line) in lines.iter().enumerate().skip(offset - 1).take(limit) {
        let number = index + 1;
        let mut rendered = format!("{number:>6}\t{line}\n");
        if rendered.len() > READ_MAX_BYTES {
            let cut = line
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| i > READ_MAX_BYTES / 2)
                .unwrap_or(line.len());
            rendered = format!("{number:>6}\t{}… [line truncated]\n", &line[..cut]);
        }
        if !out.is_empty() && out.len() + rendered.len() > READ_MAX_BYTES {
            break;
        }
        out.push_str(&rendered);
        shown_end = number;
    }

    let truncated = shown_end < total;
    if truncated {
        out.push_str(&format!(
            "\n[Showing lines {offset}-{shown_end} of {total}. Use offset={} to continue.]",
            shown_end + 1
        ));
    }
    Ok((
        out,
        json!({
            "path": path,
            "total_lines": total,
            "shown": [offset, shown_end],
            "truncated": truncated
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn call(args: Value) -> ToolCall {
        ToolCall {
            id: "r".into(),
            name: "read".into(),
            arguments: args,
            extra_content: None,
        }
    }

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir.to_path_buf())
    }

    fn run(dir: &std::path::Path, args: Value) -> ToolResultMessage {
        ReadTool.execute(&call(args), &ctx(dir), &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn numbers_lines_and_resolves_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let result = run(dir.path(), json!({ "path": "a.txt" }));
        assert!(!result.is_error);
        assert_eq!(
            result.plain_text(),
            "     1\talpha\n     2\tbeta\n     3\tgamma\n"
        );
        assert_eq!(result.details.unwrap()["total_lines"], 3);
    }

    #[test]
    fn offset_and_limit_page_with_a_continuation_note() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|n| format!("l{n}\n")).collect();
        std::fs::write(dir.path().join("b.txt"), body).unwrap();
        let result = run(
            dir.path(),
            json!({ "path": "b.txt", "offset": 4, "limit": 3 }),
        );
        let text = result.plain_text();
        assert!(text.starts_with("     4\tl4\n     5\tl5\n     6\tl6\n"));
        assert!(text.ends_with("[Showing lines 4-6 of 10. Use offset=7 to continue.]"));
    }

    #[test]
    fn errors_are_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let missing = run(dir.path(), json!({ "path": "nope.txt" }));
        assert!(missing.is_error);
        assert!(missing.plain_text().contains("cannot read"));

        let dir_result = run(dir.path(), json!({ "path": "." }));
        assert!(dir_result.is_error);
        assert!(dir_result.plain_text().contains("is a directory"));

        std::fs::write(dir.path().join("bin"), [0u8, 159, 146, 150]).unwrap();
        let binary = run(dir.path(), json!({ "path": "bin" }));
        assert!(binary.is_error);
        assert!(binary.plain_text().contains("binary"));

        let no_path = run(dir.path(), json!({}));
        assert!(no_path
            .plain_text()
            .contains("missing required argument `path`"));
        let _ = PathBuf::new();
    }

    #[test]
    fn empty_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("e"), "").unwrap();
        assert_eq!(
            run(dir.path(), json!({ "path": "e" })).plain_text(),
            "[empty file]"
        );
    }
}
