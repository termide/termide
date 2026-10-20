//! Plugins: the programs in the global plugins directory, run for each
//! loaded project whose config lets them (see `doc/en/plugins-design.md`).
//!
//! The [`Host`] supervises the processes on threads of its own; this module
//! tells it which plugins each project wants, drains what they send on the
//! tick, answers their requests and passes them the events they asked for.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, LateTools, Tool, ToolCall, ToolContext, ToolResultMessage, ToolTraits, ToolUpdate,
};
use termide_config::{plugin_enabled, Config, PluginSettings};
use termide_plugins::protocol::{self, events};
use termide_plugins::rpc::{RpcError, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};
use termide_plugins::{
    Contributions, Host, HostEvent, Installed, Launch, PluginKey, ProcessLauncher, Timing,
};

use super::App;

/// The action prefix of a plugin command in the palette.
pub(super) const PLUGIN_COMMAND_PREFIX: &str = "plugin_command:";
/// The action prefix of a plugin's restart in the palette.
pub(super) const PLUGIN_RESTART_PREFIX: &str = "plugin_restart:";

/// How long a plugin command may take before its answer is given up on.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
use crate::PanelExt;

/// The app's side of the plugins.
pub(crate) struct PluginRuntime {
    pub(super) host: Host,
    /// What the plugins directory held at startup.
    installed: Vec<Installed>,
    /// What each ready plugin contributes.
    contributions: BTreeMap<PluginKey, Contributions>,
    /// Status items, by plugin and item id.
    status: BTreeMap<(PluginKey, String), String>,
    /// Plugins given up on after crashing: marked in their project's status
    /// bar until they run again.
    gave_up: BTreeSet<PluginKey>,
    /// The projects the host was last told about.
    synced: Vec<PathBuf>,
    /// The open projects or a config changed: sync on the next tick.
    dirty: bool,
    /// The panels plugins opened, by id, with the plugin that owns each.
    panels: BTreeMap<u64, PluginKey>,
    /// The id the next plugin panel gets.
    next_panel: u64,
    /// The windows plugins asked for.
    pub(super) windows: super::plugin_windows::Windows,
    /// What the plugins were last told of the editors.
    pub(super) editors: super::plugin_editor::EditorWatch,
}

