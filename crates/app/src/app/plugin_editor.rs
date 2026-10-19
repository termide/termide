//! What plugins see of and do to the editors of their project: the active
//! file, a buffer's text, edits, opening a file; and the events of the
//! current project's editors — focus, changes, closing.
//!
//! A plugin reaches only the editors of its own project. An edit goes to an
//! open buffer and nowhere else — termide writes no file for a plugin, which
//! can write files itself — and it is one undo step, so the user sees it and
//! can take it back. Changes are told once the typing pauses, and only to
//! plugins that asked, so a busy editor does not flood them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_plugins::protocol::{self, events};
use termide_plugins::rpc::{RpcError, INVALID_PARAMS};
use termide_plugins::PluginKey;

use super::App;
use crate::PanelExt;

/// How long a buffer stays unchanged before its change is told.
const CHANGE_QUIET: Duration = Duration::from_millis(300);
/// The largest buffer `editor/read` hands over, in bytes.
const MAX_READ: usize = 16 * 1024 * 1024;
/// The most edits one `editor/edit` applies.
const MAX_EDITS: usize = 10_000;

/// What the plugins were last told of the current project's editors.
#[derive(Debug, Default)]
pub(crate) struct EditorWatch {
    /// The active editor's file.
    focused: Option<PathBuf>,
    /// Each open file: the version last seen, when it was first seen, and
    /// the version last told.
    versions: BTreeMap<PathBuf, (u64, Instant, u64)>,
    /// The project the above describes; a switch starts afresh.
    project: Option<PathBuf>,
}

/// A position from a request: `line`/`col`, LSP-counted.
fn position(value: &Value, line: &str, col: &str) -> Option<lsp_types::Position> {
    let get = |field: &str| value[field].as_u64().and_then(|n| u32::try_from(n).ok());
    Some(lsp_types::Position::new(get(line)?, get(col).unwrap_or(0)))
}

/// The `edits` of `editor/edit` as text edits.
fn text_edits(value: &Value) -> Result<Vec<lsp_types::TextEdit>, String> {
    let edits = value.as_array().ok_or("`edits` must be a list")?;
    if edits.len() > MAX_EDITS {
        return Err(format!("at most {MAX_EDITS} edits at once"));
    }
    edits
        .iter()
        .map(|edit| {
            let start = position(edit, "line", "col").ok_or("an edit has no `line`")?;
            let end = position(edit, "end_line", "end_col").unwrap_or(start);
            let text = edit["text"].as_str().ok_or("an edit has no `text`")?;
            Ok(lsp_types::TextEdit::new(
                lsp_types::Range::new(start, end),
                text.to_string(),
            ))
        })
        .collect()
}

fn path_param(params: &Value) -> Result<PathBuf, RpcError> {
    let path = PathBuf::from(
        params["path"]
            .as_str()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`path` must be a string"))?,
    );
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(RpcError::new(INVALID_PARAMS, "`path` must be absolute"))
    }
}

fn not_open(path: &Path) -> RpcError {
    RpcError::new(
        INVALID_PARAMS,
        format!("{} is not open in the plugin's project", path.display()),
    )
}

impl App {
    /// `editor/*` requests; `None` for any other method.
    pub(super) fn serve_editor_request(
        &mut self,
        key: &PluginKey,
        method: &str,
        params: &Value,
    ) -> Option<Result<Value, RpcError>> {
        Some(match method {
            protocol::EDITOR_ACTIVE => Ok(self.active_editor(key)),
            protocol::EDITOR_READ => self.read_editor(key, params),
            protocol::EDITOR_EDIT => self.edit_editor(key, params),
            protocol::EDITOR_OPEN => self.open_for_plugin(key, params),
            _ => return None,
        })
    }

    /// The active editor of the plugin's project, if it is current and one
    /// is focused.
    fn active_editor(&self, key: &PluginKey) -> Value {
        if key.project != self.open_projects.current() {
            return Value::Null;
        }
        let Some(editor) = self
            .layout_manager
            .active_panel()
            .and_then(|panel| panel.as_editor())
        else {
            return Value::Null;
        };
        let Some(path) = editor.file_path() else {
            return Value::Null;
        };
        let (line, col) = editor.lsp_cursor();
        let selection = editor.lsp_selection().map(
            |((sl, sc), (el, ec))| json!({ "line": sl, "col": sc, "end_line": el, "end_col": ec }),
        );
        json!({
            "path": path,
            "line": line,
            "col": col,
            "selection": selection,
            "modified": editor.buffer().is_modified(),
            "version": editor.edit_version(),
        })
    }

