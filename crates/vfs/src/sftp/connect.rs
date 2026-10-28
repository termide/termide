//! SSH handshake and authentication: runs on the tokio runtime, tries
//! the configured auth methods, and ferries a ready `SftpSession` back
//! to the caller for the actor to own.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::{load_secret_key, PrivateKey, PrivateKeyWithHashAlg};
use russh_sftp::client::SftpSession;

use crate::error::{VfsError, VfsResult};
use crate::types::{AuthMethod, ConnectOptions};

#[cfg(test)]
use super::known_hosts::SessionKeys;
use super::known_hosts::{self, KnownHostsFiles, Verdict, SESSION_KEYS};
use super::DEFAULT_TIMEOUT_SECS;

/// Verifies the server key against `known_hosts` before authentication.
///
/// russh calls `check_server_key` during key exchange, before any
/// authentication request, so returning `false` here ends the handshake
/// with nothing sent: a password never reaches an unverified server. The
/// reason is left in `rejection` for `do_connect` to report.
struct VerifyingHandler {
    host: String,
    port: u16,
    files: KnownHostsFiles,
    rejection: Arc<Mutex<Option<VfsError>>>,
}

impl client::Handler for VerifyingHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        use russh::keys::PublicKeyOrCertificate;
        // Certificates are never advertised (`host_key_certificates` stays
        // empty), so a server presents a bare key; should one arrive, its
        // key is checked like a bare one.
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => {
                russh::keys::PublicKey::from(cert.public_key().clone())
            }
        };
        let verdict = known_hosts::check(&self.files, &SESSION_KEYS, &self.host, self.port, &key);
        if verdict == Verdict::Trusted {
            return Ok(true);
        }
        let error = known_hosts::verdict_error(verdict, &self.host, self.port, &key);
        log::warn!("sftp: server key not accepted: {error}");
        if let Ok(mut slot) = self.rejection.lock() {
            *slot = Some(error);
        }
        Ok(false)
    }
}

/// Locate default SSH private key files for current user, in priority
/// order. Mirrors what the previous Auto auth would attempt.
fn default_key_files() -> Vec<PathBuf> {
    let mut keys = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for name in ["id_ed25519", "id_rsa", "id_ecdsa", "id_dsa"] {
            let p = home.join(".ssh").join(name);
            if p.exists() {
                keys.push(p);
            }
        }
    }
    keys
}

/// Fallback username when none is provided: $USER → $USERNAME → "root".
pub(super) fn fallback_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_string())
}

/// Expand a leading `~` in a path using `$HOME`. Returns the input
/// unchanged if there's no tilde or no home directory.
fn expand_tilde(p: &Path) -> PathBuf {
    let s = match p.to_str() {
        Some(s) => s,
        None => return p.to_path_buf(),
    };
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    p.to_path_buf()
}

