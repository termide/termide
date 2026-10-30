//! The plugin's end of the protocol: requests to termide, and the loop that
//! hands termide's messages to the plugin, each on a thread of its own so a
//! handler may ask termide and wait while termide goes on talking.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use termide_plugins::protocol;
use termide_plugins::rpc::{Incoming, Peer, RpcError, INTERNAL_ERROR, METHOD_NOT_FOUND};

/// How long a request to termide may wait: a question waits on the user.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// What the plugin asks termide.
#[derive(Clone)]
pub struct Termide {
    peer: Arc<Peer>,
}

impl Termide {
    /// Ask and wait for the answer.
    ///
    /// # Errors
    ///
    /// termide answered with an error, or the connection is gone.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.peer.request(method, params, ANSWER_TIMEOUT, &|| false)
    }

    pub fn notify(&self, message: &str, level: &str) {
        let _ = self.request(
            protocol::WINDOW_NOTIFY,
            json!({ "message": message, "level": level }),
        );
    }

    /// Show `text` in the status bar under `id`; an empty text removes it.
    pub fn status(&self, id: &str, text: &str) {
        let _ = self.request(protocol::STATUS_SET, json!({ "id": id, "text": text }));
    }

    /// How the file manager colours each path; `None` says nothing again.
    pub fn file_status(&self, items: &[(PathBuf, Option<&str>)]) {
        if items.is_empty() {
            return;
        }
        let items: Vec<Value> = items
            .iter()
            .map(|(path, status)| json!({ "path": path, "status": status }))
            .collect();
        let _ = self.request(protocol::FILES_STATUS, json!({ "items": items }));
    }

    /// The text the editor marks changed lines of `path` against; `None`
    /// takes it back.
    pub fn base(&self, path: &Path, text: Option<&str>) {
        let _ = self.request(protocol::EDITOR_BASE, json!({ "path": path, "text": text }));
    }

    /// The active editor: `{ "path", … }`, or `Null` without one.
    #[must_use]
    pub fn active(&self) -> Value {
        self.request(protocol::EDITOR_ACTIVE, json!({}))
            .unwrap_or(Value::Null)
    }

    /// Whether the user said yes.
    #[must_use]
    pub fn confirm(&self, title: &str, message: &str) -> bool {
        self.request(
            protocol::WINDOW_CONFIRM,
            json!({ "title": title, "message": message }),
        )
        .is_ok_and(|answer| answer == json!(true))
    }

    /// The index of the item picked, or `None` when the window was
    /// dismissed.
    #[must_use]
    pub fn choose(&self, title: &str, items: &[&str]) -> Option<usize> {
        self.request(
            protocol::WINDOW_CHOOSE,
            json!({ "title": title, "items": items }),
        )
        .ok()
        .and_then(|answer| answer.as_u64())
        .and_then(|index| usize::try_from(index).ok())
    }

    /// What the user typed, or `None` when the window was dismissed.
    #[must_use]
    pub fn ask(&self, title: &str, prompt: &str) -> Option<String> {
        self.request(
            protocol::WINDOW_INPUT,
            json!({ "title": title, "prompt": prompt, "default": "" }),
        )
        .ok()
        .and_then(|answer| answer.as_str().map(str::to_string))
    }

    pub fn message(&self, title: &str, message: &str) {
        let _ = self.request(
            protocol::WINDOW_MESSAGE,
            json!({ "title": title, "message": message }),
        );
    }

    pub fn run_in_terminal(&self, command: &str, cwd: &Path) {
        let _ = self.request(
            protocol::TERMINAL_RUN,
            json!({ "command": command, "cwd": cwd }),
        );
    }

    pub fn open_file(&self, path: &str) {
        let _ = self.request(protocol::EDITOR_OPEN, json!({ "path": path }));
    }

    /// Open a panel; its id.
    ///
    /// Open a panel of `diff`, a unified diff, shown as git's are.
    ///
    /// # Errors
    ///
    /// When termide does not open it.
    pub fn open_diff(&self, title: &str, diff: &str) -> Result<u64, String> {
        self.open_with(title, "diff", json!(diff), &[])
    }

    /// Open a panel of a `list` (groups of rows with their actions).
    ///
    /// # Errors
    ///
    /// When termide does not open it.
    pub fn open_list(
        &self,
        title: &str,
        list: Value,
        menu: &[(&str, &str)],
    ) -> Result<u64, String> {
        self.open_with(title, "list", list, menu)
    }

    fn open_with(
        &self,
        title: &str,
        kind: &str,
        content: Value,
        menu: &[(&str, &str)],
    ) -> Result<u64, String> {
        let menu: Vec<Value> = menu
            .iter()
            .map(|(label, action)| json!({ "label": label, "action": action }))
            .collect();
        let mut params = json!({ "title": title });
        params[kind] = content;
        if !menu.is_empty() {
            params["menu"] = Value::Array(menu);
        }
        let answer = self.request(protocol::PANEL_OPEN, params)?;
        answer["panel"]
            .as_u64()
            .ok_or_else(|| "panel/open gave no panel".to_string())
    }

    /// Show a new `list` in a list panel.
    ///
    /// # Errors
    ///
    /// When the panel is gone.
    pub fn update_list(&self, panel: u64, list: Value) -> Result<(), String> {
        self.request(
            protocol::PANEL_UPDATE,
            json!({ "panel": panel, "list": list }),
        )
        .map(drop)
    }
}

