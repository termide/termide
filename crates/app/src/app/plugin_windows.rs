//! Windows a plugin asks for: a question, a choice, a line of text, a form,
//! a message. Each is one of termide's own modals, headed `🧩 <plugin>` by
//! termide so that it never passes for termide's own questions; a plugin
//! picks only the kind and the words. There is no hidden input: a window
//! asking for a password is what a phishing plugin would show.
//!
//! The plugin's request is answered when the user closes the window — with
//! the answer, or `null` when it was dismissed. Windows wait in one queue
//! behind any modal already open, at most one per plugin, and show only
//! while their plugin's project is current.

use std::collections::VecDeque;

use serde_json::{json, Map, Value};
use termide_config::commands::{CommandParam, CommandParamType};
use termide_plugins::protocol;
use termide_plugins::rpc::{RpcError, INVALID_PARAMS};
use termide_plugins::PluginKey;
use termide_state::PendingAction;

use super::App;
use crate::state::ActiveModal;

/// The most fields a form holds, and the most items a choice offers.
const MAX_FIELDS: usize = 12;
const MAX_ITEMS: usize = 200;
/// The longest message, in characters.
const MAX_MESSAGE: usize = 2000;

/// What the window is.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Confirm { message: String },
    Choose { items: Vec<String> },
    Input { prompt: String, default: String },
    Form { fields: Vec<CommandParam> },
    Message { message: String },
}

/// A window asked for and not answered yet.
#[derive(Debug, Clone)]
struct Window {
    request: u64,
    key: PluginKey,
    /// The id of the plugin's request, answered when the window closes.
    rpc_id: Value,
    title: String,
    kind: Kind,
}

/// The plugins' windows: the one on screen and the ones waiting.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    open: Option<Window>,
    queue: VecDeque<Window>,
    next: u64,
}

impl Windows {
    fn holds(&self, key: &PluginKey) -> bool {
        self.open.as_ref().is_some_and(|w| w.key == *key)
            || self.queue.iter().any(|w| w.key == *key)
    }
}

/// A message from a plugin, cleaned of what could disguise it but its line
/// breaks.
fn clean_message(text: &str) -> String {
    text.lines()
        .map(|line| protocol::clean_text(line, MAX_MESSAGE))
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(MAX_MESSAGE)
        .collect()
}

/// The window `method` with `params` asks for.
fn parse(method: &str, params: &Value) -> Result<Kind, String> {
    let text = |field: &str| params[field].as_str().unwrap_or_default();
    Ok(match method {
        protocol::WINDOW_CONFIRM => Kind::Confirm {
            message: clean_message(text("message")),
        },
        protocol::WINDOW_MESSAGE => Kind::Message {
            message: clean_message(text("message")),
        },
        protocol::WINDOW_CHOOSE => {
            let items: Vec<String> = params["items"]
                .as_array()
                .ok_or("`items` must be a list of strings")?
                .iter()
                .filter_map(Value::as_str)
                .map(|item| protocol::clean_text(item, protocol::MAX_TITLE * 2))
                .take(MAX_ITEMS)
                .collect();
            if items.is_empty() {
                return Err("`items` is empty".into());
            }
            Kind::Choose { items }
        }
        protocol::WINDOW_INPUT => Kind::Input {
            prompt: protocol::clean_text(text("prompt"), protocol::MAX_TITLE * 2),
            default: protocol::clean_text(text("default"), MAX_MESSAGE),
        },
        protocol::WINDOW_FORM => {
            let fields = params["fields"]
                .as_array()
                .ok_or("`fields` must be a list")?
                .iter()
                .take(MAX_FIELDS)
                .map(|field| {
                    let mut param: CommandParam = serde_json::from_value(field.clone())
                        .map_err(|e| format!("a field: {e}"))?;
                    if param.name.is_empty() {
                        return Err("a field has no `name`".to_string());
                    }
                    // Everything of the form is the plugin's words on termide's
                    // window: cleaned like the rest.
                    param.label = protocol::clean_text(&param.label, protocol::MAX_LABEL);
                    param.options = param
                        .options
                        .iter()
                        .map(|option| protocol::clean_text(option, protocol::MAX_TITLE * 2))
                        .collect();
                    param.default = param
                        .default
                        .as_deref()
                        .map(|default| protocol::clean_text(default, MAX_MESSAGE));
                    Ok(param)
                })
                .collect::<Result<Vec<_>, String>>()?;
            if fields.is_empty() {
                return Err("`fields` is empty".into());
            }
            Kind::Form { fields }
        }
        _ => return Err(format!("no window {method}")),
    })
}