/// What the plugins directory beside the global config file holds now.
fn discover_installed() -> Vec<Installed> {
    match Config::config_file_path() {
        Ok(config_file) => {
            let dir = config_file
                .parent()
                .map(termide_plugins::plugins_dir)
                .unwrap_or_default();
            let (installed, problems) = termide_plugins::discover(&dir);
            for problem in problems {
                log::warn!("{problem}");
            }
            if !installed.is_empty() {
                log::info!(
                    "Plugins in {}: {}",
                    dir.display(),
                    installed
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            installed
        }
        Err(e) => {
            log::warn!("No plugins: {e}");
            Vec::new()
        }
    }
}

impl PluginRuntime {
    /// Read the plugins directory beside the global config file.
    pub(crate) fn new() -> Self {
        let installed = discover_installed();
        // Sources of completion, hover and code actions, ready before any
        // editor opens: one without a server keeps asking them.
        if !installed.is_empty() {
            let _ = super::plugin_providers::provider_hub();
        }
        Self {
            host: Host::new(Arc::new(ProcessLauncher), Timing::default()),
            installed,
            contributions: BTreeMap::new(),
            status: BTreeMap::new(),
            gave_up: BTreeSet::new(),
            synced: Vec::new(),
            dirty: true,
            panels: BTreeMap::new(),
            next_panel: 1,
            windows: Default::default(),
            editors: Default::default(),
        }
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// The status items of `project`'s plugins, in plugin and id order, then
    /// a mark for each of its plugins given up on, joined for the status
    /// bar; `None` without any.
    fn status_line(&self, project: &Path) -> Option<String> {
        let gave_up = termide_i18n::t().plugins_state_gave_up();
        let items: Vec<String> = self
            .status
            .iter()
            .filter(|((key, _), _)| key.project == project)
            .map(|(_, text)| text.clone())
            .chain(
                self.gave_up
                    .iter()
                    .filter(|key| key.project == project)
                    .map(|key| format!("⚠ {}: {gave_up}", key.name)),
            )
            .collect();
        (!items.is_empty()).then(|| items.join(" · "))
    }

    /// The launches `plugins` (a project's effective `[plugins]`) allows,
    /// with one line for each plugin that cannot be started.
    fn launches(&self, plugins: &BTreeMap<String, PluginSettings>) -> (Vec<Launch>, Vec<String>) {
        let mut launches = Vec::new();
        let mut problems = Vec::new();
        for installed in &self.installed {
            if !plugin_enabled(plugins, &installed.name) {
                continue;
            }
            let program = match installed.program() {
                Ok(program) => program,
                Err(e) => {
                    problems.push(format!("plugin {}: {e}", installed.name));
                    continue;
                }
            };
            let settings = plugins
                .get(&installed.name)
                .and_then(|entry| serde_json::to_value(&entry.settings).ok())
                .unwrap_or_else(|| json!({}));
            launches.push(Launch {
                name: installed.name.clone(),
                dir: installed.dir.clone(),
                program,
                args: installed.manifest.args.clone(),
                env: installed.manifest.env.clone(),
                settings,
            });
        }
        (launches, problems)
    }
}

impl App {
    /// The roots of the projects whose panels are loaded: the current one
    /// and the parked ones. A project reopened from the last run but not
    /// entered yet has none, and no plugins either.
    fn loaded_project_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.open_projects.current().to_path_buf()];
        roots.extend(self.parked_roots());
        roots
    }

    /// Tell the host which plugins each loaded project wants.
    fn sync_plugins(&mut self) {
        self.plugins.dirty = false;
        if self.plugins.installed.is_empty() && self.plugins.synced.is_empty() {
            return;
        }
        let roots = self.loaded_project_roots();
        let current = self.open_projects.current().to_path_buf();
        let mut stopped = Vec::new();
        for root in &roots {
            let config = if *root == current {
                Ok(Arc::clone(&self.state.config))
            } else {
                Config::with_project_overlay(&self.state.global_baseline, root)
                    .map(|(config, _)| Arc::new(config))
            };
            let config = match config {
                Ok(config) => config,
                Err(e) => {
                    log::warn!("Plugins of {}: cannot read its config: {e}", root.display());
                    continue;
                }
            };
            let (launches, problems) = self.plugins.launches(&config.plugins);
            for problem in problems {
                log::warn!("{problem}");
            }
            stopped.extend(self.plugins.host.set_project(root, launches));
        }
        let closed: Vec<PathBuf> = self
            .plugins
            .synced
            .iter()
            .filter(|root| !roots.contains(root))
            .cloned()
            .collect();
        for root in &closed {
            stopped.extend(self.plugins.host.close_project(root));
            self.drop_project_windows(root);
        }
        // A plugin stopped or replaced here sends no `Stopped`: what termide
        // holds of it goes now, before a new process can be taken for it.
        for key in stopped {
            self.forget_plugin(&key);
        }
        self.plugins.synced = roots;
        self.refresh_plugin_ui();
    }

    /// Let go of all termide holds of `key`'s process — what it contributes,
    /// its tools and language features, status items, marks, windows and
    /// panels — when it stopped or was replaced: a new process knows nothing
    /// of any of it.
    fn forget_plugin(&mut self, key: &PluginKey) {
        self.plugins.contributions.remove(key);
        tool_hub().set(key, Vec::new());
        super::plugin_providers::provider_hub().set(key, None);
        self.plugins.status.retain(|(k, _), _| k != key);
        self.plugins.gave_up.remove(key);
        self.orphan_plugin_panels(key);
        self.drop_plugin_windows(key);
        self.clear_plugin_diagnostics(key);
        self.refresh_plugin_ui();
    }

    /// Show the current project's status items and commands.
    fn refresh_plugin_ui(&mut self) {
        let line = self.plugins.status_line(self.open_projects.current());
        if line != self.state.ui.plugin_status {
            self.state.ui.plugin_status = line;
            self.state.needs_redraw = true;
        }
        let commands = self.plugin_palette_commands();
        if commands != self.state.ui.plugin_commands {
            self.state.ui.plugin_commands = commands;
            // Their keys are bound from the table, built again.
            self.state.cache.hotkey_table = None;
            self.state.needs_redraw = true;
        }
    }

    /// Per tick: sync if needed, then act on what the plugins sent.
    pub(super) fn poll_plugins(&mut self) {
        if self.plugins.dirty {
            self.sync_plugins();
        }
        for event in self.plugins.host.poll() {
            self.handle_plugin_event(event);
        }
        self.notice_closed_plugin_panels();
        self.serve_plugin_windows();
        self.watch_editors_for_plugins();
        self.serve_plugin_saves();
    }

    /// Tell each plugin about its panels the user closed since the last tick.
    fn notice_closed_plugin_panels(&mut self) {
        if self.plugins.panels.is_empty() {
            return;
        }
        let open = self.plugin_panel_ids();
        let closed: Vec<(u64, PluginKey)> = self
            .plugins
            .panels
            .iter()
            .filter(|(id, _)| !open.contains(id))
            .map(|(id, key)| (*id, key.clone()))
            .collect();
        for (id, key) in closed {
            self.plugins.panels.remove(&id);
            self.plugins
                .host
                .notify(&key, protocol::PANEL_CLOSED, &json!({ "panel": id }));
        }
    }

    /// A `plugin:<action>` link was followed in plugin panel `panel`.
    pub(super) fn plugin_panel_action(&mut self, panel: u64, action: &str) {
        let Some(key) = self.plugins.panels.get(&panel) else {
            return;
        };
        self.plugins.host.notify(
            key,
            protocol::PANEL_ACTION,
            &json!({ "panel": panel, "action": action }),
        );
    }

    /// The plugin `key` stopped: its panels stay, saying so, and no longer
    /// belong to anyone — a plugin started again knows nothing of them.
    fn orphan_plugin_panels(&mut self, key: &PluginKey) {
        let orphaned: Vec<u64> = self
            .plugins
            .panels
            .iter()
            .filter(|(_, owner)| *owner == key)
            .map(|(id, _)| *id)
            .collect();
        for id in orphaned {
            self.plugins.panels.remove(&id);
            if let Some(panel) = self.plugin_panel_mut(id) {
                panel.set_plugin_content(None, format!("*The plugin {} stopped.*", key.name));
            }
        }
        self.state.needs_redraw = true;
    }

    fn handle_plugin_event(&mut self, event: HostEvent) {
        match event {
            HostEvent::Ready { key, contributions } => {
                log::info!("plugin {} ready for {}", key.name, key.project.display());
                self.plugins.gave_up.remove(&key);
                self.publish_plugin_tools(&key, &contributions);
                self.plugins.contributions.insert(key, contributions);
                self.refresh_plugin_ui();
            }
            HostEvent::Contributions { key, contributions } => {
                self.publish_plugin_tools(&key, &contributions);
                self.plugins.contributions.insert(key, contributions);
                self.refresh_plugin_ui();
            }
            HostEvent::Stopped {
                key,
                reason,
                restart_in,
            } => {
                self.forget_plugin(&key);
                if restart_in.is_none() {
                    // Marked in its project's status bar for as long as it
                    // stays off; the message only where the user is.
                    if key.project == self.open_projects.current() {
                        self.state.set_error(format!(
                            "Plugin {} stopped and will not be restarted: {reason}",
                            key.name
                        ));
                    }
                    self.plugins.gave_up.insert(key);
                    self.refresh_plugin_ui();
                }
            }
            HostEvent::Request {
                key,
                id,
                method,
                params,
            } => {
                // A window is answered when the user closes it.
                if super::plugin_windows::is_window_method(&method) {
                    self.ask_plugin_window(key, id, &method, &params);
                    return;
                }
                let outcome = self.serve_plugin_request(&key, &method, &params);
                self.plugins.host.respond(&key, &id, outcome);
            }
            HostEvent::Notification { key, method, .. } => {
                log::debug!("plugin {}: unexpected notification {method}", key.name);
            }
        }
    }

    /// Offer `key`'s tools to the agent sessions of its project.
    fn publish_plugin_tools(&self, key: &PluginKey, contributions: &Contributions) {
        let Some(peer) = self.plugins.host.peer(key) else {
            return;
        };
        let tools = contributions
            .tools
            .iter()
            .map(|info| {
                Arc::new(PluginTool::new(&key.name, info.clone(), Arc::clone(&peer)))
                    as Arc<dyn Tool>
            })
            .collect();
        tool_hub().set(key, tools);
        super::plugin_providers::provider_hub().set(
            key,
            Some((Arc::clone(&peer), contributions.providers.clone())),
        );
    }

    /// Carry out a plugin's request.
    fn serve_plugin_request(
        &mut self,
        key: &PluginKey,
        method: &str,
        params: &Value,
    ) -> Result<Value, RpcError> {
        if let Some(outcome) = self.serve_editor_request(key, method, params) {
            return outcome;
        }
        let text = |field: &str| {
            params[field]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("`{field}` must be a string")))
        };
        match method {
            protocol::WINDOW_NOTIFY => {
                let message = format!("{}: {}", key.name, text("message")?);
                // Only the current project's plugins speak on screen; a
                // parked project's would talk over another project.
                if key.project == self.open_projects.current() {
                    if params["level"].as_str() == Some("error") {
                        self.state.set_error(message);
                    } else {
                        self.state.set_info(message);
                    }
                    self.state.needs_redraw = true;
                } else {
                    log::info!("{message}");
                }
                Ok(Value::Null)
            }
            protocol::STATUS_SET => {
                let item = (key.clone(), text("id")?);
                let value = text("text")?;
                if value.is_empty() {
                    self.plugins.status.remove(&item);
                } else {
                    self.plugins.status.insert(item, value);
                }
                self.refresh_plugin_ui();
                Ok(Value::Null)
            }
            protocol::TERMINAL_RUN => {
                // In a terminal panel the user sees, with the command typed
                // in it: nothing a plugin starts this way runs unseen.
                if key.project != self.open_projects.current() {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "the plugin's project is in the background",
                    ));
                }
                let command = text("command")?;
                if !is_one_command_line(&command) {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "`command` must be one line with no control characters",
                    ));
                }
                let cwd = match params["cwd"].as_str() {
                    Some(cwd) if Path::new(cwd).is_absolute() => PathBuf::from(cwd),
                    Some(_) => return Err(RpcError::new(INVALID_PARAMS, "`cwd` must be absolute")),
                    None => key.project.clone(),
                };
                self.run_in_new_terminal(&command, Some(cwd))
                    .map_err(|e| RpcError::new(INTERNAL_ERROR, e.to_string()))?;
                self.state.needs_redraw = true;
                Ok(Value::Null)
            }
            protocol::EDITOR_DIAGNOSTICS => {
                let path = PathBuf::from(text("path")?);
                if !path.is_absolute() {
                    return Err(RpcError::new(INVALID_PARAMS, "`path` must be absolute"));
                }
                let items = plugin_diagnostics(&key.name, &params["items"])
                    .map_err(|e| RpcError::new(INVALID_PARAMS, e))?;
                self.set_plugin_diagnostics(key, path, items);
                Ok(Value::Null)
            }
            protocol::PANEL_OPEN => {
                if key.project != self.open_projects.current() {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        "the plugin's project is in the background",
                    ));
                }
                let id = self.plugins.next_panel;
                self.plugins.next_panel += 1;
                let title = plugin_title(&key.name, params["title"].as_str());
                let markdown = text("markdown")?;
                let mut panel =
                    termide_panel_markdown::MarkdownPanel::for_plugin(id, title, markdown);
                panel.set_plugin_menu(protocol::menu_items(&params["menu"]));
                self.add_panel(Box::new(panel));
                self.plugins.panels.insert(id, key.clone());
                self.state.needs_redraw = true;
                Ok(json!({ "panel": id }))
            }
            protocol::PANEL_UPDATE => {
                let id = self.owned_panel(key, params)?;
                let markdown = text("markdown")?;
                let title = params["title"]
                    .as_str()
                    .map(|title| plugin_title(&key.name, Some(title)));
                let menu = params.get("menu").map(protocol::menu_items);
                if let Some(panel) = self.plugin_panel_mut(id) {
                    panel.set_plugin_content(title, markdown);
                    if let Some(menu) = menu {
                        panel.set_plugin_menu(menu);
                    }
                }
                self.state.needs_redraw = true;
                Ok(Value::Null)
            }
            protocol::PANEL_CLOSE => {
                let id = self.owned_panel(key, params)?;
                let position = self.layout_manager.iter_all_panels_mut().position(|panel| {
                    panel
                        .as_any()
                        .downcast_ref::<termide_panel_markdown::MarkdownPanel>()
                        .and_then(termide_panel_markdown::MarkdownPanel::plugin_panel_id)
                        == Some(id)
                });
                match position {
                    Some(position) => {
                        let width = self.state.terminal.width;
                        self.layout_manager.remove_panel_at(position, width);
                        self.plugins.panels.remove(&id);
                        self.state.needs_redraw = true;
                        Ok(Value::Null)
                    }
                    None => Err(RpcError::new(
                        INVALID_PARAMS,
                        "the panel is not in the current project",
                    )),
                }
            }
            _ => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("termide offers no method {method}"),
            )),
        }
    }

    /// `key`'s marks on the file at `path` are `items` now; none clears them.
    fn set_plugin_diagnostics(
        &mut self,
        key: &PluginKey,
        path: PathBuf,
        items: Vec<lsp_types::Diagnostic>,
    ) {
        let owner = (key.project.clone(), key.name.clone());
        let by_plugin = self
            .state
            .plugin_diagnostics
            .entry(path.clone())
            .or_default();
        if items.is_empty() {
            by_plugin.remove(&owner);
        } else {
            by_plugin.insert(owner, items);
        }
        if by_plugin.is_empty() {
            self.state.plugin_diagnostics.remove(&path);
        }
        self.publish_diagnostics(path);
    }

    /// The plugin `key` is gone: its marks go with it.
    fn clear_plugin_diagnostics(&mut self, key: &PluginKey) {
        let owner = (key.project.clone(), key.name.clone());
        let paths: Vec<PathBuf> = self
            .state
            .plugin_diagnostics
            .iter()
            .filter(|(_, by_plugin)| by_plugin.contains_key(&owner))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            self.set_plugin_diagnostics(key, path, Vec::new());
        }
    }

    /// The panel `params` names, if `key` owns it.
    fn owned_panel(&self, key: &PluginKey, params: &Value) -> Result<u64, RpcError> {
        let id = params["panel"]
            .as_u64()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`panel` must be a number"))?;
        if self.plugins.panels.get(&id) == Some(key) {
            Ok(id)
        } else {
            Err(RpcError::new(
                INVALID_PARAMS,
                format!("no panel {id} of yours"),
            ))
        }
    }

    /// Pass the event `method` about `path` to the current project's plugins
    /// that asked for it.
    pub(super) fn emit_plugin_file_event(&self, method: &str, path: &Path) {
        self.plugins.host.emit(
            self.open_projects.current(),
            method,
            &json!({ "path": path }),
        );
    }

    /// Hand the paths the editors saved to their project's plugins, the
    /// parked projects' as well as the current one's.
    pub(super) fn emit_plugin_saves(&mut self) {
        let current = self.open_projects.current().to_path_buf();
        let mut saved: Vec<(PathBuf, PathBuf)> = self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut())
            .flat_map(termide_panel_editor::Editor::take_saved_paths)
            .map(|path| (current.clone(), path))
            .collect();
        saved.extend(self.take_parked_saved_paths());
        for (project, path) in saved {
            self.plugins
                .host
                .emit(&project, events::FILE_SAVED, &json!({ "path": path }));
        }
    }

    /// The current project's plugin commands for the palette: the action
    /// name each runs as, and its label.
    pub(super) fn plugin_palette_commands(&self) -> Vec<(String, String)> {
        let current = self.open_projects.current();
        self.plugins
            .contributions
            .iter()
            .filter(|(key, _)| key.project == current)
            .flat_map(|(key, contributions)| {
                contributions.commands.iter().map(move |command| {
                    (
                        format!("{PLUGIN_COMMAND_PREFIX}{}:{}", key.name, command.id),
                        format!("{}: {}", key.name, command.title),
                    )
                })
            })
            .collect()
    }

    /// The keys the current project's plugin commands may take, by action:
    /// one with `Ctrl` or `Alt` and another modifier, or an F-key with one —
    /// keys a program in the terminal rarely wants — and that nothing in
    /// termide binds already, the user's commands included. A key that does
    /// not qualify is left unbound, with a line in the log.
    pub(super) fn plugin_command_keys(&self) -> Vec<(String, String)> {
        let mut taken: Vec<termide_config::ParsedKeyBinding> =
            termide_config::enumerate_bindings(&self.state.config)
                .into_iter()
                .map(|(_, binding, _)| binding)
                .collect();
        if let Some(registry) = self.state.cache.commands_registry.as_ref() {
            taken.extend(
                registry
                    .commands_with_hotkeys()
                    .into_iter()
                    .filter_map(|(_, key)| termide_config::parse_keybinding(key).ok()),
            );
        }
        let current = self.open_projects.current();
        let mut keys = Vec::new();
        for (plugin, contributions) in &self.plugins.contributions {
            if plugin.project != current {
                continue;
            }
            for command in &contributions.commands {
                let Some(key) = &command.key else { continue };
                let Ok(parsed) = termide_config::parse_keybinding(key) else {
                    log::warn!("plugin {}: `{key}` is not a key", plugin.name);
                    continue;
                };
                let allowed = plugin_key_allowed(&parsed);
                if !allowed {
                    log::warn!(
                        "plugin {}: `{key}` needs Ctrl or Alt with another modifier, or an F-key with one",
                        plugin.name
                    );
                } else if taken.contains(&parsed) {
                    log::warn!("plugin {}: `{key}` is taken", plugin.name);
                } else {
                    taken.push(parsed);
                    keys.push((
                        format!("{PLUGIN_COMMAND_PREFIX}{}:{}", plugin.name, command.id),
                        key.clone(),
                    ));
                }
            }
        }
        keys
    }

    /// Run a plugin command named by its action, `<plugin>:<id>` after the
    /// prefix. The plugin is asked on a thread of its own: it answers when it
    /// is done, and speaks through `window/notify` if it has something to say.
    pub(super) fn run_plugin_command(&mut self, action: &str) {
        let Some((name, id)) = action.split_once(':') else {
            return;
        };
        let key = PluginKey {
            project: self.open_projects.current().to_path_buf(),
            name: name.to_string(),
        };
        let Some(peer) = self.plugins.host.peer(&key) else {
            self.state
                .set_error(format!("Plugin {name} is not running"));
            return;
        };
        let (name, id) = (name.to_string(), id.to_string());
        std::thread::spawn(move || {
            if let Err(e) = peer.request(
                protocol::COMMAND_EXECUTE,
                json!({ "id": id }),
                COMMAND_TIMEOUT,
                &|| false,
            ) {
                log::warn!("plugin {name}: command {id}: {e}");
            }
        });
    }

    /// The names of the plugins installed, for the settings.
    pub(super) fn plugin_names(&self) -> Vec<String> {
        self.plugins
            .installed
            .iter()
            .map(|p| p.name.clone())
            .collect()
    }

    /// The palette's entries for the plugins themselves: their state, a
    /// fresh read of the directory, and a restart of each the current
    /// project runs.
    pub(super) fn plugin_management_commands(&self) -> Vec<(String, String)> {
        let t = termide_i18n::t();
        let mut entries = vec![
            (
                "plugins_status".to_string(),
                t.palette_plugins_status().to_string(),
            ),
            (
                "plugins_reload".to_string(),
                t.palette_plugins_reload().to_string(),
            ),
        ];
        let current = self.open_projects.current();
        for (key, _) in self.plugins.host.states() {
            if key.project == current {
                entries.push((
                    format!("{PLUGIN_RESTART_PREFIX}{}", key.name),
                    t.palette_plugins_restart_fmt(&key.name),
                ));
            }
        }
        entries
    }

    /// The installed plugins and where each stands in the current project.
    pub(super) fn show_plugins_status(&mut self) {
        let t = termide_i18n::t();
        let current = self.open_projects.current().to_path_buf();
        let states = self.plugins.host.states();
        let lines: Vec<(String, String)> = if self.plugins.installed.is_empty() {
            vec![(String::new(), t.plugins_none().to_string())]
        } else {
            self.plugins
                .installed
                .iter()
                .map(|installed| {
                    let state = states
                        .iter()
                        .find(|(key, _)| key.project == current && key.name == installed.name)
                        .map(|(_, state)| *state);
                    let text = match state {
                        Some(termide_plugins::PluginState::Ready) => t.plugins_state_ready(),
                        Some(termide_plugins::PluginState::Starting) => t.plugins_state_starting(),
                        Some(termide_plugins::PluginState::GaveUp) => t.plugins_state_gave_up(),
                        None => t.plugins_state_off(),
                    };
                    (installed.name.clone(), text.to_string())
                })
                .collect()
        };
        let modal = termide_modal::InfoModal::new(t.plugins_title(), lines);
        self.state
            .show_message_modal(crate::state::ActiveModal::Info(Box::new(modal)));
    }

    /// Read the plugins directory again — one installed, removed or changed
    /// since takes effect — and run what the projects now want.
    pub(super) fn reload_plugins(&mut self) {
        self.plugins.installed = discover_installed();
        // The first plugins installed since termide started may answer for
        // a language: editors opened from now on ask them.
        if !self.plugins.installed.is_empty() {
            let _ = super::plugin_providers::provider_hub();
        }
        // A plugin's code may have changed with nothing in its launch to
        // show it: every one starts afresh, one given up on included.
        for root in self.plugins.synced.clone() {
            for key in self.plugins.host.close_project(&root) {
                self.forget_plugin(&key);
            }
        }
        self.sync_plugins();
        let count = self.plugins.installed.len();
        self.state
            .set_info(termide_i18n::t().plugins_reloaded_fmt(count));
    }

    /// Start the current project's plugin `name` afresh.
    pub(super) fn restart_plugin(&mut self, name: &str) {
        let key = PluginKey {
            project: self.open_projects.current().to_path_buf(),
            name: name.to_string(),
        };
        if self.plugins.host.restart(&key) {
            self.forget_plugin(&key);
        }
    }

    /// Stop every plugin, waiting briefly for each to finish.
    pub(super) fn shutdown_plugins(&mut self) {
        self.plugins.host.shutdown();
    }
}

