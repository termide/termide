//! API keys of agent connections: from the environment, or from the
//! password vault.
//!
//! A connection's key is the value of the variable its `api_key_env` names
//! when that is set; otherwise the vault entry kept under the connection's
//! name. Model calls run on the agents' threads while the vault lives on the
//! UI thread, so a call that needs a key from the vault sends the
//! connection's name down a process-wide channel and waits, watching its
//! cancel token; the app answers on its tick — straight from the vault when
//! it is unlocked, after the master password prompt when it is not.
//!
//! A key the app handed out is kept in memory for the rest of the process,
//! as the session's SSH passphrase is: locking the vault later does not take
//! it from the connections already using it. Storing, replacing, removing
//! or moving a key through the settings updates what is kept.
//!
//! No key ever goes into a log line or an error message.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use termide_agent_core::{
    AssistantMessage, CancelToken, Failure, FailureKind, ModelInfo, Provider, Request, StopReason,
    StreamEvent, ThinkingLevel,
};
use termide_config::Connection;
use termide_core::SecretText;
use termide_secrets::{SecretKind, Vault};

/// How often a call waiting for its key looks at its cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// The app's answer to an [`Ask`]: the key; `Ok(None)` when the vault holds
/// none after all; `Err(())` when it holds one the user did not unlock.
pub(crate) type Answer = Result<Option<SecretText>, ()>;

/// A model call's request for the key of `connection`.
pub(crate) struct Ask {
    pub connection: String,
    /// The call's token: a stopped call no longer needs an answer.
    pub cancel: CancelToken,
    pub reply: Sender<Answer>,
}

/// Why a call has no key to go with.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// The vault holds the key, and the user did not unlock it.
    Locked(String),
    /// The call was stopped while it waited.
    Stopped,
}

/// The keys handed out and the asks for more.
pub(crate) struct Broker {
    asks: Sender<Ask>,
    pending: Mutex<Receiver<Ask>>,
    /// Set once an app answers the asks; a headless run has none.
    served: AtomicBool,
    keys: Mutex<HashMap<String, SecretText>>,
    vault_path: Option<PathBuf>,
}

impl Broker {
    pub(crate) fn new(vault_path: Option<PathBuf>) -> Self {
        let (asks, pending) = mpsc::channel();
        Self {
            asks,
            pending: Mutex::new(pending),
            served: AtomicBool::new(false),
            keys: Mutex::new(HashMap::new()),
            vault_path,
        }
    }

