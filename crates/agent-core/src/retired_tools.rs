//! Rewriting the shell tool's old name, `bash`, out of what termide keeps
//! itself: the global `ai/hooks.toml` and `ai/agents/*/AGENT.md`, and the
//! session logs. A project's own files are the repository's and are left as
//! they are.
//!
//! Transitional: it ships for one release after the rename, then goes
//! together with the old name's other readers ([`crate::canonical_tool_name`]
//! and the rest).

use std::path::Path;

use serde_json::Value;

use crate::atomic_write::write_atomic;
use crate::hooks::HOOKS_FILE;
use crate::layers::AGENT_FILE;
use crate::tool::{LEGACY_SHELL_TOOL, SHELL_TOOL};

/// Rename the tool in the global hooks and agent profiles under `global`.
pub(crate) fn rewrite_global(global: &Path) {
    rewrite_file(&global.join(HOOKS_FILE), rename_in_hooks);
    let Ok(agents) = std::fs::read_dir(global.join("agents")) else {
        return;
    };
    for agent in agents.flatten() {
        rewrite_file(&agent.path().join(AGENT_FILE), rename_in_agent);
    }
}

/// Replace the file at `path` with what `rename` makes of it, when it makes
/// anything; a missing or unreadable file is left alone.
fn rewrite_file(path: &Path, rename: fn(&str) -> Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Some(renamed) = rename(&text) else {
        return;
    };
    match write_atomic(path, renamed.as_bytes()) {
        Ok(()) => log::info!(
            "Renamed the {LEGACY_SHELL_TOOL} tool to {SHELL_TOOL} in {}",
            path.display()
        ),
        Err(error) => log::warn!("Could not rewrite {}: {error}", path.display()),
    }
}

/// `hooks.toml` with the old name renamed in every hook's `tools`.
fn rename_in_hooks(text: &str) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().ok()?;
    let mut changed = false;
    for (_, hook) in doc.iter_mut() {
        let Some(tools) = hook
            .as_table_like_mut()
            .and_then(|hook| hook.get_mut("tools"))
            .and_then(|tools| tools.as_array_mut())
        else {
            continue;
        };
        for tool in tools.iter_mut() {
            if tool.as_str() == Some(LEGACY_SHELL_TOOL) {
                let decor = tool.decor().clone();
                *tool = SHELL_TOOL.into();
                *tool.decor_mut() = decor;
                changed = true;
            }
        }
    }
    changed.then(|| doc.to_string())
}

/// An `AGENT.md` with the old name renamed in its front matter's `tools`, in
/// any of the shapes it is read in: `tools: read, bash`, a bracketed list, or
/// a block list of `- bash` lines below the key.
fn rename_in_agent(text: &str) -> Option<String> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_start_matches('\u{feff}').trim_end() != "---" {
        return None;
    }
    let mut out = String::from(first);
    let mut changed = false;
    let mut in_tools = false;
    let mut in_front = true;
    for line in lines {
        if !in_front {
            out.push_str(line);
            continue;
        }
        let trimmed = line.trim_end();
        let stripped = trimmed.trim_start();
        if trimmed == "---" {
            in_front = false;
            out.push_str(line);
            continue;
        }
        if stripped.starts_with("tools:") {
            in_tools = true;
            let (key, rest) = line.split_at(line.len() - stripped.len() + "tools:".len());
            let renamed = rename_words(rest);
            changed |= renamed != rest;
            out.push_str(key);
            out.push_str(&renamed);
            continue;
        }
        if in_tools && stripped.starts_with('-') {
            let renamed = rename_words(line);
            changed |= renamed != line;
            out.push_str(&renamed);
            continue;
        }
        if !stripped.is_empty() && !stripped.starts_with('#') {
            in_tools = false;
        }
        out.push_str(line);
    }
    changed.then_some(out)
}

/// `text` with every word that is the old name, alone between quotes,
/// commas, brackets, dashes of a list or spaces, renamed.
fn rename_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(if word == LEGACY_SHELL_TOOL {
            SHELL_TOOL
        } else {
            word
        });
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric()
            || matches!(c, '_' | '*' | ':' | '.')
            || (c == '-' && !word.is_empty())
        {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The marker left in the sessions directory once every log in it has been
/// rewritten, so the pass runs no more.
const LOGS_DONE: &str = ".shell-tool-renamed";

/// Rewrite the old name out of every session log under `sessions`, once. A
/// log another termide holds open is skipped and tried again the next time;
/// the pass is marked done only when none was skipped. Meant to run on a
/// thread of its own: it reads every log.
pub fn rewrite_session_logs(sessions: &Path) {
    let marker = sessions.join(LOGS_DONE);
    if marker.exists() || !sessions.is_dir() {
        return;
    }
    let mut skipped = 0usize;
    let mut rewritten = 0usize;
    let mut stack = vec![sessions.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            skipped += 1;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                match rewrite_log(&path) {
                    Ok(true) => rewritten += 1,
                    Ok(false) => {}
                    Err(error) => {
                        log::warn!("Could not rewrite {}: {error}", path.display());
                        skipped += 1;
                    }
                }
            }
        }
    }
    log::info!(
        "Renamed the {LEGACY_SHELL_TOOL} tool to {SHELL_TOOL} in {rewritten} session logs, {skipped} left for later"
    );
    if skipped == 0 {
        let _ = std::fs::write(&marker, "");
    }
}