/// Try every reasonable auth method until one succeeds. Stops at the
/// first success; returns AuthenticationFailed if none work.
///
/// `host` is used to look up Host-specific entries in `~/.ssh/config`
/// (IdentityFile / IdentitiesOnly) when `auth == Auto`.
async fn try_authenticate(
    session: &mut client::Handle<VerifyingHandler>,
    host: &str,
    username: &str,
    auth: &AuthMethod,
    cancelled: &Arc<AtomicBool>,
) -> VfsResult<()> {
    let cancelled_check = || -> VfsResult<()> {
        if cancelled.load(Ordering::SeqCst) {
            Err(VfsError::ConnectionFailed("Connection cancelled".into()))
        } else {
            Ok(())
        }
    };

    cancelled_check()?;

    match auth {
        AuthMethod::None => {
            // Most permissive interpretation of "None": try SSH agent.
            try_auth_agent(session, username).await
        }
        AuthMethod::Password(password) => try_auth_password(session, username, password).await,
        AuthMethod::SshKey {
            private_key,
            passphrase,
        } => try_auth_keyfile(session, username, private_key, passphrase.as_deref()).await,
        AuthMethod::SshAgent => try_auth_agent(session, username).await,
        AuthMethod::Auto => {
            // Phase 2b: honor ~/.ssh/config IdentityFile / IdentitiesOnly.
            // Priority order matches OpenSSH's default behavior:
            //   1. SSH agent (unless IdentitiesOnly=yes)
            //   2. Keys from IdentityFile entries in ssh_config
            //   3. Default keys in ~/.ssh/id_*
            let host_cfg = crate::ssh_config::SshConfig::from_default_path()
                .map(|cfg| cfg.get_host_config(host))
                .unwrap_or_default();

            if !host_cfg.identities_only && try_auth_agent(session, username).await.is_ok() {
                return Ok(());
            }
            cancelled_check()?;

            // Try keys from ssh_config first (they're authoritative).
            for raw_key in &host_cfg.identity_files {
                let key_path = expand_tilde(raw_key);
                if !key_path.exists() {
                    continue;
                }
                if try_auth_keyfile(session, username, &key_path, None)
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
                cancelled_check()?;
            }

            // Fallback to default keys unless IdentitiesOnly forbids it.
            if !host_cfg.identities_only {
                for key_path in default_key_files() {
                    if try_auth_keyfile(session, username, &key_path, None)
                        .await
                        .is_ok()
                    {
                        return Ok(());
                    }
                    cancelled_check()?;
                }
            }

            Err(VfsError::AuthenticationFailed(
                "No authentication method succeeded (tried SSH agent and SSH keys). \
                 Provide a password or specific key."
                    .into(),
            ))
        }
    }
}

async fn try_auth_password(
    session: &mut client::Handle<VerifyingHandler>,
    username: &str,
    password: &str,
) -> VfsResult<()> {
    let res = session
        .authenticate_password(username, password)
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("password auth error: {e}")))?;
    if res.success() {
        Ok(())
    } else {
        Err(VfsError::AuthenticationFailed("Password rejected".into()))
    }
}

async fn try_auth_keyfile(
    session: &mut client::Handle<VerifyingHandler>,
    username: &str,
    key_path: &Path,
    passphrase: Option<&str>,
) -> VfsResult<()> {
    let key = load_secret_key(key_path, passphrase).map_err(|e| {
        VfsError::AuthenticationFailed(format!(
            "Failed to load private key '{}': {}",
            key_path.display(),
            e
        ))
    })?;
    authenticate_with_key(session, username, key).await
}

async fn authenticate_with_key(
    session: &mut client::Handle<VerifyingHandler>,
    username: &str,
    key: PrivateKey,
) -> VfsResult<()> {
    let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key), None);
    let res = session
        .authenticate_publickey(username, key_with_alg)
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("publickey auth error: {e}")))?;
    if res.success() {
        Ok(())
    } else {
        Err(VfsError::AuthenticationFailed("Public key rejected".into()))
    }
}

