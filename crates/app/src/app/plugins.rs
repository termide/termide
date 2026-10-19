//! Plugins: the programs in the global plugins directory, run for each
//! loaded project whose config lets them (see `doc/en/plugins-design.md`).
//!
//! The [`Host`] supervises the processes on threads of its own; this module
//! tells it which plugins each project wants, drains what they send on the
//! tick, answers their requests and passes them the events they asked for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, LateTools, Tool, ToolCall, ToolContext, ToolResultMessage, ToolTraits, ToolUpdate,
};
use termide_config::{plugin_enabled, Config, PluginSettings};
use termide_plugins::protocol::{self, events};
use termide_plugins::rpc::{RpcError, INVALID_PARAMS, METHOD_NOT_FOUND};
use termide_plugins::{
    Contributions, Host, HostEvent, Installed, Launch, PluginKey, ProcessLauncher, Timing,
};

use super::App;

/// The action prefix of a plugin command in the palette.
pub(super) const PLUGIN_COMMAND_PREFIX: &str = "plugin_command:";

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
}

impl PluginRuntime {
    /// Read the plugins directory beside the global config file.
    pub(crate) fn new() -> Self {
        let installed = match Config::config_file_path() {
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
        };
        Self {
            host: Host::new(Arc::new(ProcessLauncher), Timing::default()),
            installed,
            contributions: BTreeMap::new(),
            status: BTreeMap::new(),
            synced: Vec::new(),
            dirty: true,
            panels: BTreeMap::new(),
            next_panel: 1,
            windows: Default::default(),
        }
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// The status items of `project`'s plugins, in plugin and id order,
    /// joined for the status bar; `None` without any.
    fn status_line(&self, project: &Path) -> Option<String> {
        let items: Vec<&str> = self
            .status
            .iter()
            .filter(|((key, _), _)| key.project == project)
            .map(|(_, text)| text.as_str())
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
        if self.plugins.installed.is_empty() {
            return;
        }
        let roots = self.loaded_project_roots();
        let current = self.open_projects.current().to_path_buf();
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
            self.plugins.host.set_project(root, launches);
        }
        for gone in self
            .plugins
            .synced
            .iter()
            .filter(|root| !roots.contains(root))
        {
            self.plugins.host.close_project(gone);
        }
        let gone: Vec<PathBuf> = self
            .plugins
            .synced
            .iter()
            .filter(|root| !roots.contains(root))
            .cloned()
            .collect();
        for root in gone {
            self.drop_project_windows(&root);
        }
        let host = &self.plugins.host;
        let alive = host.states();
        self.plugins
            .contributions
            .retain(|key, _| alive.iter().any(|(k, _)| k == key));
        self.plugins
            .status
            .retain(|(key, _), _| alive.iter().any(|(k, _)| k == key));
        for key in tool_hub().keys() {
            if !alive.iter().any(|(k, _)| *k == key) {
                tool_hub().set(&key, Vec::new());
            }
        }
        self.plugins.synced = roots;
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
                self.plugins.contributions.remove(&key);
                tool_hub().set(&key, Vec::new());
                self.orphan_plugin_panels(&key);
                self.drop_plugin_windows(&key);
                self.plugins.status.retain(|(k, _), _| *k != key);
                self.refresh_plugin_ui();
                if restart_in.is_none() {
                    self.state.set_error(format!(
                        "Plugin {} stopped and will not be restarted: {reason}",
                        key.name
                    ));
                    self.state.needs_redraw = true;
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
    }

    /// Carry out a plugin's request.
    fn serve_plugin_request(
        &mut self,
        key: &PluginKey,
        method: &str,
        params: &Value,
    ) -> Result<Value, RpcError> {
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

    /// Stop every plugin, waiting briefly for each to finish.
    pub(super) fn shutdown_plugins(&mut self) {
        self.plugins.host.shutdown();
    }
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
        _on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let arguments = if call.arguments.is_object() {
            call.arguments.clone()
        } else {
            json!({})
        };
        match self.peer.request(
            protocol::TOOL_CALL,
            json!({ "name": self.info.name, "arguments": arguments }),
            TOOL_TIMEOUT,
            &|| cancel.is_cancelled(),
        ) {
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

    /// The keys holding tools now.
    fn keys(&self) -> Vec<PluginKey> {
        self.lock().tools.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::plugin_title;

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