/// A window's heading as termide shows it: the puzzle piece and the
/// plugin's name, which the plugin cannot choose, then its title.
fn heading(plugin: &str, title: Option<&str>) -> String {
    match title
        .map(|title| protocol::clean_text(title, protocol::MAX_TITLE))
        .filter(|title| !title.is_empty())
    {
        Some(title) => format!("🧩 {plugin} · {title}"),
        None => format!("🧩 {plugin}"),
    }
}

/// Whether `method` asks for a window.
pub(super) fn is_window_method(method: &str) -> bool {
    method.starts_with("window/") && method != protocol::WINDOW_NOTIFY
}

impl App {
    /// Queue the window a plugin's request asks for; a malformed request,
    /// or one from a plugin that already has a window, is refused at once.
    pub(super) fn ask_plugin_window(
        &mut self,
        key: PluginKey,
        rpc_id: Value,
        method: &str,
        params: &Value,
    ) {
        let refuse = |this: &Self, message: String| {
            this.plugins
                .host
                .respond(&key, &rpc_id, Err(RpcError::new(INVALID_PARAMS, message)));
        };
        if self.plugins.windows.holds(&key) {
            return refuse(self, "busy: one window at a time".into());
        }
        let kind = match parse(method, params) {
            Ok(kind) => kind,
            Err(e) => return refuse(self, e),
        };
        let windows = &mut self.plugins.windows;
        windows.next += 1;
        let title = heading(&key.name, params["title"].as_str());
        windows.queue.push_back(Window {
            request: windows.next,
            key,
            rpc_id,
            title,
            kind,
        });
        self.serve_plugin_windows();
    }

    /// Per tick: answer a window closed without an answer, and show the next
    /// one waiting once nothing else is on screen.
    pub(super) fn serve_plugin_windows(&mut self) {
        if let Some(open) = &self.plugins.windows.open {
            let request = open.request;
            let shown = self.state.active_modal.is_some()
                && matches!(
                    self.state.pending_action,
                    Some(PendingAction::PluginWindow { request: r }) if r == request
                );
            if !shown {
                // Dismissed: the modal went without a confirmed result.
                if matches!(
                    self.state.pending_action,
                    Some(PendingAction::PluginWindow { request: r }) if r == request
                ) {
                    self.state.pending_action = None;
                }
                if let Some(window) = self.plugins.windows.open.take() {
                    self.plugins
                        .host
                        .respond(&window.key, &window.rpc_id, Ok(Value::Null));
                }
            }
        }
        if self.plugins.windows.open.is_some() || self.state.has_modal() {
            return;
        }
        let current = self.open_projects.current().to_path_buf();
        let Some(index) = self
            .plugins
            .windows
            .queue
            .iter()
            .position(|w| w.key.project == current)
        else {
            return;
        };
        let Some(window) = self.plugins.windows.queue.remove(index) else {
            return;
        };
        let modal = match &window.kind {
            Kind::Confirm { message } => ActiveModal::Confirm(Box::new(
                termide_modal::ConfirmModal::new(window.title.clone(), message.clone()),
            )),
            Kind::Message { message } => {
                ActiveModal::Info(Box::new(termide_modal::InfoModal::new(
                    window.title.clone(),
                    vec![(String::new(), message.clone())],
                )))
            }
            Kind::Choose { items } => ActiveModal::Select(Box::new(
                termide_modal::SelectModal::single(window.title.clone(), "", items.clone()),
            )),
            Kind::Input { prompt, default } => {
                ActiveModal::Input(Box::new(termide_modal::InputModal::with_default(
                    window.title.clone(),
                    prompt.clone(),
                    default.clone(),
                )))
            }
            Kind::Form { fields } => ActiveModal::CommandParams(Box::new(
                termide_modal::CommandParamsModal::new(window.key.name.clone(), fields.clone())
                    .with_title(window.title.clone()),
            )),
        };
        self.state.set_pending_action(
            PendingAction::PluginWindow {
                request: window.request,
            },
            modal,
        );
        self.state.needs_redraw = true;
        self.plugins.windows.open = Some(window);
    }