#[cfg(unix)]
async fn try_auth_agent(
    session: &mut client::Handle<VerifyingHandler>,
    username: &str,
) -> VfsResult<()> {
    use russh::keys::agent::client::AgentClient;
    if std::env::var_os("SSH_AUTH_SOCK").is_none() {
        return Err(VfsError::AuthenticationFailed(
            "SSH agent not available (SSH_AUTH_SOCK unset)".into(),
        ));
    }
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("agent connect failed: {e}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| VfsError::AuthenticationFailed(format!("agent identities failed: {e}")))?;
    for identity in identities {
        let pk = identity.public_key().into_owned();
        let res = session
            .authenticate_publickey_with(username, pk, None, &mut agent)
            .await;
        if let Ok(auth_result) = res {
            if auth_result.success() {
                return Ok(());
            }
        }
    }
    Err(VfsError::AuthenticationFailed(
        "SSH agent had no usable keys".into(),
    ))
}

#[cfg(not(unix))]
async fn try_auth_agent(
    _session: &mut client::Handle<VerifyingHandler>,
    _username: &str,
) -> VfsResult<()> {
    Err(VfsError::AuthenticationFailed(
        "SSH agent not supported on this platform".into(),
    ))
}

/// Top-level async connect routine. Runs on the global runtime.
pub(super) async fn do_connect(
    host: String,
    port: u16,
    username: String,
    options: ConnectOptions,
    cancelled: Arc<AtomicBool>,
) -> VfsResult<(SftpSession, Option<String>)> {
    let files = KnownHostsFiles::openssh_defaults();
    let session =
        open_verified_session(files, &host, port, &username, &options, &cancelled).await?;
    open_sftp(session, &username).await
}

/// Connect, verify the server key against `files`, and authenticate.
async fn open_verified_session(
    files: KnownHostsFiles,
    host: &str,
    port: u16,
    username: &str,
    options: &ConnectOptions,
    cancelled: &Arc<AtomicBool>,
) -> VfsResult<client::Handle<VerifyingHandler>> {
    let host = host.to_string();
    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }
    let timeout_secs = options.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    // Ask for a key type already recorded for the host first, as OpenSSH
    // does, so a host known by one key type is not reported unknown
    // because the server offered another.
    let known = known_hosts::known_algorithms(&files, &SESSION_KEYS, &host, port);
    let defaults = russh::Preferred::default();
    let preferred = russh::Preferred {
        key: Cow::Owned(known_hosts::prefer_known(&defaults.key, &known)),
        ..defaults
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(timeout_secs * 5)),
        preferred,
        ..Default::default()
    });

    let rejection = Arc::new(Mutex::new(None));
    let handler = VerifyingHandler {
        host: host.clone(),
        port,
        files,
        rejection: Arc::clone(&rejection),
    };
    let addr = (host.as_str(), port);

    let connect_fut = client::connect(config, addr, handler);
    let mut session = tokio::time::timeout(Duration::from_secs(timeout_secs), connect_fut)
        .await
        .map_err(|_| VfsError::ConnectionFailed(format!("Connection to {host}:{port} timed out")))?
        .map_err(|e| {
            // A refused server key ends the handshake with a generic
            // russh error; report why it was refused instead.
            rejection
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .unwrap_or_else(|| VfsError::ConnectionFailed(format!("SSH connect failed: {e}")))
        })?;

    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }

    try_authenticate(&mut session, &host, username, &options.auth, cancelled).await?;

    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }
    Ok(session)
}