    /// The process's broker, over the vault at its default place.
    pub(crate) fn global() -> &'static Broker {
        static GLOBAL: OnceLock<Broker> = OnceLock::new();
        GLOBAL.get_or_init(|| Broker::new(Vault::default_path()))
    }

    fn keys(&self) -> MutexGuard<'_, HashMap<String, SecretText>> {
        self.keys.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// From now on an app answers the asks: calls wait for it rather than
    /// fail as in a run without the UI.
    pub(crate) fn attach(&self) {
        self.served.store(true, Ordering::Release);
    }

    /// The asks that came in since the last call; from now on the asks are
    /// answered.
    pub(crate) fn take_asks(&self) -> Vec<Ask> {
        self.attach();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .collect()
    }

    /// Keep `key` as the key of `connection`.
    pub(crate) fn remember(&self, connection: &str, key: SecretText) {
        self.keys().insert(connection.to_string(), key);
    }

    /// Drop the key kept for `connection`.
    pub(crate) fn forget(&self, connection: &str) {
        self.keys().remove(connection);
    }

    /// Keep the key of connection `from` as that of `to`.
    pub(crate) fn rename(&self, from: &str, to: &str) {
        let mut keys = self.keys();
        if let Some(key) = keys.remove(from) {
            keys.insert(to.to_string(), key);
        }
    }

    /// The key handed out for `connection`, if any.
    pub(crate) fn remembered(&self, connection: &str) -> Option<SecretText> {
        self.keys().get(connection).cloned()
    }

    /// The key of connection `name` that needs nobody to ask: its variable's
    /// value, else the one handed out before.
    pub(crate) fn known(&self, name: &str, connection: &Connection) -> Option<SecretText> {
        connection
            .api_key_env_name()
            .and_then(|var| std::env::var(var).ok())
            .filter(|key| !key.is_empty())
            .map(SecretText::new)
            .or_else(|| self.remembered(name))
    }

    /// Whether the vault keeps a key for `connection`. Reading the entry's
    /// metadata needs no master password.
    fn stored(&self, connection: &str) -> bool {
        let Some(path) = &self.vault_path else {
            return false;
        };
        match Vault::open(path) {
            Ok(vault) => {
                vault.is_some_and(|v| v.find(SecretKind::ApiKey, connection, None).is_some())
            }
            Err(e) => {
                log::warn!("password vault unreadable: {e}");
                false
            }
        }
    }

    /// The key a call on connection `name` goes with: [`Broker::known`],
    /// else the vault's, asked of the app — which may ask the user for the
    /// master password, so this blocks until the answer or `cancel`.
    /// `Ok(None)` when there is no key anywhere: a local server needs none.
    pub(crate) fn resolve(
        &self,
        name: &str,
        connection: &Connection,
        cancel: &CancelToken,
    ) -> Result<Option<SecretText>, Unavailable> {
        if let Some(key) = self.known(name, connection) {
            return Ok(Some(key));
        }
        if !self.stored(name) {
            return Ok(None);
        }
        if !self.served.load(Ordering::Acquire) {
            return Err(Unavailable::Locked(format!(
                "the API key of connection `{name}` is in the password vault, which a run \
                 without the UI cannot unlock; name a variable holding it in api_key_env"
            )));
        }
        let (reply, answer) = mpsc::channel();
        let ask = Ask {
            connection: name.to_string(),
            cancel: cancel.clone(),
            reply,
        };
        if self.asks.send(ask).is_err() {
            return Err(Unavailable::Locked(locked_message(name)));
        }
        loop {
            match answer.recv_timeout(CANCEL_POLL) {
                Ok(Ok(key)) => return Ok(key),
                Ok(Err(())) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(Unavailable::Locked(locked_message(name)))
                }
                Err(RecvTimeoutError::Timeout) if cancel.is_cancelled() => {
                    return Err(Unavailable::Stopped)
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

fn locked_message(name: &str) -> String {
    format!(
        "the API key of connection `{name}` is in the password vault, which stayed locked; \
         send the request again to be asked for the master password"
    )
}

/// What builds a connection's provider with a key.
pub(crate) type BuildProvider = fn(&Connection, Option<String>) -> Arc<dyn Provider>;

/// The provider of a model connection, built again with the key each call
/// resolves to, so a key stored, unlocked or replaced after the session
/// began is the one its next call goes with.
pub(crate) struct KeyedProvider {
    name: String,
    connection_name: String,
    connection: Connection,
    build: BuildProvider,
    broker: &'static Broker,
    built: Mutex<Built>,
}

struct Built {
    key: Option<SecretText>,
    provider: Arc<dyn Provider>,
}

impl KeyedProvider {
    pub(crate) fn new(
        connection_name: &str,
        connection: &Connection,
        build: BuildProvider,
        broker: &'static Broker,
    ) -> Self {
        let key = broker.known(connection_name, connection);
        let provider = build(connection, key.as_ref().map(|k| k.expose().to_string()));
        Self {
            name: provider.name().to_string(),
            connection_name: connection_name.to_string(),
            connection: connection.clone(),
            build,
            broker,
            built: Mutex::new(Built { key, provider }),
        }
    }

    fn built(&self) -> MutexGuard<'_, Built> {
        self.built.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The provider as last built.
    fn current(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.built().provider)
    }

    /// The provider for `key`, built again when the key changed.
    fn with_key(&self, key: Option<SecretText>) -> Arc<dyn Provider> {
        let mut built = self.built();
        let same =
            built.key.as_ref().map(SecretText::expose) == key.as_ref().map(SecretText::expose);
        if !same {
            built.provider = (self.build)(
                &self.connection,
                key.as_ref().map(|k| k.expose().to_string()),
            );
            built.key = key;
        }
        Arc::clone(&built.provider)
    }

    fn resolve(&self, cancel: &CancelToken) -> Result<Arc<dyn Provider>, Unavailable> {
        self.broker
            .resolve(&self.connection_name, &self.connection, cancel)
            .map(|key| self.with_key(key))
    }
}

impl Provider for KeyedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        match self.resolve(cancel) {
            Ok(provider) => provider.stream(request, on_event, cancel),
            Err(Unavailable::Stopped) => AssistantMessage::failed(
                &self.name,
                request.model.id.clone(),
                StopReason::Aborted,
                "stopped while waiting for the API key",
            ),
            // Not a failure that passes by waiting: no retries, no prompt
            // again until the user sends again.
            Err(Unavailable::Locked(message)) => AssistantMessage::failed(
                &self.name,
                request.model.id.clone(),
                StopReason::Error,
                message,
            )
            .with_failure(Failure {
                kind: FailureKind::Auth,
                resets_at: None,
            }),
        }
    }

    fn endpoint(&self) -> Option<String> {
        self.current().endpoint()
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        match self.resolve(&CancelToken::new()) {
            Ok(provider) => provider.list_models(),
            Err(Unavailable::Locked(message)) => Err(message),
            Err(Unavailable::Stopped) => Err("stopped while waiting for the API key".into()),
        }
    }

    fn thinking_levels(&self, model: &str) -> Vec<ThinkingLevel> {
        self.current().thinking_levels(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_secrets::KdfParams;

    const FAST: KdfParams = KdfParams {
        m_cost: 64,
        t_cost: 1,
        p_cost: 1,
    };

    fn leak(broker: Broker) -> &'static Broker {
        Box::leak(Box::new(broker))
    }

    /// A broker over a vault holding the key `secret` for connection `c`.
    fn broker_with_stored_key() -> (tempfile::TempDir, &'static Broker) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.toml");
        let mut vault = Vault::create(&path, "m", FAST).unwrap();
        vault
            .put(SecretKind::ApiKey, "c", None, "secret", None)
            .unwrap();
        (dir, leak(Broker::new(Some(path))))
    }

    fn connection(api_key_env: &str) -> Connection {
        Connection {
            api_key_env: api_key_env.to_string(),
            ..Connection::default()
        }
    }

    #[test]
    fn no_key_anywhere_is_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let broker = Broker::new(Some(dir.path().join("vault.toml")));
        let cancel = CancelToken::new();
        assert_eq!(
            broker
                .resolve("c", &connection(""), &cancel)
                .unwrap()
                .map(|k| k.expose().to_string()),
            None
        );
    }

    #[test]
    fn a_pasted_key_is_not_taken_for_a_variable_name() {
        let broker = Broker::new(None);
        broker.remember("c", SecretText::new("kept"));
        let key = broker.known("c", &connection("sk-or-v1-abc")).unwrap();
        assert_eq!(key.expose(), "kept");
    }

    #[test]
    fn without_an_app_a_stored_key_is_unavailable() {
        let (_dir, broker) = broker_with_stored_key();
        let result = broker.resolve("c", &connection(""), &CancelToken::new());
        let Err(Unavailable::Locked(message)) = result else {
            panic!("expected the locked error");
        };
        assert!(message.contains("`c`") && !message.contains("secret"));
    }

    #[test]
    fn the_app_answers_an_ask_and_the_key_is_kept() {
        let (_dir, broker) = broker_with_stored_key();
        assert!(broker.take_asks().is_empty());
        let waiter = std::thread::spawn(move || {
            broker
                .resolve("c", &connection(""), &CancelToken::new())
                .map(|key| key.map(|k| k.expose().to_string()))
        });
        let ask = loop {
            if let Some(ask) = broker.take_asks().pop() {
                break ask;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(ask.connection, "c");
        broker.remember("c", SecretText::new("secret"));
        ask.reply.send(Ok(Some(SecretText::new("secret")))).unwrap();
        assert_eq!(waiter.join().unwrap(), Ok(Some("secret".to_string())));
        // The next call needs nobody.
        assert_eq!(
            broker
                .resolve("c", &connection(""), &CancelToken::new())
                .unwrap()
                .unwrap()
                .expose(),
            "secret"
        );
    }

    #[test]
    fn a_declined_unlock_and_a_stopped_call_end_the_wait() {
        let (_dir, broker) = broker_with_stored_key();
        broker.take_asks();
        let waiter = std::thread::spawn(move || {
            broker
                .resolve("c", &connection(""), &CancelToken::new())
                .map(|_| ())
        });
        let ask = loop {
            if let Some(ask) = broker.take_asks().pop() {
                break ask;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        ask.reply.send(Err(())).unwrap();
        assert!(matches!(
            waiter.join().unwrap(),
            Err(Unavailable::Locked(_))
        ));

        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            broker.resolve("c", &connection(""), &cancel).map(|_| ()),
            Err(Unavailable::Stopped)
        );
    }

    #[test]
    fn rename_and_forget_move_the_kept_key() {
        let broker = Broker::new(None);
        broker.remember("a", SecretText::new("k"));
        broker.rename("a", "b");
        assert!(broker.remembered("a").is_none());
        assert_eq!(broker.remembered("b").unwrap().expose(), "k");
        broker.forget("b");
        assert!(broker.remembered("b").is_none());
    }
}
