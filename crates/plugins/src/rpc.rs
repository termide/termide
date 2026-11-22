//! JSON-RPC 2.0 over a pair of streams, one message per line, in both
//! directions: either side may send requests and notifications.
//!
//! Blocking I/O on a reader thread, as the MCP client does, and on a writer
//! thread fed by a bounded queue, so a sender never waits on the pipe: a
//! plugin that stops reading has its messages refused. Replies to our
//! requests are handed to the waiting caller over a channel; the other
//! side's requests and notifications go to the `on_incoming` callback, and a
//! request is answered later with [`Peer::respond`], from any thread, so the
//! callback never has to wait for the UI.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Sent when a request is given up on — cancelled, or past its timeout —
/// so the other side can stop working on it: `{ "id" }`, as LSP does.
pub const CANCEL_REQUEST: &str = "$/cancelRequest";
const ABORTED: &str = "aborted";
const NO_REPLY: &str = "no reply within";

/// JSON-RPC's "method not found".
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's "invalid params".
pub const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC's "internal error": the request was fine, carrying it out failed.
pub const INTERNAL_ERROR: i64 = -32603;

/// A message the other side sent unasked.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A request, owed an answer through [`Peer::respond`] under `id`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// An error answer: a JSON-RPC code and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Requests waiting for their answer, and whether the connection ended: a
/// request made after that fails at once instead of waiting it out.
#[derive(Default)]
struct Waiting {
    replies: HashMap<u64, Sender<Result<Value, String>>>,
    closed: bool,
}

impl Waiting {
    /// The connection is gone: every waiting request learns it now.
    fn close(&mut self) {
        self.closed = true;
        self.replies.clear();
    }
}

type Pending = Arc<Mutex<Waiting>>;

/// How many messages may wait for the writer thread. Past it the other side
/// is taken to have stopped reading: a message is refused rather than left
/// to block the caller, which is often the UI thread.
const OUTBOX_CAPACITY: usize = 1024;

/// One end of the connection.
///
/// Messages are written by a thread of its own, in the order they were
/// sent, so a peer that stops reading never blocks a sender: once the
/// bounded outbox is full, sending fails at once.
pub struct Peer {
    outbox: SyncSender<String>,
    /// Set while the outbox is full, so the overflow is logged once per
    /// stall rather than once per message.
    overflowing: AtomicBool,
    pending: Pending,
    next_id: AtomicU64,
}

impl Peer {
    /// Speak over `reader` and `writer`, handing what the other side sends
    /// unasked to `on_incoming` on the reader thread. `on_closed` runs on
    /// that thread once the reader ends.
    pub fn new(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        on_incoming: impl Fn(Incoming) + Send + 'static,
        on_closed: impl FnOnce() + Send + 'static,
    ) -> Self {
        let pending: Pending = Arc::default();
        let reader_pending = Arc::clone(&pending);
        std::thread::spawn(move || {
            read_loop(reader, &reader_pending, &on_incoming);
            on_closed();
        });
        let (outbox, queued) = mpsc::sync_channel(OUTBOX_CAPACITY);
        let writer_pending = Arc::clone(&pending);
        std::thread::spawn(move || write_loop(writer, &queued, &writer_pending));
        Self {
            outbox,
            overflowing: AtomicBool::new(false),
            pending,
            next_id: AtomicU64::new(1),
        }
    }