    fn read_editor(&mut self, key: &PluginKey, params: &Value) -> Result<Value, RpcError> {
        let path = path_param(params)?;
        let editor = self
            .project_editor_mut(&key.project, &path)
            .ok_or_else(|| not_open(&path))?;
        let text = editor.content_string();
        if text.len() > MAX_READ {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "the buffer is too large to hand over",
            ));
        }
        Ok(json!({
            "text": text,
            "version": editor.edit_version(),
            "modified": editor.buffer().is_modified(),
        }))
    }

    fn edit_editor(&mut self, key: &PluginKey, params: &Value) -> Result<Value, RpcError> {
        let path = path_param(params)?;
        let edits = text_edits(&params["edits"]).map_err(|e| RpcError::new(INVALID_PARAMS, e))?;
        let expected = params["version"].as_u64();
        let editor = self
            .project_editor_mut(&key.project, &path)
            .ok_or_else(|| not_open(&path))?;
        if expected.is_some_and(|version| version != editor.edit_version()) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "stale: the buffer changed since that version",
            ));
        }
        if !editor.apply_plugin_edits(&edits) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "the editor only views its file",
            ));
        }
        let version = editor.edit_version();
        // The language server must see what the buffer holds now.
        if let Some(lsp_manager) = self.state.lsp_manager.as_ref() {
            if let Some(editor) = self
                .layout_manager
                .iter_all_panels_mut()
                .filter_map(|panel| panel.as_editor_mut())
                .find(|editor| editor.file_path() == Some(path.as_path()))
            {
                editor.notify_lsp_change(lsp_manager);
            }
        }
        self.state.needs_redraw = true;
        Ok(json!({ "version": version }))
    }

    /// Open `path` at `line`/`col` (from 0) in the plugin's project, which
    /// must be the current one.
    fn open_for_plugin(&mut self, key: &PluginKey, params: &Value) -> Result<Value, RpcError> {
        if key.project != self.open_projects.current() {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "the plugin's project is in the background",
            ));
        }
        let path = path_param(params)?;
        let line = params["line"].as_u64().unwrap_or(0) as usize;
        let column = params["col"].as_u64().unwrap_or(0) as usize;
        // As a go-to-definition opens a file: 1-based there.
        self.process_single_event(termide_core::PanelEvent::OpenFileAt {
            path,
            line: line + 1,
            column: column + 1,
        })
        .map_err(|e| RpcError::new(INVALID_PARAMS, e.to_string()))?;
        Ok(Value::Null)
    }

    /// Per tick: tell the current project's plugins which file is focused,
    /// which changed once the typing paused, and which closed — what any of
    /// them asked for.
    pub(super) fn watch_editors_for_plugins(&mut self) {
        let project = self.open_projects.current().to_path_buf();
        let host = &self.plugins.host;
        let wants_focus = host.wants(&project, events::EDITOR_FOCUSED);
        let wants_change = host.wants(&project, events::EDITOR_CHANGED);
        let wants_close = host.wants(&project, events::FILE_CLOSED);
        if !(wants_focus || wants_change || wants_close) {
            return;
        }
        if self.plugins.editors.project.as_ref() != Some(&project) {
            self.plugins.editors = EditorWatch {
                project: Some(project.clone()),
                ..Default::default()
            };
        }

        let focused = self
            .layout_manager
            .active_panel()
            .and_then(|panel| panel.as_editor())
            .and_then(|editor| editor.file_path().map(Path::to_path_buf));
        let open: Vec<(PathBuf, u64)> = self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut())
            .filter_map(|editor| Some((editor.file_path()?.to_path_buf(), editor.edit_version())))
            .collect();

        let mut tell: Vec<(&str, Value)> = Vec::new();
        let watch = &mut self.plugins.editors;
        if focused != watch.focused {
            watch.focused.clone_from(&focused);
            if let Some(path) = &focused {
                tell.push((events::EDITOR_FOCUSED, json!({ "path": path })));
            }
        }
        let now = Instant::now();
        let open_paths: BTreeSet<&PathBuf> = open.iter().map(|(path, _)| path).collect();
        let closed: Vec<PathBuf> = watch
            .versions
            .keys()
            .filter(|path| !open_paths.contains(path))
            .cloned()
            .collect();
        for path in closed {
            watch.versions.remove(&path);
            tell.push((events::FILE_CLOSED, json!({ "path": path })));
        }
        for (path, version) in open {
            let entry = watch
                .versions
                .entry(path.clone())
                .or_insert((version, now, version));
            if entry.0 != version {
                *entry = (version, now, entry.2);
            } else if entry.2 != version && now.duration_since(entry.1) >= CHANGE_QUIET {
                entry.2 = version;
                tell.push((
                    events::EDITOR_CHANGED,
                    json!({ "path": path, "version": version }),
                ));
            }
        }
        for (method, params) in tell {
            self.plugins.host.emit(&project, method, &params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_read_as_lsp_text_edits() {
        let edits = text_edits(&json!([
            { "line": 2, "col": 4, "end_line": 2, "end_col": 9, "text": "x" },
            { "line": 0, "text": "// top\n" },
        ]))
        .unwrap();
        assert_eq!(edits[0].range.start, lsp_types::Position::new(2, 4));
        assert_eq!(edits[0].range.end, lsp_types::Position::new(2, 9));
        assert_eq!(edits[1].range.start, edits[1].range.end, "an insertion");
        assert!(text_edits(&json!([{ "line": 1 }])).is_err());
        assert!(text_edits(&json!("x")).is_err());
    }
}
