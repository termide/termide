//! What plugins see of and do to the editors of their project: the active
//! file, a buffer's text, edits, opening a file; and the events of the
//! current project's editors — focus, changes, closing.
//!
//! A plugin reaches only the editors of its own project. An edit goes to an
//! open buffer and nowhere else — termide writes no file for a plugin, which
//! can write files itself — and it is one undo step, so the user sees it and
//! can take it back. Changes are told once the typing pauses, and only to
//! plugins that asked, so a busy editor does not flood them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
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

/// How long each plugin may take over `file/will_save`.
const WILL_SAVE_TIMEOUT: Duration = Duration::from_secs(3);

/// Where a plugin's answer to `file/will_save` arrives.
type Answer = Receiver<Result<Value, String>>;

/// A `Ctrl+S` the plugins see first: the ones still to ask, and the answer
/// waited for.
#[derive(Debug)]
pub(crate) struct SaveJob {
    project: PathBuf,
    path: PathBuf,
    queue: VecDeque<PluginKey>,
    waiting: Option<(PluginKey, u64, Answer)>,
}

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
    /// Saves waiting for the plugins.
    saves: Vec<SaveJob>,
}

impl EditorWatch {
    /// Start afresh on a switch to `project`. The saves under way stay: each
    /// names its own project, and its editor already gave its Ctrl+S over.
    fn follow_project(&mut self, project: &Path) {
        if self.project.as_deref() != Some(project) {
            *self = Self {
                project: Some(project.to_path_buf()),
                saves: std::mem::take(&mut self.saves),
                ..Self::default()
            };
        }
    }
}

/// A position from a request: `line`/`col`, LSP-counted.
fn position(value: &Value, line: &str, col: &str) -> Option<lsp_types::Position> {
    let get = |field: &str| value[field].as_u64().and_then(|n| u32::try_from(n).ok());
    Some(lsp_types::Position::new(get(line)?, get(col).unwrap_or(0)))
}

/// The `edits` of `editor/edit` as text edits.
pub(super) fn text_edits(value: &Value) -> Result<Vec<lsp_types::TextEdit>, String> {
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

    /// Per tick: hold `Ctrl+S` for the plugins that asked to see a file
    /// before it is written, ask them one after another, apply their edits
    /// and write the file. Nothing waits on the UI thread: each plugin is
    /// asked on a thread of its own and has three seconds.
    pub(super) fn serve_plugin_saves(&mut self) {
        let project = self.open_projects.current().to_path_buf();
        let gate = self.plugins.host.wants(&project, events::FILE_WILL_SAVE);
        let mut held = Vec::new();
        for editor in self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut())
        {
            editor.set_save_gate(gate);
            if editor.take_gated_save() {
                if let Some(path) = editor.file_path() {
                    held.push(path.to_path_buf());
                }
            }
        }
        for path in held {
            let saves = &mut self.plugins.editors.saves;
            if saves.iter().any(|job| job.path == path) {
                continue;
            }
            let queue = self
                .plugins
                .host
                .subscribers(&project, events::FILE_WILL_SAVE)
                .into();
            self.plugins.editors.saves.push(SaveJob {
                project: project.clone(),
                path,
                queue,
                waiting: None,
            });
        }

        let mut jobs = std::mem::take(&mut self.plugins.editors.saves);
        jobs.retain_mut(|job| self.advance_save(job));
        self.plugins.editors.saves.extend(jobs);
    }

    /// One step of `job`; `false` once it is done.
    fn advance_save(&mut self, job: &mut SaveJob) -> bool {
        if let Some((key, version, rx)) = &job.waiting {
            let outcome = match rx.try_recv() {
                Ok(outcome) => outcome,
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("no answer".to_string()),
            };
            let (key, version) = (key.clone(), *version);
            job.waiting = None;
            match outcome.and_then(|result| match result.get("edits") {
                Some(edits) => text_edits(edits),
                None => Ok(Vec::new()),
            }) {
                Ok(edits) if !edits.is_empty() => {
                    let Some(editor) = self.project_editor_mut(&job.project, &job.path) else {
                        return false;
                    };
                    if editor.edit_version() == version {
                        editor.apply_plugin_edits(&edits);
                    } else {
                        log::info!(
                            "plugin {}: its edits before saving came after the buffer changed",
                            key.name
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => log::warn!("plugin {}: before saving: {e}", key.name),
            }
        }
        while let Some(key) = job.queue.pop_front() {
            let Some(peer) = self.plugins.host.peer(&key) else {
                continue;
            };
            let Some(editor) = self.project_editor_mut(&job.project, &job.path) else {
                return false;
            };
            let version = editor.edit_version();
            let params = json!({ "path": job.path, "version": version });
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(peer.request(
                    events::FILE_WILL_SAVE,
                    params,
                    WILL_SAVE_TIMEOUT,
                    &|| false,
                ));
            });
            job.waiting = Some((key, version, rx));
            return true;
        }
        // Everyone was asked: write the file, as Ctrl+S would have.
        // Held apart while the editor is borrowed, and put back below.
        let lsp_manager = self.state.lsp_manager.take();
        let Some(editor) = self.project_editor_mut(&job.project, &job.path) else {
            self.state.lsp_manager = lsp_manager;
            return false;
        };
        let saved = editor.save();
        if saved.is_ok() {
            if let Some(lsp_manager) = &lsp_manager {
                editor.notify_lsp_change(lsp_manager);
            }
        }
        self.state.lsp_manager = lsp_manager;
        match saved {
            Ok(_) => {
                let name = job
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.state.set_info(format!("Saved: {name}"));
            }
            Err(e) => self.state.set_error(format!("Save failed: {e}")),
        }
        self.state.needs_redraw = true;
        false
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
        self.plugins.editors.follow_project(&project);

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

    #[test]
    fn a_switch_of_project_keeps_the_saves_the_plugins_hold() {
        let mut watch = EditorWatch::default();
        watch.follow_project(Path::new("/a"));
        watch.focused = Some(PathBuf::from("/a/x.rs"));
        watch.saves.push(SaveJob {
            project: PathBuf::from("/a"),
            path: PathBuf::from("/a/x.rs"),
            queue: VecDeque::new(),
            waiting: None,
        });
        watch.follow_project(Path::new("/b"));
        assert_eq!(watch.project.as_deref(), Some(Path::new("/b")));
        assert_eq!(watch.focused, None, "what was told of /a is forgotten");
        assert_eq!(watch.saves.len(), 1, "the Ctrl+S held for /a still goes");
    }
}