/// Whether a plugin may bind `key`: `Ctrl` or `Alt` with another modifier,
/// or an F-key with one — what a program in the terminal rarely asks for.
fn plugin_key_allowed(key: &termide_config::ParsedKeyBinding) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mods = key.modifiers;
    let count = [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SHIFT,
    ]
    .iter()
    .filter(|m| mods.contains(**m))
    .count();
    match key.key {
        KeyCode::F(_) => count >= 1,
        _ => mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && count >= 2,
    }
}

/// The most marks a plugin sets on one file.
const MAX_DIAGNOSTICS: usize = 1000;

/// The `items` of `editor/diagnostics` as diagnostics from `plugin`.
fn plugin_diagnostics(plugin: &str, items: &Value) -> Result<Vec<lsp_types::Diagnostic>, String> {
    use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
    let items = items.as_array().ok_or("`items` must be a list")?;
    let number = |item: &Value, field: &str| {
        item[field]
            .as_u64()
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    items
        .iter()
        .take(MAX_DIAGNOSTICS)
        .map(|item| {
            let line = number(item, "line").ok_or("an item has no `line`")?;
            let message = item["message"].as_str().ok_or("an item has no `message`")?;
            let message = message
                .lines()
                .map(|l| protocol::clean_text(l, 500))
                .collect::<Vec<_>>()
                .join("\n");
            let severity = match item["severity"].as_str().unwrap_or("warning") {
                "error" => DiagnosticSeverity::ERROR,
                "warning" => DiagnosticSeverity::WARNING,
                "info" => DiagnosticSeverity::INFORMATION,
                "hint" => DiagnosticSeverity::HINT,
                other => return Err(format!("no severity {other}")),
            };
            // Without a column the whole line is marked: to the start of the
            // next one. A column alone marks the word there.
            let (start, end) = match number(item, "col") {
                None => (
                    Position::new(line, 0),
                    Position::new(line.saturating_add(1), 0),
                ),
                Some(col) => {
                    let end_line = number(item, "end_line").unwrap_or(line);
                    let end_col = number(item, "end_col").unwrap_or(col);
                    (Position::new(line, col), Position::new(end_line, end_col))
                }
            };
            Ok(Diagnostic {
                range: Range::new(start, end),
                severity: Some(severity),
                code: item["code"]
                    .as_str()
                    .map(|c| NumberOrString::String(c.to_string())),
                source: Some(plugin.to_string()),
                message,
                ..Default::default()
            })
        })
        .collect()
}

/// The plugin event an agent panel's `PanelEvent::AgentRun` is told as.
pub(super) fn agent_run_event(panel: u64, finished: Option<bool>) -> (&'static str, Value) {
    match finished {
        None => (events::AGENT_STARTED, json!({ "panel": panel })),
        Some(ok) => (events::AGENT_FINISHED, json!({ "panel": panel, "ok": ok })),
    }
}

/// The open projects a change of the repository at `repo` concerns: the one
/// it is in, or is the root of, and those inside it.
pub(super) fn projects_of_repo<'a>(
    roots: impl Iterator<Item = &'a Path>,
    repo: &Path,
) -> Vec<PathBuf> {
    roots
        .filter(|root| repo.starts_with(root) || root.starts_with(repo))
        .map(Path::to_path_buf)
        .collect()
}

