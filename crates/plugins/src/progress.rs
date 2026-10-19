//! Progress of a plugin's tool call while it runs: `tool/progress`
//! `{ "token", "text" }`, the output so far, whole. The call that passed the
//! token in `tool/call` receives it as it comes, on the connection's own
//! thread, with no wait for the UI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};

fn registry() -> &'static Mutex<HashMap<u64, Sender<String>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, Sender<String>>>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

/// A token for one call, and where its progress arrives. Drop it with
/// [`unregister`] when the call ends.
#[must_use]
pub fn register() -> (u64, Receiver<String>) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(token, tx);
    (token, rx)
}

/// The call of `token` ended: what still comes for it is dropped.
pub fn unregister(token: u64) {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&token);
}

/// Hand `text` to the call of `token`, if it still runs.
pub(crate) fn deliver(token: u64, text: String) {
    if let Some(tx) = registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&token)
    {
        let _ = tx.send(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reaches_its_call_until_it_ends() {
        let (token, rx) = register();
        deliver(token, "half".into());
        assert_eq!(rx.try_recv().unwrap(), "half");
        unregister(token);
        deliver(token, "late".into());
        assert!(rx.try_recv().is_err());
    }
}