/// Rewrite one log under its session's lock; whether it changed. A log held
/// by another process is an error, to be tried again.
fn rewrite_log(path: &Path) -> std::io::Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let needle = format!("\"{LEGACY_SHELL_TOOL}\"");
    if !text.contains(&needle) {
        return Ok(false);
    }
    let _claim = crate::session::claim_log(path)?;
    // Read again under the claim: the holder may have appended since.
    let text = std::fs::read_to_string(path)?;
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if !body.contains(&needle) {
            out.push_str(line);
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(body) else {
            out.push_str(line);
            continue;
        };
        if rename_in_entry(&mut value) {
            changed = true;
            out.push_str(&value.to_string());
            out.push_str(&line[body.len()..]);
        } else {
            out.push_str(line);
        }
    }
    if changed {
        write_atomic(path, out.as_bytes())?;
    }
    Ok(changed)
}

/// Rename the tool wherever a log entry names it: a call's `name`, a
/// result's `tool_name`, a toolset's `disabled`. Whether anything changed.
fn rename_in_entry(value: &mut Value) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            let is_call = map.contains_key("arguments") && map.contains_key("id");
            for (key, field) in map.iter_mut() {
                let names_tool = match key.as_str() {
                    "name" => is_call,
                    "tool_name" => true,
                    _ => false,
                };
                if names_tool && field.as_str() == Some(LEGACY_SHELL_TOOL) {
                    *field = Value::from(SHELL_TOOL);
                    changed = true;
                } else if key == "disabled" {
                    if let Value::Array(items) = field {
                        for item in items {
                            if item.as_str() == Some(LEGACY_SHELL_TOOL) {
                                *item = Value::from(SHELL_TOOL);
                                changed = true;
                            }
                        }
                    }
                } else {
                    changed |= rename_in_entry(field);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                changed |= rename_in_entry(item);
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hooks_name_the_new_tool_and_keep_the_rest() {
        let text = "[guard]\nevent = \"before_tool_call\"\ntools = [\"bash\", \"write\"] # careful\ncommand = \"g.sh\"\n\n[log]\nevent = \"after_tool_call\"\ncommand = \"l.sh\"\n";
        assert_eq!(
            rename_in_hooks(text).unwrap(),
            "[guard]\nevent = \"before_tool_call\"\ntools = [\"shell\", \"write\"] # careful\ncommand = \"g.sh\"\n\n[log]\nevent = \"after_tool_call\"\ncommand = \"l.sh\"\n"
        );
        assert_eq!(rename_in_hooks("[a]\ntools = [\"read\"]\n"), None);
    }

    #[test]
    fn an_agents_tools_take_the_new_name_in_every_shape() {
        assert_eq!(
            rename_in_agent("---\ntools: read, bash\n---\nUse bash freely.\n").unwrap(),
            "---\ntools: read, shell\n---\nUse bash freely.\n"
        );
        assert_eq!(
            rename_in_agent("---\ntools: [\"read\", 'bash']\nmode: plan\n---\n").unwrap(),
            "---\ntools: [\"read\", 'shell']\nmode: plan\n---\n"
        );
        assert_eq!(
            rename_in_agent(
                "---\ntools:\n  - read\n  - \"bash\"\n# note\ndescription: bash helper\n---\n"
            )
            .unwrap(),
            "---\ntools:\n  - read\n  - \"shell\"\n# note\ndescription: bash helper\n---\n"
        );
        // Only the word itself, and only in `tools`.
        assert_eq!(rename_in_agent("---\ntools: bash-ish, rebash\n---\n"), None);
        assert_eq!(rename_in_agent("No front matter: tools: bash\n"), None);
    }

    #[test]
    fn session_logs_are_rewritten_once_skipping_an_open_one() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        let project = sessions.join("p");
        std::fs::create_dir_all(&project).unwrap();
        let call = json!({ "type": "message", "message": { "role": "assistant", "content": [
            { "type": "toolCall", "id": "c", "name": "bash", "arguments": { "command": "echo \"bash\"" } }
        ] } });
        let result = json!({ "type": "message", "message": { "role": "toolResult",
            "tool_call_id": "c", "tool_name": "bash", "content": [] } });
        let toolset = json!({ "type": "toolset", "disabled": ["bash", "mcp:x"] });
        let other = json!({ "type": "message", "message": { "role": "user", "content": "\"bash\" is a shell" } });
        let log = format!("{call}\n{result}\n{toolset}\n{other}\n");
        let old = project.join("a.jsonl");
        std::fs::write(&old, &log).unwrap();
        let held = project.join("b.jsonl");
        std::fs::write(&held, &log).unwrap();

        let claim = crate::session::claim_log(&held).unwrap();
        rewrite_session_logs(&sessions);
        let rewritten = std::fs::read_to_string(&old).unwrap();
        let lines: Vec<Value> = rewritten
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines[0]["message"]["content"][0]["name"], "shell");
        // What the command said stays as it was.
        assert_eq!(
            lines[0]["message"]["content"][0]["arguments"]["command"],
            "echo \"bash\""
        );
        assert_eq!(lines[1]["message"]["tool_name"], "shell");
        assert_eq!(lines[2]["disabled"], json!(["shell", "mcp:x"]));
        assert_eq!(lines[3], other);
        // The open one waits, so the pass is not done.
        assert_eq!(std::fs::read_to_string(&held).unwrap(), log);
        assert!(!sessions.join(LOGS_DONE).exists());

        drop(claim);
        rewrite_session_logs(&sessions);
        assert_eq!(std::fs::read_to_string(&held).unwrap(), rewritten);
        assert!(sessions.join(LOGS_DONE).exists());
    }
}
