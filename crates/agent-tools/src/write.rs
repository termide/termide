//! `write`: create or overwrite a whole file.

use serde_json::{json, Value};
use termide_agent_core::ToolText;
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate};

use crate::args::{required_str, resolve_path};

pub struct WriteTool;

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        &ToolText::seed("write").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path, absolute or relative to the working directory" },
                "content": { "type": "string", "description": "Complete new content of the file" }
            },
            "required": ["path", "content"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("write").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("write").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        match write(call, ctx) {
            Ok((text, details)) => ToolResultMessage::text(call, text).with_details(details),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

fn write(call: &ToolCall, ctx: &ToolContext) -> Result<(String, Value), String> {
    let path = resolve_path(ctx, required_str(call, "path")?)?;
    let content = required_str(call, "content")?;
    if path.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    let existed = path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    let verb = if existed { "Overwrote" } else { "Created" };
    Ok((
        format!("{verb} {} ({} bytes)", path.display(), content.len()),
        json!({ "path": path, "created": !existed, "bytes": content.len() }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &std::path::Path, args: Value) -> ToolResultMessage {
        let call = ToolCall {
            id: "w".into(),
            name: "write".into(),
            arguments: args,
            extra_content: None,
        };
        let ctx = ToolContext::new(dir.to_path_buf());
        WriteTool.execute(&call, &ctx, &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn creates_parents_and_reports_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let first = run(
            dir.path(),
            json!({ "path": "nested/dir/f.txt", "content": "one" }),
        );
        assert!(!first.is_error);
        assert!(first.plain_text().starts_with("Created"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested/dir/f.txt")).unwrap(),
            "one"
        );

        let second = run(
            dir.path(),
            json!({ "path": "nested/dir/f.txt", "content": "two" }),
        );
        assert!(second.plain_text().starts_with("Overwrote"));
        assert_eq!(second.details.unwrap()["created"], false);
    }

    #[test]
    fn refuses_directories_and_missing_arguments() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), json!({ "path": ".", "content": "" })).is_error);
        let missing = run(dir.path(), json!({ "path": "x" }));
        assert!(missing.plain_text().contains("`content`"));
    }
}