/// Whether `command` is what `terminal/run` may type: one line the user
/// sees whole. A `\r` would run a second command after it, and other control
/// characters (`Ctrl+U`, escape sequences) rewrite the line out of sight.
fn is_one_command_line(command: &str) -> bool {
    !command.trim().is_empty() && !command.chars().any(char::is_control)
}

/// A plugin panel's title as termide shows it: the plugin's name, which the
/// plugin cannot choose, then its own title, cleaned. Nothing a plugin sends
/// can make its panel pass for one of termide's.
fn plugin_title(plugin: &str, title: Option<&str>) -> String {
    let title = title
        .map(|title| protocol::clean_text(title, protocol::MAX_TITLE))
        .filter(|title| !title.is_empty());
    match title {
        Some(title) => format!("{plugin}: {title}"),
        None => plugin.to_string(),
    }
}

/// How long a plugin tool may run.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// A plugin's tool as a tool of the agent. Treated as an MCP server's: the
/// permission rules ask before running it unless it declares itself
/// read-only.
struct PluginTool {
    name: String,
    description: String,
    info: termide_plugins::ToolInfo,
    peer: Arc<termide_plugins::Peer>,
}

impl PluginTool {
    fn new(
        plugin: &str,
        info: termide_plugins::ToolInfo,
        peer: Arc<termide_plugins::Peer>,
    ) -> Self {
        let description = if info.description.is_empty() {
            format!("Tool `{}` of the plugin `{plugin}`.", info.name)
        } else {
            info.description.clone()
        };
        Self {
            name: termide_agent_mcp::tool_name(&format!("plugin-{plugin}"), &info.name),
            description,
            info,
            peer,
        }
    }
}