    /// Send a request and wait for its result, up to `timeout`, giving up
    /// early when `cancelled` turns true.
    ///
    /// # Errors
    ///
    /// The other side answered with an error, the connection closed, the
    /// wait ran out, or it was cancelled.
    pub fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending_map();
            if pending.closed {
                return Err(format!("{method}: the plugin closed the connection"));
            }
            pending.replies.insert(id, tx);
        }
        let sent = self.write(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        let outcome = sent.and_then(|()| wait(&rx, timeout, cancelled));
        if let Err(error) = &outcome {
            self.pending_map().replies.remove(&id);
            // Given up on, not failed: the plugin may stop the work.
            if error.starts_with(ABORTED) || error.starts_with(NO_REPLY) {
                let _ = self.notify(CANCEL_REQUEST, json!({ "id": id }));
            }
        }
        outcome.map_err(|error| format!("{method}: {error}"))
    }

    /// Send a notification.
    ///
    /// # Errors
    ///
    /// When it cannot be written.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Answer the other side's request `id`.
    ///
    /// # Errors
    ///
    /// When the answer cannot be written.
    pub fn respond(&self, id: &Value, outcome: Result<Value, RpcError>) -> Result<(), String> {
        let message = match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": error.code, "message": error.message }
            }),
        };
        self.write(&message)
    }

    fn pending_map(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `message` for the writer thread, never waiting for it.
    fn write(&self, message: &Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        match self.outbox.try_send(line) {
            Ok(()) => {
                self.overflowing.store(false, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                if !self.overflowing.swap(true, Ordering::Relaxed) {
                    log::warn!("plugin: not reading its input; dropping messages to it");
                }
                Err("the plugin is not reading its input".to_string())
            }
            Err(TrySendError::Disconnected(_)) => {
                Err("cannot write to the plugin: the connection is closed".to_string())
            }
        }
    }
}

/// Write each queued line in turn, until the peer is dropped or a write
/// fails. A failed write ends the connection: the waiting requests fail,
/// and later sends see the outbox disconnected.
fn write_loop(mut writer: impl Write, queued: &Receiver<String>, pending: &Pending) {
    for line in queued {
        if let Err(error) = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.flush())
        {
            log::debug!("plugin: cannot write to it ({error})");
            pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .close();
            break;
        }
    }
}

fn wait(
    rx: &Receiver<Result<Value, String>>,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled() {
            return Err(ABORTED.to_string());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("{NO_REPLY} {} ms", timeout.as_millis()));
        }
        match rx.recv_timeout(left.min(Duration::from_millis(50))) {
            Ok(reply) => return reply,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the plugin closed the connection".to_string())
            }
        }
    }
}