/// What a plugin does with termide's messages.
pub trait Handler: Send + Sync + 'static {
    /// The answer to `method`: a request's result, or an error. A
    /// notification's answer is dropped.
    ///
    /// # Errors
    ///
    /// When the work fails; the message is shown to whoever asked.
    fn handle(&self, method: &str, params: &Value) -> Result<Value, RpcError>;
}

/// Serve termide over `reader` and `writer` until it says `exit` or closes
/// the connection. `initialize` gets `start`'s answer: the handler for the
/// rest, and the contributions to answer with.
pub fn serve<H: Handler>(
    reader: impl Read + Send + 'static,
    writer: impl Write + Send + 'static,
    start: impl FnOnce(&Termide, &Value) -> (H, Value) + Send + 'static,
) {
    let (done_tx, done) = mpsc::channel::<()>();
    let closed_tx = done_tx.clone();
    let slot: Arc<OnceLock<Termide>> = Arc::new(OnceLock::new());
    let handler: Arc<OnceLock<Arc<H>>> = Arc::new(OnceLock::new());
    let start = std::sync::Mutex::new(Some(start));
    let incoming_slot = Arc::clone(&slot);
    let peer = Arc::new(Peer::new(
        reader,
        writer,
        move |incoming| {
            // The reader starts before the peer is put in the slot.
            let termide = incoming_slot.wait().clone();
            let (id, method, params) = match incoming {
                Incoming::Request { id, method, params } => (Some(id), method, params),
                Incoming::Notification { method, params } => (None, method, params),
            };
            if method == protocol::EXIT {
                let _ = done_tx.send(());
                return;
            }
            if method == protocol::INITIALIZE {
                let begin = start
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let outcome = match begin {
                    Some(begin) => {
                        let (made, contributions) = begin(&termide, &params);
                        let _ = handler.set(Arc::new(made));
                        Ok(contributions)
                    }
                    None => Err(RpcError::new(INTERNAL_ERROR, "already initialized")),
                };
                if let Some(id) = id {
                    let _ = termide.peer.respond(&id, outcome);
                }
                return;
            }
            let Some(handler) = handler.get().cloned() else {
                if let Some(id) = id {
                    let _ = termide
                        .peer
                        .respond(&id, Err(RpcError::new(INTERNAL_ERROR, "not initialized")));
                }
                return;
            };
            if method == termide_plugins::rpc::CANCEL_REQUEST {
                return;
            }
            std::thread::spawn(move || {
                let outcome = if method == protocol::SHUTDOWN {
                    Ok(Value::Null)
                } else {
                    handler.handle(&method, &params)
                };
                if let Some(id) = id {
                    let _ = termide.peer.respond(&id, outcome);
                }
            });
        },
        move || {
            let _ = closed_tx.send(());
        },
    ));
    let _ = slot.set(Termide { peer });
    let _ = done.recv();
}

/// The error a handler gives for a method it does not know.
#[must_use]
pub fn unknown_method(method: &str) -> RpcError {
    RpcError::new(METHOD_NOT_FOUND, format!("no method {method}"))
}