/// The source a plugin's tools arrive from, apart from MCP servers.
fn tool_source(plugin: &str) -> String {
    format!("{}{plugin}", termide_agent_core::PLUGIN_TOOL_SOURCE)
}

impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        if self.info.input_schema.is_object() {
            self.info.input_schema.clone()
        } else {
            json!({ "type": "object", "properties": {} })
        }
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits {
            mcp: true,
            read_only: self.info.read_only,
        }
    }

    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let arguments = if call.arguments.is_object() {
            call.arguments.clone()
        } else {
            json!({})
        };
        // The request waits on a thread of its own, so that the progress the
        // plugin sends meanwhile reaches the transcript as it comes.
        let (token, progress) = termide_plugins::progress::register(&self.peer);
        let (done_tx, done) = std::sync::mpsc::channel();
        let peer = Arc::clone(&self.peer);
        let params = json!({ "name": self.info.name, "arguments": arguments, "progress": token });
        let thread_cancel = cancel.clone();
        std::thread::spawn(move || {
            let outcome = peer.request(protocol::TOOL_CALL, params, TOOL_TIMEOUT, &|| {
                thread_cancel.is_cancelled()
            });
            let _ = done_tx.send(outcome);
        });
        let outcome = loop {
            while let Ok(text) = progress.try_recv() {
                on_update(ToolUpdate::Output(text));
            }
            match done.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(outcome) => break outcome,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break Err("the plugin's call ended unanswered".to_string())
                }
            }
        };
        termide_plugins::progress::unregister(token);
        match outcome {
            Ok(result) => {
                let text = result["text"].as_str().unwrap_or_default().to_string();
                if result["isError"].as_bool() == Some(true) {
                    ToolResultMessage::error(call, text)
                } else {
                    ToolResultMessage::text(call, text)
                }
            }
            Err(error) => ToolResultMessage::error(call, error),
        }
    }
}