/// Open the SFTP subsystem on an authenticated session.
async fn open_sftp(
    session: client::Handle<VerifyingHandler>,
    username: &str,
) -> VfsResult<(SftpSession, Option<String>)> {
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| VfsError::ConnectionFailed(format!("SSH channel open failed: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| VfsError::ConnectionFailed(format!("SFTP subsystem request failed: {e}")))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| VfsError::ConnectionFailed(format!("SFTP session init failed: {e}")))?;

    let home_dir = sftp
        .canonicalize(".")
        .await
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(format!("/home/{username}")));

    Ok((sftp, home_dir))
}

#[cfg(test)]
mod tests {
    //! The handshake against an in-process russh server: the server key is
    //! checked before any authentication request reaches the server.

    use std::sync::atomic::AtomicUsize;

    use russh::keys::{decode_secret_key, PrivateKey};
    use russh::server::{self, Auth};
    use tokio::net::TcpListener;

    use super::*;
    use crate::sftp::known_hosts::{describe, trust};

    const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDO45WFIYYTp73JnbetwaSgEHtcB26w0+54jW3pgcB9CwAAAIhmXsWBZl7F
gQAAAAtzc2gtZWQyNTUxOQAAACDO45WFIYYTp73JnbetwaSgEHtcB26w0+54jW3pgcB9Cw
AAAED8wGvmq/SEg/v3vgSA0ag+4XfizpvzOO6zrkdngXK4W87jlYUhhhOnvcmdt63BpKAQ
e1wHbrDT7niNbemBwH0LAAAAAAECAwQF
-----END OPENSSH PRIVATE KEY-----
";
    /// Another Ed25519 key, recorded for the host to simulate a change.
    const OTHER_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";

    /// Counts every authentication request and rejects it.
    #[derive(Clone)]
    struct CountingServer(Arc<AtomicUsize>);

    impl server::Handler for CountingServer {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Auth::reject())
        }

        async fn auth_password(&mut self, _user: &str, _pw: &str) -> Result<Auth, Self::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Auth::reject())
        }
    }

    fn host_key() -> PrivateKey {
        decode_secret_key(HOST_KEY, None).unwrap()
    }

    /// Serve SSH on an ephemeral port; returns the port and the count of
    /// authentication requests received.
    async fn serve() -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let auths = Arc::new(AtomicUsize::new(0));
        let config = Arc::new(server::Config {
            keys: vec![host_key()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let handler = CountingServer(Arc::clone(&auths));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let config = Arc::clone(&config);
                let handler = handler.clone();
                tokio::spawn(async move {
                    if let Ok(session) = server::run_stream(config, stream, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });
        (port, auths)
    }

    fn files_with(content: &str) -> (tempfile::TempDir, KnownHostsFiles) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, content).unwrap();
        let files = KnownHostsFiles {
            read: vec![path.clone()],
            user: Some(path),
        };
        (dir, files)
    }

    async fn connect(files: &KnownHostsFiles, port: u16) -> VfsResult<()> {
        let options = ConnectOptions::with_password("secret").with_timeout(10);
        let cancelled = Arc::new(AtomicBool::new(false));
        open_verified_session(files.clone(), "127.0.0.1", port, "u", &options, &cancelled)
            .await
            .map(|_| ())
    }

    #[tokio::test]
    async fn unknown_key_stops_before_the_password_and_remembering_lets_it_through() {
        let (port, auths) = serve().await;
        let (_dir, files) = files_with("");

        let err = connect(&files, port).await.unwrap_err();
        let VfsError::UnknownHostKey(key) = err else {
            panic!("expected an unknown host key, got {err:?}");
        };
        assert_eq!(auths.load(Ordering::SeqCst), 0, "nothing sent before trust");
        let expected = describe("127.0.0.1", port, host_key().public_key());
        assert_eq!(*key, expected);

        // Remembered in this test's known_hosts; the session store is
        // process-wide, so a fresh one proves the file alone suffices.
        trust(&files, &SessionKeys::new(), &key, true).unwrap();
        let err = connect(&files, port).await.unwrap_err();
        assert!(
            matches!(err, VfsError::AuthenticationFailed(_)),
            "the handshake passes and the password is tried: {err:?}"
        );
        assert!(auths.load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn changed_key_is_refused_without_authenticating() {
        let (port, auths) = serve().await;
        let (_dir, files) = files_with(&format!("[127.0.0.1]:{port} {OTHER_KEY}\n"));

        let err = connect(&files, port).await.unwrap_err();
        match err {
            VfsError::HostKeyChanged {
                host,
                port: p,
                recorded_in: Some(location),
            } => {
                assert_eq!((host.as_str(), p), ("127.0.0.1", port));
                assert!(location.ends_with("known_hosts:1"), "{location}");
            }
            other => panic!("expected a changed host key, got {other:?}"),
        }
        assert_eq!(auths.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn accept_once_trusts_the_key_for_the_rest_of_the_run() {
        let (port, auths) = serve().await;
        let (_dir, files) = files_with("");
        let key = describe("127.0.0.1", port, host_key().public_key());
        // The process-wide store, as `trust_host_key(.., false)` uses;
        // the ephemeral port keeps it apart from other tests.
        trust(&files, &SESSION_KEYS, &key, false).unwrap();
        assert_eq!(std::fs::read_to_string(&files.read[0]).unwrap(), "");

        let err = connect(&files, port).await.unwrap_err();
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
        assert!(auths.load(Ordering::SeqCst) > 0);
    }
}
