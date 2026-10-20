//! Progress of a plugin's tool call while it runs: `tool/progress`
//! `{ "token", "text" }`, the output so far, whole. The call that passed the
//! token in `tool/call` receives it as it comes, on the connection's own
//! thread, with no wait for the UI — from the plugin it called alone: tokens
//! count up and are easy to guess, so each is bound to the connection the
//! call went over, and another plugin's progress for it is dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::rpc::Peer;

/// A connection by its address: the call holds its `Arc` while the token is
/// registered, so the address names that connection alone meanwhile.
type Owner = usize;

fn owner(peer: &Arc<Peer>) -> Owner {
    Arc::as_ptr(peer) as usize
}

/// The calls running, by token: the connection each went over and where
/// its progress goes.
type Registry = Mutex<HashMap<u64, (Owner, Sender<String>)>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

/// A token for one call over `peer`, and where its progress arrives. Drop
/// it with [`unregister`] when the call ends.
#[must_use]
pub fn register(peer: &Arc<Peer>) -> (u64, Receiver<String>) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(token, (owner(peer), tx));
    (token, rx)
}

/// The call of `token` ended: what still comes for it is dropped.
pub fn unregister(token: u64) {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&token);
}

/// Hand `text`, which came over `peer`, to the call of `token`, if it still
/// runs and went over that connection.
pub(crate) fn deliver(peer: &Arc<Peer>, token: u64, text: String) {
    if let Some((from, tx)) = registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&token)
    {
        if *from == owner(peer) {
            let _ = tx.send(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{empty, sink};

    fn peer() -> Arc<Peer> {
        Arc::new(Peer::new(empty(), sink(), |_| {}, || {}))
    }

    #[test]
    fn progress_reaches_its_call_until_it_ends() {
        let plugin = peer();
        let (token, rx) = register(&plugin);
        deliver(&plugin, token, "half".into());
        assert_eq!(rx.try_recv().unwrap(), "half");
        unregister(token);
        deliver(&plugin, token, "late".into());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn another_plugin_cannot_write_into_a_call() {
        let (called, other) = (peer(), peer());
        let (token, rx) = register(&called);
        deliver(&other, token, "forged".into());
        assert!(rx.try_recv().is_err());
        unregister(token);
    }
}