fn read_loop(reader: impl Read, pending: &Pending, on_incoming: &dyn Fn(Incoming)) {
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                log::debug!("plugin: skipping a line that is not JSON ({error})");
                continue;
            }
        };
        let id = message.get("id").filter(|id| !id.is_null());
        let method = message.get("method").and_then(Value::as_str);
        match (id, method) {
            (Some(id), Some(method)) => on_incoming(Incoming::Request {
                id: id.clone(),
                method: method.to_string(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
            }),
            (None, Some(method)) => on_incoming(Incoming::Notification {
                method: method.to_string(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
            }),
            (Some(id), None) => {
                let reply = match message.get("error") {
                    Some(error) => Err(format!(
                        "{} (code {})",
                        error["message"].as_str().unwrap_or("error"),
                        error["code"]
                    )),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let waiting = id.as_u64().and_then(|id| {
                    pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .replies
                        .remove(&id)
                });
                if let Some(tx) = waiting {
                    let _ = tx.send(reply);
                }
            }
            (None, None) => {}
        }
    }
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::pipe;

    const SECOND: Duration = Duration::from_secs(1);
    fn never() -> bool {
        false
    }

    /// Two peers wired to each other; the second echoes requests back as
    /// results, refusing `fail`.
    fn pair() -> (Peer, Receiver<Incoming>, Receiver<()>) {
        let (a_in, b_out) = pipe().unwrap();
        let (b_in, a_out) = pipe().unwrap();
        let (seen_tx, seen) = mpsc::channel();
        let (closed_tx, closed) = mpsc::channel();
        let a = Peer::new(
            a_in,
            a_out,
            move |incoming| {
                let _ = seen_tx.send(incoming);
            },
            move || {
                let _ = closed_tx.send(());
            },
        );
        std::thread::spawn(move || {
            let b: Arc<Mutex<Option<Arc<Peer>>>> = Arc::new(Mutex::new(None));
            let b_slot = Arc::clone(&b);
            let peer = Arc::new(Peer::new(
                b_in,
                b_out,
                move |incoming| {
                    let peer = b_slot.lock().unwrap().clone().unwrap();
                    if let Incoming::Request { id, method, params } = incoming {
                        let outcome = if method == "fail" {
                            Err(RpcError::new(METHOD_NOT_FOUND, "no such method"))
                        } else {
                            Ok(json!({ "method": method, "params": params }))
                        };
                        peer.respond(&id, outcome).unwrap();
                        if method == "ask-back" {
                            peer.notify("note", json!({ "n": 1 })).unwrap();
                            peer.notify("bye", json!({})).unwrap();
                        }
                    }
                },
                || {},
            ));
            *b.lock().unwrap() = Some(Arc::clone(&peer));
            // Keep the peer alive while the test runs.
            std::thread::sleep(Duration::from_secs(5));
            drop(peer);
        });
        (a, seen, closed)
    }

    #[test]
    fn requests_get_results_and_errors() {
        let (a, _seen, _closed) = pair();
        // Give the other side time to install itself.
        std::thread::sleep(Duration::from_millis(50));
        let result = a
            .request("echo", json!({ "x": 1 }), SECOND, &never)
            .unwrap();
        assert_eq!(result, json!({ "method": "echo", "params": { "x": 1 } }));
        let error = a.request("fail", json!({}), SECOND, &never).unwrap_err();
        assert!(error.contains("no such method"), "{error}");
    }

    #[test]
    fn notifications_from_the_other_side_reach_the_callback() {
        let (a, seen, _closed) = pair();
        std::thread::sleep(Duration::from_millis(50));
        a.request("ask-back", json!({}), SECOND, &never).unwrap();
        assert_eq!(
            seen.recv_timeout(SECOND).unwrap(),
            Incoming::Notification {
                method: "note".into(),
                params: json!({ "n": 1 })
            }
        );
    }

    #[test]
    fn a_closed_connection_fails_waiting_requests_and_reports_once() {
        let (a_in, b_out) = pipe().unwrap();
        let (_b_in, a_out) = pipe().unwrap();
        let (closed_tx, closed) = mpsc::channel();
        let a = Peer::new(a_in, a_out, |_| {}, move || closed_tx.send(()).unwrap());
        let waiter = std::thread::spawn(move || a.request("x", json!({}), SECOND * 5, &never));
        std::thread::sleep(Duration::from_millis(50));
        drop(b_out);
        let error = waiter.join().unwrap().unwrap_err();
        assert!(error.contains("closed"), "{error}");
        closed.recv_timeout(SECOND).unwrap();
    }

    /// A writer that never returns, as a pipe to a plugin that stopped
    /// reading once its buffer is full.
    struct Stuck(Mutex<Receiver<()>>);
    impl Write for Stuck {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.lock().unwrap().recv();
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_peer_that_stops_reading_never_blocks_the_sender() {
        let (a_in, _b_out) = pipe().unwrap();
        let (release, stuck) = mpsc::channel();
        let a = Peer::new(a_in, Stuck(Mutex::new(stuck)), |_| {}, || {});
        let started = Instant::now();
        let mut refused = 0;
        for n in 0..OUTBOX_CAPACITY + 10 {
            if a.notify("event", json!({ "n": n })).is_err() {
                refused += 1;
            }
        }
        // The writer thread holds one message, the outbox the rest.
        assert!(refused >= 9, "only {refused} refused");
        let error = a.request("x", json!({}), SECOND * 5, &never).unwrap_err();
        assert!(error.contains("not reading"), "{error}");
        assert!(started.elapsed() < SECOND, "the sender waited");
        drop(release);
    }

    #[test]
    fn a_request_after_the_connection_closed_fails_at_once() {
        let (a_in, b_out) = pipe().unwrap();
        let (b_in, a_out) = pipe().unwrap();
        let (closed_tx, closed) = mpsc::channel();
        let a = Peer::new(a_in, a_out, |_| {}, move || closed_tx.send(()).unwrap());
        drop((b_out, b_in));
        closed.recv_timeout(SECOND).unwrap();
        let started = Instant::now();
        let error = a.request("x", json!({}), SECOND * 5, &never).unwrap_err();
        assert!(error.contains("closed"), "{error}");
        assert!(started.elapsed() < SECOND, "the request waited");
    }

    #[test]
    fn queued_messages_keep_their_order() {
        let (a_in, _b_out) = pipe().unwrap();
        let (b_in, a_out) = pipe().unwrap();
        let a = Peer::new(a_in, a_out, |_| {}, || {});
        for n in 0..100 {
            a.notify("event", json!({ "n": n })).unwrap();
        }
        let mut lines = BufReader::new(b_in).lines();
        for n in 0..100 {
            let message: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            assert_eq!(message["params"]["n"], n);
        }
    }

    #[test]
    fn a_request_gives_up_when_cancelled_and_says_so() {
        let (a_in, _b_out) = pipe().unwrap();
        let (b_in, a_out) = pipe().unwrap();
        let a = Peer::new(a_in, a_out, |_| {}, || {});
        let error = a.request("x", json!({}), SECOND * 5, &|| true).unwrap_err();
        assert!(error.contains("aborted"), "{error}");
        let mut lines = BufReader::new(b_in).lines();
        let request: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        let cancel: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(cancel["method"], CANCEL_REQUEST);
        assert_eq!(cancel["params"]["id"], request["id"]);
    }
}
