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

/// How long a plugin may take over `file/will_edit`: long enough for the
/// questions it asks the user on the way.
const WILL_EDIT_TIMEOUT: Duration = Duration::from_secs(300);

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
    /// Editing turns off once the file is written: a plugin asked for it
    /// when editing was turned off over unsaved work.
    then_view: bool,
}

/// Editing turned on for a read-only file, or off for a writable one,
/// which the plugins see first: the ones still to ask, and the answer
/// waited for.
#[derive(Debug)]
pub(crate) struct EditJob {
    project: PathBuf,
    path: PathBuf,
    /// Editing asked on, or off.
    edit: bool,
    queue: VecDeque<PluginKey>,
    waiting: Option<(PluginKey, Answer)>,
    /// What a plugin asked done with unsaved work before editing turns off.
    buffer: Option<UnsavedWork>,
}

/// What becomes of unsaved work when editing turns off, as a plugin's
/// answer to `file/will_edit` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsavedWork {
    /// Written first, as `Ctrl+S` would.
    Save,
    /// Dropped for the file on disk, which a revert may just have changed.
    Discard,
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
    /// `Ctrl+E`s waiting for the plugins.
    edits: Vec<EditJob>,
}

impl EditorWatch {
    /// Start afresh on a switch to `project`. The saves under way stay: each
    /// names its own project, and its editor already gave its Ctrl+S over.
    fn follow_project(&mut self, project: &Path) {
        if self.project.as_deref() != Some(project) {
            *self = Self {
                project: Some(project.to_path_buf()),
                saves: std::mem::take(&mut self.saves),
                edits: std::mem::take(&mut self.edits),
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

/// Where a job stands after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobStep {
    /// A plugin is being asked, or is about to be: again next tick.
    Pending,
    /// Every plugin was asked: the job's ending is up.
    Ready,
    /// Over with nothing more to do: a plugin dealt with it, or the editor
    /// closed.
    Dropped,
}

/// What ends a `Ctrl+E` once the plugins had their say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditEnding {
    /// The editor closed: nothing to do.
    Dropped,
    /// Write the file through the plugins that see saves, then view it.
    SaveThenView,
    /// Ask about the unsaved work, as `Ctrl+E` does.
    AskStopEditing,
    /// Turn editing on or off; `reload` first takes the file from disk.
    SetEditable { edit: bool, reload: bool },
}

/// What a job reaches: the editor of its file and the plugins. The jobs
/// decide; [`JobPlace`] carries it out in the app, the tests watch.
trait JobWorld {
    /// The editor's edit version and whether it holds unsaved work; `None`
    /// once it is closed.
    fn editor_state(&mut self) -> Option<(u64, bool)>;
    /// Apply a plugin's edits to the editor.
    fn apply(&mut self, edits: &[lsp_types::TextEdit]);
    /// Ask the plugin `key` on a thread of its own; `None` when it is not
    /// running.
    fn ask(
        &mut self,
        key: &PluginKey,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Option<Answer>;
}

/// A plugin's answer, when it came: `Err(())` while it is still awaited,
/// a failure for a plugin that went away.
fn take_answer(rx: &Answer) -> Result<Result<Value, String>, ()> {
    match rx.try_recv() {
        Ok(outcome) => Ok(outcome),
        Err(std::sync::mpsc::TryRecvError::Empty) => Err(()),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Ok(Err("no answer".to_string())),
    }
}

impl SaveJob {
    /// Take the awaited answer and apply its edits, then ask the next
    /// plugin that is running; `Ready` once all were asked.
    fn step(&mut self, world: &mut impl JobWorld) -> JobStep {
        if let Some((key, version, rx)) = &self.waiting {
            let Ok(outcome) = take_answer(rx) else {
                return JobStep::Pending;
            };
            let (key, version) = (key.clone(), *version);
            self.waiting = None;
            match outcome.and_then(|result| match result.get("edits") {
                Some(edits) => text_edits(edits),
                None => Ok(Vec::new()),
            }) {
                Ok(edits) if !edits.is_empty() => match world.editor_state() {
                    None => return JobStep::Dropped,
                    Some((now, _)) if now == version => world.apply(&edits),
                    Some(_) => log::info!(
                        "plugin {}: its edits before saving came after the buffer changed",
                        key.name
                    ),
                },
                Ok(_) => {}
                Err(e) => log::warn!("plugin {}: before saving: {e}", key.name),
            }
        }
        while let Some(key) = self.queue.pop_front() {
            let Some((version, _)) = world.editor_state() else {
                return JobStep::Dropped;
            };
            let params = json!({ "path": self.path, "version": version });
            if let Some(rx) = world.ask(&key, events::FILE_WILL_SAVE, params, WILL_SAVE_TIMEOUT) {
                self.waiting = Some((key, version, rx));
                return JobStep::Pending;
            }
        }
        JobStep::Ready
    }
}

impl EditJob {
    /// Take the awaited answer, then ask the next plugin; `Dropped` once
    /// one dealt with it, `Ready` once all were asked.
    fn step(&mut self, world: &mut impl JobWorld) -> JobStep {
        if let Some((key, rx)) = &self.waiting {
            let Ok(outcome) = take_answer(rx) else {
                return JobStep::Pending;
            };
            match outcome {
                Ok(answer) if answer["handled"] == json!(true) => return JobStep::Dropped,
                // The plugin settled what becomes of unsaved work: the
                // others are not asked again.
                Ok(answer) => {
                    self.buffer = match answer["buffer"].as_str() {
                        Some("save") => Some(UnsavedWork::Save),
                        Some("discard") => Some(UnsavedWork::Discard),
                        _ => None,
                    };
                    if self.buffer.is_some() {
                        self.queue.clear();
                    }
                }
                Err(e) => log::warn!("plugin {}: {}: {e}", key.name, events::FILE_WILL_EDIT),
            }
            self.waiting = None;
        }
        if let Some(key) = self.queue.pop_front() {
            let Some((_, modified)) = world.editor_state() else {
                return JobStep::Dropped;
            };
            let params = json!({ "path": self.path, "edit": self.edit, "modified": modified });
            // One not running is passed over; the next is asked next tick.
            self.waiting = world
                .ask(&key, events::FILE_WILL_EDIT, params, WILL_EDIT_TIMEOUT)
                .map(|rx| (key, rx));
            return JobStep::Pending;
        }
        JobStep::Ready
    }

    /// What ends the job once every plugin was asked and none dealt with
    /// it: the unsaved work as a plugin asked, else editing on or off as
    /// `Ctrl+E` would have done, asking about unsaved work as it does.
    fn ending(&self, world: &mut impl JobWorld) -> EditEnding {
        if self.buffer == Some(UnsavedWork::Save) {
            return EditEnding::SaveThenView;
        }
        let Some((_, modified)) = world.editor_state() else {
            return EditEnding::Dropped;
        };
        if !self.edit && self.buffer.is_none() && modified {
            return EditEnding::AskStopEditing;
        }
        EditEnding::SetEditable {
            edit: self.edit,
            reload: self.buffer == Some(UnsavedWork::Discard),
        }
    }
}

/// A job's file in the app: its project's editor and plugins.
struct JobPlace<'a> {
    app: &'a mut App,
    project: PathBuf,
    path: PathBuf,
}

impl<'a> JobPlace<'a> {
    fn of(app: &'a mut App, project: &Path, path: &Path) -> Self {
        Self {
            app,
            project: project.to_path_buf(),
            path: path.to_path_buf(),
        }
    }
}

impl JobWorld for JobPlace<'_> {
    fn editor_state(&mut self) -> Option<(u64, bool)> {
        self.app
            .project_editor_mut(&self.project, &self.path)
            .map(|editor| (editor.edit_version(), editor.buffer().is_modified()))
    }

    fn apply(&mut self, edits: &[lsp_types::TextEdit]) {
        if let Some(editor) = self.app.project_editor_mut(&self.project, &self.path) {
            editor.apply_plugin_edits(edits);
        }
    }

    fn ask(
        &mut self,
        key: &PluginKey,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Option<Answer> {
        let peer = self.app.plugins.host.peer(key)?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(peer.request(method, params, timeout, &|| false));
        });
        Some(rx)
    }
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
        // Positions count as LSP counts them (see `doc/en/plugins.md`).
        self.process_single_event(termide_core::PanelEvent::OpenFileAtLsp {
            path,
            line,
            character: column,
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
                then_view: false,
            });
        }

        let mut jobs = std::mem::take(&mut self.plugins.editors.saves);
        jobs.retain_mut(|job| self.advance_save(job));
        self.plugins.editors.saves.extend(jobs);
    }

    /// One step of `job`; `false` once it is done.
    fn advance_save(&mut self, job: &mut SaveJob) -> bool {
        let mut place = JobPlace::of(self, &job.project, &job.path);
        match job.step(&mut place) {
            JobStep::Pending => return true,
            JobStep::Dropped => return false,
            JobStep::Ready => {}
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
            if job.then_view {
                editor.set_editable(false);
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
                self.state
                    .set_info(termide_i18n::t().status_file_saved(&name));
            }
            Err(e) => self
                .state
                .set_error(termide_i18n::t().status_error_save(&e.to_string())),
        }
        self.state.needs_redraw = true;
        false
    }

    /// Per tick: hold turning editing on for a file that is read-only on
    /// disk, or off for a saved writable one, for the plugins that open
    /// files for edit, and ask them one after another. A plugin that dealt
    /// with it leaves the rest to the file's permission, which the editor
    /// follows; when none did, editing is turned on or off as it would have
    /// been.
    pub(super) fn serve_plugin_edits(&mut self) {
        let project = self.open_projects.current().to_path_buf();
        let gate = self.plugins.host.wants(&project, events::FILE_WILL_EDIT);
        let mut held = Vec::new();
        for editor in self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut())
        {
            editor.set_edit_gate(gate);
            if let Some(edit) = editor.take_gated_edit() {
                if let Some(path) = editor.file_path() {
                    held.push((path.to_path_buf(), edit));
                }
            }
        }
        for (path, edit) in held {
            if self
                .plugins
                .editors
                .edits
                .iter()
                .any(|job| job.path == path)
            {
                continue;
            }
            let queue = self
                .plugins
                .host
                .subscribers(&project, events::FILE_WILL_EDIT)
                .into();
            self.plugins.editors.edits.push(EditJob {
                project: project.clone(),
                path,
                edit,
                queue,
                waiting: None,
                buffer: None,
            });
        }

        let mut jobs = std::mem::take(&mut self.plugins.editors.edits);
        jobs.retain_mut(|job| self.advance_edit(job));
        self.plugins.editors.edits.extend(jobs);
    }

    /// Write the file of `path` as `Ctrl+S` does, through the plugins that
    /// see it first, and turn editing off once it is written.
    pub(super) fn save_then_view(&mut self, project: &Path, path: &Path) {
        if let Some(save) = self
            .plugins
            .editors
            .saves
            .iter_mut()
            .find(|save| save.path == path)
        {
            save.then_view = true;
            return;
        }
        let queue = self
            .plugins
            .host
            .subscribers(project, events::FILE_WILL_SAVE)
            .into();
        self.plugins.editors.saves.push(SaveJob {
            project: project.to_path_buf(),
            path: path.to_path_buf(),
            queue,
            waiting: None,
            then_view: true,
        });
    }

    /// One step of `job`; `false` once it is done.
    fn advance_edit(&mut self, job: &mut EditJob) -> bool {
        let mut place = JobPlace::of(self, &job.project, &job.path);
        let ending = match job.step(&mut place) {
            JobStep::Pending => return true,
            JobStep::Dropped => return false,
            JobStep::Ready => job.ending(&mut place),
        };
        match ending {
            EditEnding::Dropped => {}
            EditEnding::SaveThenView => self.save_then_view(&job.project, &job.path),
            EditEnding::AskStopEditing => self.ask_stop_editing(job.path.clone()),
            EditEnding::SetEditable { edit, reload } => {
                let Some(editor) = self.project_editor_mut(&job.project, &job.path) else {
                    return false;
                };
                if reload {
                    if let Err(e) = editor.reload_from_disk() {
                        self.state
                            .set_error(termide_i18n::t().status_error_reload(&e.to_string()));
                        return false;
                    }
                }
                editor.set_editable(edit);
                self.state.needs_redraw = true;
            }
        }
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
            then_view: false,
        });
        watch.follow_project(Path::new("/b"));
        assert_eq!(watch.project.as_deref(), Some(Path::new("/b")));
        assert_eq!(watch.focused, None, "what was told of /a is forgotten");
        assert_eq!(watch.saves.len(), 1, "the Ctrl+S held for /a still goes");
    }

    type Reply = std::sync::mpsc::Sender<Result<Value, String>>;

    /// An editor and plugins as a job sees them.
    struct Fake {
        /// The editor's version and unsaved work; `None` once it closed.
        editor: Option<(u64, bool)>,
        /// The plugins running.
        running: BTreeSet<&'static str>,
        /// Each question asked: the plugin, the method, the params, and
        /// where its answer goes.
        asked: Vec<(String, &'static str, Value, Reply)>,
        applied: Vec<Vec<lsp_types::TextEdit>>,
    }

    impl Fake {
        fn new(running: &[&'static str]) -> Self {
            Self {
                editor: Some((7, true)),
                running: running.iter().copied().collect(),
                asked: Vec::new(),
                applied: Vec::new(),
            }
        }

        /// The plugin asked last answers.
        fn answer(&self, outcome: Result<Value, String>) {
            self.asked.last().unwrap().3.send(outcome).unwrap();
        }

        fn asked(&self) -> Vec<&str> {
            self.asked.iter().map(|(name, ..)| name.as_str()).collect()
        }
    }

    impl JobWorld for Fake {
        fn editor_state(&mut self) -> Option<(u64, bool)> {
            self.editor
        }

        fn apply(&mut self, edits: &[lsp_types::TextEdit]) {
            self.applied.push(edits.to_vec());
        }

        fn ask(
            &mut self,
            key: &PluginKey,
            method: &'static str,
            params: Value,
            _timeout: Duration,
        ) -> Option<Answer> {
            if !self.running.contains(key.name.as_str()) {
                return None;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            self.asked.push((key.name.clone(), method, params, tx));
            Some(rx)
        }
    }

    fn keys(names: &[&str]) -> VecDeque<PluginKey> {
        names
            .iter()
            .map(|name| PluginKey {
                project: PathBuf::from("/p"),
                name: (*name).to_string(),
            })
            .collect()
    }

    fn save_job(names: &[&str]) -> SaveJob {
        SaveJob {
            project: PathBuf::from("/p"),
            path: PathBuf::from("/p/a.txt"),
            queue: keys(names),
            waiting: None,
            then_view: false,
        }
    }

    fn edit_job(names: &[&str], edit: bool) -> EditJob {
        EditJob {
            project: PathBuf::from("/p"),
            path: PathBuf::from("/p/a.txt"),
            edit,
            queue: keys(names),
            waiting: None,
            buffer: None,
        }
    }

    fn trim() -> Value {
        json!({ "edits": [{ "line": 0, "col": 3, "end_col": 5, "text": "" }] })
    }

    /// A plugin's edits land when the buffer is as it was asked about; one
    /// not running is passed over; the file is written once all answered.
    #[test]
    fn a_save_applies_the_edits_of_each_running_plugin_then_writes() {
        let mut world = Fake::new(&["trim", "fmt"]);
        let mut job = save_job(&["gone", "trim", "fmt"]);
        assert_eq!(job.step(&mut world), JobStep::Pending);
        assert_eq!(
            world.asked(),
            ["trim"],
            "the one not running is passed over"
        );
        assert_eq!(world.asked[0].1, events::FILE_WILL_SAVE);
        assert_eq!(world.asked[0].2["version"], 7);
        assert_eq!(job.step(&mut world), JobStep::Pending, "no answer yet");

        world.answer(Ok(trim()));
        assert_eq!(job.step(&mut world), JobStep::Pending);
        assert_eq!(world.applied.len(), 1);
        assert_eq!(world.asked(), ["trim", "fmt"]);

        world.answer(Ok(Value::Null));
        assert_eq!(
            job.step(&mut world),
            JobStep::Ready,
            "now the file is written"
        );
        assert_eq!(
            world.applied.len(),
            1,
            "an answer with no edits changes nothing"
        );
    }

    /// Edits for a buffer that changed while the plugin worked would land
    /// in the wrong places: they are dropped, and the save goes on.
    #[test]
    fn a_saves_edits_for_a_changed_buffer_are_dropped() {
        let mut world = Fake::new(&["trim"]);
        let mut job = save_job(&["trim"]);
        job.step(&mut world);
        world.editor = Some((8, true));
        world.answer(Ok(trim()));
        assert_eq!(job.step(&mut world), JobStep::Ready);
        assert!(world.applied.is_empty());
    }

    /// A plugin that fails or goes away does not hold the save up.
    #[test]
    fn a_save_goes_on_past_a_failed_or_vanished_plugin() {
        let mut world = Fake::new(&["a", "b"]);
        let mut job = save_job(&["a", "b"]);
        job.step(&mut world);
        world.answer(Err("timed out".into()));
        assert_eq!(job.step(&mut world), JobStep::Pending);
        world.asked.pop();
        assert_eq!(
            job.step(&mut world),
            JobStep::Ready,
            "its answer can no longer come"
        );
        assert!(world.applied.is_empty());
    }

    /// The editor closed while a plugin worked: nothing is left to write.
    #[test]
    fn a_save_ends_when_its_editor_closes() {
        let mut world = Fake::new(&["trim", "fmt"]);
        let mut job = save_job(&["trim", "fmt"]);
        job.step(&mut world);
        world.editor = None;
        world.answer(Ok(trim()));
        assert_eq!(job.step(&mut world), JobStep::Dropped);
        assert!(world.applied.is_empty());

        let mut job = save_job(&["trim"]);
        assert_eq!(
            job.step(&mut world),
            JobStep::Dropped,
            "before asking anyone"
        );
        assert_eq!(world.asked.len(), 1);
    }

    /// With no plugin running the file is written straight away.
    #[test]
    fn a_save_with_no_plugin_running_writes_at_once() {
        let mut world = Fake::new(&[]);
        assert_eq!(save_job(&["a", "b"]).step(&mut world), JobStep::Ready);
        assert!(world.asked.is_empty());
    }

    /// A plugin that handled `Ctrl+E` ends it: the others are not asked.
    #[test]
    fn an_edit_a_plugin_handled_ends_there() {
        let mut world = Fake::new(&["p4", "other"]);
        let mut job = edit_job(&["p4", "other"], true);
        assert_eq!(job.step(&mut world), JobStep::Pending);
        assert_eq!(world.asked[0].1, events::FILE_WILL_EDIT);
        assert_eq!(
            world.asked[0].2,
            json!({ "path": "/p/a.txt", "edit": true, "modified": true })
        );
        assert_eq!(job.step(&mut world), JobStep::Pending, "no answer yet");
        world.answer(Ok(json!({ "handled": true })));
        assert_eq!(job.step(&mut world), JobStep::Dropped);
        assert_eq!(world.asked(), ["p4"]);
    }

    /// A plugin that asks for the unsaved work to be saved settles it: the
    /// others are not asked, and the file is written, then viewed.
    #[test]
    fn an_edit_whose_unsaved_work_a_plugin_saves_is_written_then_viewed() {
        let mut world = Fake::new(&["p4", "other"]);
        let mut job = edit_job(&["p4", "other"], false);
        job.step(&mut world);
        world.answer(Ok(json!({ "buffer": "save" })));
        assert_eq!(job.step(&mut world), JobStep::Ready);
        assert_eq!(world.asked(), ["p4"]);
        assert_eq!(job.ending(&mut world), EditEnding::SaveThenView);
    }

    /// Discarding takes the file from disk before editing turns off, and
    /// no question about the unsaved work is asked.
    #[test]
    fn an_edit_whose_unsaved_work_a_plugin_discards_reloads_the_file() {
        let mut world = Fake::new(&["p4"]);
        let mut job = edit_job(&["p4"], false);
        job.step(&mut world);
        world.answer(Ok(json!({ "buffer": "discard" })));
        assert_eq!(job.step(&mut world), JobStep::Ready);
        assert_eq!(
            job.ending(&mut world),
            EditEnding::SetEditable {
                edit: false,
                reload: true
            }
        );
    }

    /// When no plugin dealt with it, `Ctrl+E` does what it does without
    /// plugins: asks about unsaved work before editing turns off.
    #[test]
    fn an_edit_no_plugin_dealt_with_goes_as_ctrl_e_would() {
        let mut world = Fake::new(&["p4"]);
        let mut job = edit_job(&["gone", "p4"], false);
        assert_eq!(job.step(&mut world), JobStep::Pending, "one not running");
        assert!(world.asked.is_empty());
        job.step(&mut world);
        world.answer(Ok(json!({ "handled": false })));
        assert_eq!(job.step(&mut world), JobStep::Ready);
        assert_eq!(job.ending(&mut world), EditEnding::AskStopEditing);

        world.editor = Some((7, false));
        assert_eq!(
            job.ending(&mut world),
            EditEnding::SetEditable {
                edit: false,
                reload: false
            }
        );
        let on = edit_job(&[], true);
        world.editor = Some((7, true));
        assert_eq!(
            on.ending(&mut world),
            EditEnding::SetEditable {
                edit: true,
                reload: false
            }
        );
    }

    /// The editor closed while a plugin was asked: the job ends.
    #[test]
    fn an_edit_ends_when_its_editor_closes() {
        let mut world = Fake::new(&["p4", "other"]);
        let mut job = edit_job(&["p4", "other"], false);
        job.step(&mut world);
        world.editor = None;
        world.answer(Ok(json!({ "handled": false })));
        assert_eq!(job.step(&mut world), JobStep::Dropped);
        assert_eq!(edit_job(&[], true).ending(&mut world), EditEnding::Dropped);
    }
}