/// The plugins' tools, handed to the agent sessions of their project.
/// Process-wide, as the sessions are set up away from the [`App`].
#[derive(Default)]
pub(crate) struct PluginToolHub {
    inner: std::sync::Mutex<HubInner>,
}

#[derive(Default)]
struct HubInner {
    tools: BTreeMap<PluginKey, Vec<Arc<dyn Tool>>>,
    subscribers: Vec<(PathBuf, Sender<LateTools>)>,
}

/// The one hub.
pub(crate) fn tool_hub() -> &'static PluginToolHub {
    static HUB: std::sync::OnceLock<PluginToolHub> = std::sync::OnceLock::new();
    HUB.get_or_init(PluginToolHub::default)
}

impl PluginToolHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Send `project`'s plugin tools to `tx`: those there now at once, and
    /// every change after.
    pub(crate) fn subscribe(&self, project: &Path, tx: Sender<LateTools>) {
        let mut inner = self.lock();
        for (key, tools) in &inner.tools {
            if key.project == project {
                let _ = tx.send(LateTools::Ready {
                    source: tool_source(&key.name),
                    tools: tools.clone(),
                });
            }
        }
        inner.subscribers.push((project.to_path_buf(), tx));
    }

    /// `key`'s tools are now `tools`; none withdraws them.
    fn set(&self, key: &PluginKey, tools: Vec<Arc<dyn Tool>>) {
        let mut inner = self.lock();
        let had = inner.tools.contains_key(key);
        if tools.is_empty() && !had {
            return;
        }
        let event = |source: String| {
            if tools.is_empty() {
                LateTools::Gone { source }
            } else {
                LateTools::Ready {
                    source,
                    tools: tools.clone(),
                }
            }
        };
        inner.subscribers.retain(|(project, tx)| {
            *project != key.project || tx.send(event(tool_source(&key.name))).is_ok()
        });
        if tools.is_empty() {
            inner.tools.remove(key);
        } else {
            inner.tools.insert(key.clone(), tools);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{agent_run_event, is_one_command_line, plugin_title, projects_of_repo};
    use std::path::{Path, PathBuf};

    #[test]
    fn a_plugin_given_up_on_is_marked_in_its_own_projects_status_bar() {
        use super::{Host, PluginKey, PluginRuntime, ProcessLauncher, Timing};
        use std::collections::{BTreeMap, BTreeSet};
        use std::sync::Arc;
        let key = |project: &str, name: &str| PluginKey {
            project: PathBuf::from(project),
            name: name.into(),
        };
        let mut plugins = PluginRuntime {
            host: Host::new(Arc::new(ProcessLauncher), Timing::default()),
            installed: Vec::new(),
            contributions: BTreeMap::new(),
            status: BTreeMap::new(),
            gave_up: BTreeSet::new(),
            synced: Vec::new(),
            dirty: false,
            panels: BTreeMap::new(),
            next_panel: 1,
            windows: Default::default(),
            editors: Default::default(),
        };
        plugins
            .status
            .insert((key("/a", "clock"), "t".into()), "12:00".into());
        plugins.gave_up.insert(key("/a", "lint"));
        let line = plugins.status_line(Path::new("/a")).unwrap();
        assert!(line.starts_with("12:00 · ⚠ lint: "), "{line}");
        assert_eq!(plugins.status_line(Path::new("/b")), None);
    }

    #[test]
    fn a_repository_change_reaches_each_open_project_it_concerns() {
        let roots = [
            Path::new("/w/app"),
            Path::new("/w/app/crates/core"),
            Path::new("/w/other"),
            Path::new("/w/application"),
        ];
        assert_eq!(
            projects_of_repo(roots.into_iter(), Path::new("/w/app")),
            [PathBuf::from("/w/app"), PathBuf::from("/w/app/crates/core")]
        );
        // A project inside a repository that holds more than it.
        assert_eq!(
            projects_of_repo(roots.into_iter(), Path::new("/w")),
            roots.map(PathBuf::from)
        );
        assert!(projects_of_repo(roots.into_iter(), Path::new("/elsewhere")).is_empty());
    }

    #[test]
    fn an_agent_run_is_told_as_started_then_finished() {
        let (method, params) = agent_run_event(7, None);
        assert_eq!(
            (method, params),
            ("agent/started", serde_json::json!({ "panel": 7 }))
        );
        let (method, params) = agent_run_event(7, Some(false));
        assert_eq!(
            (method, params),
            (
                "agent/finished",
                serde_json::json!({ "panel": 7, "ok": false })
            )
        );
    }

    #[test]
    fn a_plugin_runs_one_visible_line_and_nothing_hidden() {
        assert!(is_one_command_line("cargo test -p x"));
        for hidden in [
            "ls\rrm -rf ~",
            "ls\n",
            "ls\u{15}rm x",
            "ls\u{1b}[2K",
            "a\tb",
            "  ",
        ] {
            assert!(!is_one_command_line(hidden), "{hidden:?}");
        }
    }

    #[test]
    fn a_plugins_marks_become_diagnostics_of_its_own() {
        use lsp_types::{DiagnosticSeverity, Position};
        let marks = super::plugin_diagnostics(
            "lint",
            &serde_json::json!([
                { "line": 3, "message": "whole line" },
                { "line": 1, "col": 4, "severity": "error", "message": "here", "code": "E1" },
            ]),
        )
        .unwrap();
        assert_eq!(marks[0].range.start, Position::new(3, 0));
        assert_eq!(marks[0].range.end, Position::new(4, 0));
        assert_eq!(marks[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(marks[1].range.end, Position::new(1, 4));
        assert_eq!(marks[1].source.as_deref(), Some("lint"));
        assert!(super::plugin_diagnostics("lint", &serde_json::json!([{ "line": 1 }])).is_err());
        assert!(super::plugin_diagnostics(
            "lint",
            &serde_json::json!([{ "line": 1, "message": "x", "severity": "fatal" }])
        )
        .is_err());
    }

    #[test]
    fn a_plugin_takes_only_keys_a_terminal_program_rarely_wants() {
        let allowed =
            |key: &str| super::plugin_key_allowed(&termide_config::parse_keybinding(key).unwrap());
        assert!(allowed("Ctrl+Alt+R"));
        assert!(allowed("Ctrl+Shift+R"));
        assert!(allowed("Alt+Shift+R"));
        assert!(allowed("Shift+F7"));
        assert!(!allowed("Ctrl+R"), "the shell's history search");
        assert!(!allowed("Alt+B"), "readline's word back");
        assert!(!allowed("F7"));
        assert!(!allowed("R"));
    }

    #[test]
    fn a_plugin_panel_is_always_headed_by_its_plugin() {
        assert_eq!(plugin_title("deploy", Some("Report")), "deploy: Report");
        assert_eq!(plugin_title("deploy", None), "deploy");
        assert_eq!(plugin_title("deploy", Some(" \u{200B} ")), "deploy");
        // A title cannot pose as another panel, or push the name away.
        assert_eq!(
            plugin_title("deploy", Some("\u{202E}Terminal\nzsh")),
            "deploy: Terminal zsh"
        );
    }
}