    /// The user answered window `request`: hand the answer to its plugin.
    pub(super) fn answer_plugin_window(&mut self, request: u64, value: &dyn std::any::Any) {
        let Some(window) = self.plugins.windows.open.take_if(|w| w.request == request) else {
            return;
        };
        let answer = match &window.kind {
            Kind::Confirm { .. } => json!(value.downcast_ref::<bool>().copied().unwrap_or(false)),
            Kind::Message { .. } => Value::Null,
            Kind::Choose { .. } => value
                .downcast_ref::<Vec<usize>>()
                .and_then(|picked| picked.first())
                .map_or(Value::Null, |index| json!(index)),
            Kind::Input { .. } => value
                .downcast_ref::<String>()
                .map_or(Value::Null, |text| json!(text)),
            Kind::Form { fields } => value
                .downcast_ref::<termide_modal::CommandParamsResult>()
                .map_or(Value::Null, |result| form_answer(fields, &result.values)),
        };
        self.plugins
            .host
            .respond(&window.key, &window.rpc_id, Ok(answer));
        self.state.needs_redraw = true;
    }

    /// The project at `root` closed: its plugins' windows go.
    pub(super) fn drop_project_windows(&mut self, root: &std::path::Path) {
        let keys: Vec<PluginKey> = self
            .plugins
            .windows
            .open
            .iter()
            .chain(&self.plugins.windows.queue)
            .filter(|w| w.key.project == root)
            .map(|w| w.key.clone())
            .collect();
        for key in keys {
            self.drop_plugin_windows(&key);
        }
    }

    /// The plugin `key` stopped: its windows go, the one on screen with it.
    pub(super) fn drop_plugin_windows(&mut self, key: &PluginKey) {
        self.plugins.windows.queue.retain(|w| w.key != *key);
        if self
            .plugins
            .windows
            .open
            .as_ref()
            .is_some_and(|w| w.key == *key)
        {
            self.plugins.windows.open = None;
            if matches!(
                self.state.pending_action,
                Some(PendingAction::PluginWindow { .. })
            ) {
                self.state.pending_action = None;
                self.state.close_modal();
                self.state.needs_redraw = true;
            }
        }
    }
}

/// A form's values as the plugin gets them: switches as booleans, numbers
/// as numbers where they read as one, the rest as text.
fn form_answer(
    fields: &[CommandParam],
    values: &std::collections::HashMap<String, String>,
) -> Value {
    let mut out = Map::new();
    for field in fields {
        let Some(raw) = values.get(&field.name) else {
            continue;
        };
        let value = match field.param_type {
            CommandParamType::Bool => json!(raw == "true"),
            CommandParamType::Number => {
                raw.parse::<f64>().map_or_else(|_| json!(raw), |n| json!(n))
            }
            CommandParamType::Text | CommandParamType::Select => json!(raw),
        };
        out.insert(field.name.clone(), value);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_headed_by_its_plugin() {
        assert_eq!(heading("deploy", Some("Ship it?")), "🧩 deploy · Ship it?");
        assert_eq!(heading("deploy", Some("\u{202E}")), "🧩 deploy");
        assert_eq!(heading("deploy", None), "🧩 deploy");
    }

    #[test]
    fn requests_become_windows_or_are_refused() {
        assert_eq!(
            parse("window/confirm", &json!({ "message": "Go?\nReally" })).unwrap(),
            Kind::Confirm {
                message: "Go?\nReally".into()
            }
        );
        assert!(parse("window/choose", &json!({ "items": [] })).is_err());
        assert!(parse(
            "window/form",
            &json!({ "fields": [{ "label": "x", "type": "text" }] })
        )
        .is_err());
        let Kind::Form { fields } = parse(
            "window/form",
            &json!({ "fields": [{ "name": "env", "label": "Env", "type": "select", "options": ["a", "b"] }] }),
        )
        .unwrap() else {
            panic!("a form");
        };
        assert_eq!(fields[0].options, ["a", "b"]);
        assert!(is_window_method("window/form"));
        assert!(!is_window_method("window/notify"));
    }

    #[test]
    fn a_forms_values_keep_their_types() {
        let Kind::Form { fields } = parse(
            "window/form",
            &json!({ "fields": [
                { "name": "on", "type": "bool" },
                { "name": "n", "type": "number" },
                { "name": "t", "type": "text" },
            ] }),
        )
        .unwrap() else {
            panic!("a form");
        };
        let values = std::collections::HashMap::from([
            ("on".to_string(), "true".to_string()),
            ("n".to_string(), "3".to_string()),
            ("t".to_string(), "hi".to_string()),
        ]);
        assert_eq!(
            form_answer(&fields, &values),
            json!({ "on": true, "n": 3.0, "t": "hi" })
        );
    }
}
