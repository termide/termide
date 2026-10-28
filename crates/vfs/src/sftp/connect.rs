//! SSH handshake and authentication: runs on the tokio runtime, tries
//! the configured auth methods, and ferries a ready `SftpSession` back
//! to the caller for the actor to own.
//!
//! A host with `ProxyJump` is reached through its jump hosts, each one a
//! full SSH session of its own: its key checked against its own
//! `known_hosts`, authenticated with keys and the agent from its own
//! `ssh_config` entry, and a `direct-tcpip` channel opened from it to the
//! next host, over which that host's session runs. `ProxyCommand` is not
//! run: it would execute an arbitrary command.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::{load_secret_key, PrivateKey, PrivateKeyWithHashAlg};
use russh::ChannelStream;
use russh_sftp::client::SftpSession;

use crate::error::{VfsError, VfsResult};
use crate::ssh_config::{JumpHost, SshHostConfig, Tokens};
use crate::types::{AuthMethod, ConnectOptions, HostKey};

#[cfg(test)]
use super::known_hosts::SessionKeys;
use super::known_hosts::{self, KnownHostsFiles, Verdict, SESSION_KEYS};
use super::DEFAULT_TIMEOUT_SECS;

/// Jump hosts followed at most on the way to one host; a longer chain is
/// taken for a `ProxyJump` loop.
const MAX_JUMPS: usize = 8;

/// Where an SFTP connection goes and how its server key is checked: the
/// host, port and user as given, completed from the SSH configuration.
#[derive(Debug, Clone)]
pub(super) struct Target {
    /// The host as given, which `Host` patterns matched.
    host: String,
    /// What to connect to: `HostName`, else `host`.
    address: String,
    /// The given port, else `Port`, else 22.
    port: u16,
    /// The given user, else `User`, else the local user name.
    username: String,
    /// The name the server key is recorded under in `known_hosts`.
    known_as: String,
    files: KnownHostsFiles,
    /// `StrictHostKeyChecking yes`: refuse an unknown key without asking.
    strict: bool,
    /// The configuration, `%` tokens expanded.
    config: SshHostConfig,
    tokens: Tokens,
}

impl Target {
    /// Complete what the user gave with `config`: a port or user given
    /// explicitly wins over `Port` and `User`, as on the `ssh` command
    /// line.
    pub(super) fn resolve(
        host: &str,
        port: Option<u16>,
        username: Option<&str>,
        mut config: SshHostConfig,
    ) -> Self {
        let address = config.address(host).to_string();
        let port = port.or(config.port).unwrap_or(22);
        let username = username
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or_else(|| config.user.clone())
            .unwrap_or_else(fallback_username);
        let tokens = Tokens {
            original_host: host.to_string(),
            hostname: address.clone(),
            port,
            remote_user: username.clone(),
            local_user: fallback_username(),
            home: dirs::home_dir().map(|h| h.to_string_lossy().into_owned()),
            host_key_alias: config
                .host_key_alias
                .clone()
                .unwrap_or_else(|| host.to_string()),
        };
        config.expand_tokens(&tokens);
        let known_as = known_hosts::known_name(config.host_key_alias.as_deref(), &address, port);
        Self {
            host: host.to_string(),
            address,
            port,
            username,
            known_as,
            files: KnownHostsFiles::from_config(&config),
            strict: config.strict_host_key_checking.unwrap_or(false),
            config,
            tokens,
        }
    }

    /// The connection a host key question was about, resolved again from
    /// the SSH configuration, so its key is recorded where it is read.
    pub(super) fn of_key(key: &HostKey) -> Self {
        let config = SshHostConfig::for_host(&key.host);
        Self::resolve(&key.host, Some(key.port), Some(&key.user), config)
    }

    pub(super) fn known_hosts_files(&self) -> &KnownHostsFiles {
        &self.files
    }
}

/// The hosts to connect to, in order, to reach `host`: its jump hosts,
/// then `host` itself. `lookup` gives the SSH configuration of a host.
///
/// Resolved as `ssh` resolves `ProxyJump a,b`, which runs `ssh -J a b`
/// to reach the target: the last jump host is reached through the ones
/// before it, and only the first is reached through its own `ProxyJump`.
/// Each host is resolved with its own configuration; `user@host:port` in
/// `ProxyJump` wins over that host's `User` and `Port`.
pub(super) fn route(
    host: &str,
    port: Option<u16>,
    username: Option<&str>,
    lookup: &dyn Fn(&str) -> SshHostConfig,
) -> VfsResult<Vec<Target>> {
    let mut hops = Vec::new();
    add_hops(host, port, username, None, lookup, 0, &mut hops)?;
    Ok(hops)
}

/// Push the hosts leading to `host`, then `host`. `jumps`, when given,
/// stands for `host`'s `ProxyJump`, as `ssh -J` does; `depth` counts the
/// jump hosts followed so far.
fn add_hops(
    host: &str,
    port: Option<u16>,
    username: Option<&str>,
    jumps: Option<&[JumpHost]>,
    lookup: &dyn Fn(&str) -> SshHostConfig,
    depth: usize,
    hops: &mut Vec<Target>,
) -> VfsResult<()> {
    let target = Target::resolve(host, port, username, lookup(host));
    let own;
    let jumps = match jumps {
        Some(jumps) => jumps,
        None => {
            if let Some(command) = &target.config.proxy_command {
                if !command.eq_ignore_ascii_case("none") {
                    log::warn!("sftp: ProxyCommand for {host} is not supported; ignored");
                }
            }
            own = target
                .config
                .jump_hosts(&target.tokens)
                .map_err(|e| VfsError::ConnectionFailed(format!("{host}: {e}")))?;
            &own
        }
    };
    if let Some((last, before)) = jumps.split_last() {
        if depth >= MAX_JUMPS {
            return Err(VfsError::ConnectionFailed(format!(
                "ProxyJump: more than {MAX_JUMPS} jump hosts on the way to {host}; \
                 is there a loop?"
            )));
        }
        let before = (!before.is_empty()).then_some(before);
        add_hops(
            &last.host,
            last.port,
            last.user.as_deref(),
            before,
            lookup,
            depth + 1,
            hops,
        )?;
    }
    hops.push(target);
    Ok(())
}

/// Verifies the server key against `known_hosts` before authentication.
///
/// russh calls `check_server_key` during key exchange, before any
/// authentication request, so returning `false` here ends the handshake
/// with nothing sent: a password never reaches an unverified server. The
/// reason is left in `rejection` for `do_connect` to report.
struct VerifyingHandler {
    host: String,
    port: u16,
    user: String,
    known_as: String,
    files: KnownHostsFiles,
    strict: bool,
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
        let verdict = known_hosts::check(&self.files, &SESSION_KEYS, &self.known_as, &key);
        if verdict == Verdict::Trusted {
            return Ok(true);
        }
        let described =
            known_hosts::describe(&self.host, self.port, &self.user, &self.known_as, &key);
        let error = known_hosts::verdict_error(verdict, described, self.strict);
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
/// `host_cfg` supplies `IdentityFile` / `IdentitiesOnly` when
/// `auth == Auto`.
async fn try_authenticate(
    session: &mut client::Handle<VerifyingHandler>,
    host_cfg: &SshHostConfig,
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
            let identities_only = host_cfg.identities_only.unwrap_or(false);

            if !identities_only && try_auth_agent(session, username).await.is_ok() {
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
            if !identities_only {
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

/// Top-level async connect routine. Runs on the global runtime. `port`
/// and `username` are what the user gave, if anything; the SSH
/// configuration for `host` completes them.
pub(super) async fn do_connect(
    host: String,
    port: Option<u16>,
    username: Option<String>,
    options: ConnectOptions,
    cancelled: Arc<AtomicBool>,
) -> VfsResult<(SftpSession, Option<String>)> {
    let route = route(&host, port, username.as_deref(), &SshHostConfig::for_host)?;
    let session = open_route(&route, &options, &cancelled).await?;
    let target = route.last().expect("a route ends with its target");
    open_sftp(session, &target.username).await
}

/// Open an authenticated session to the last host of `route`, through the
/// ones before it. Jump hosts authenticate with keys and the agent only:
/// the password typed for the target never reaches them.
async fn open_route(
    route: &[Target],
    options: &ConnectOptions,
    cancelled: &Arc<AtomicBool>,
) -> VfsResult<client::Handle<VerifyingHandler>> {
    let mut previous: Option<(client::Handle<VerifyingHandler>, &Target)> = None;
    for (i, hop) in route.iter().enumerate() {
        let is_target = i + 1 == route.len();
        let tunnel = match &previous {
            Some((session, jump)) => Some(open_tunnel(session, jump, hop).await?),
            None => None,
        };
        let auth = if is_target {
            &options.auth
        } else {
            &AuthMethod::Auto
        };
        let session = open_verified_session(hop, tunnel, auth, options, cancelled).await;
        let session = if is_target {
            session?
        } else {
            session.map_err(|e| at_jump_host(&hop.host, e))?
        };
        // The jump host's session stays up for as long as the channel to
        // the next host is open, so its handle need not be kept.
        previous = Some((session, hop));
    }
    previous
        .map(|(session, _)| session)
        .ok_or_else(|| VfsError::ConnectionFailed("no host to connect to".into()))
}

/// A `direct-tcpip` channel from `jump` to `next`'s address and port.
async fn open_tunnel(
    session: &client::Handle<VerifyingHandler>,
    jump: &Target,
    next: &Target,
) -> VfsResult<ChannelStream<client::Msg>> {
    let channel = session
        .channel_open_direct_tcpip(next.address.as_str(), u32::from(next.port), "127.0.0.1", 0)
        .await
        .map_err(|e| {
            VfsError::ConnectionFailed(format!(
                "Jump host {}: cannot reach {}:{}: {e}",
                jump.host, next.address, next.port
            ))
        })?;
    Ok(channel.into_stream())
}

/// An error from jump host `host`, named so. Host key errors already name
/// it and keep their type for the host key questions; a refused login
/// stops being `AuthenticationFailed`, which would ask for the target's
/// password.
fn at_jump_host(host: &str, error: VfsError) -> VfsError {
    match error {
        VfsError::AuthenticationFailed(message) | VfsError::ConnectionFailed(message) => {
            VfsError::ConnectionFailed(format!("Jump host {host}: {message}"))
        }
        other => other,
    }
}

/// Connect, directly or over `tunnel`, verify the server key against the
/// target's `known_hosts`, and authenticate with `auth`.
async fn open_verified_session(
    target: &Target,
    tunnel: Option<ChannelStream<client::Msg>>,
    auth: &AuthMethod,
    options: &ConnectOptions,
    cancelled: &Arc<AtomicBool>,
) -> VfsResult<client::Handle<VerifyingHandler>> {
    let (address, port) = (target.address.as_str(), target.port);
    if cancelled.load(Ordering::SeqCst) {
        return Err(VfsError::ConnectionFailed("Connection cancelled".into()));
    }
    let timeout_secs = options.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    // Ask for a key type already recorded for the host first, as OpenSSH
    // does, so a host known by one key type is not reported unknown
    // because the server offered another.
    let known = known_hosts::known_algorithms(&target.files, &SESSION_KEYS, &target.known_as);
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
        host: target.host.clone(),
        port,
        user: target.username.clone(),
        known_as: target.known_as.clone(),
        files: target.files.clone(),
        strict: target.strict,
        rejection: Arc::clone(&rejection),
    };

    let connect_fut = async {
        match tunnel {
            Some(stream) => client::connect_stream(config, stream, handler).await,
            None => client::connect(config, (address, port), handler).await,
        }
    };
    let mut session = tokio::time::timeout(Duration::from_secs(timeout_secs), connect_fut)
        .await
        .map_err(|_| {
            VfsError::ConnectionFailed(format!("Connection to {address}:{port} timed out"))
        })?
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

    try_authenticate(
        &mut session,
        &target.config,
        &target.username,
        auth,
        cancelled,
    )
    .await?;

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
    use crate::sftp::known_hosts::{describe, host_token, trust};

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
            hash: false,
        };
        (dir, files)
    }

    /// `host` resolved through `config`, checked against `files` only.
    fn target_for(host: &str, config: SshHostConfig, files: &KnownHostsFiles) -> Target {
        let mut target = Target::resolve(host, None, Some("u"), config);
        target.files = files.clone();
        target
    }

    fn local(port: u16) -> SshHostConfig {
        SshHostConfig {
            port: Some(port),
            ..Default::default()
        }
    }

    async fn connect_to(target: &Target) -> VfsResult<()> {
        let options = ConnectOptions::with_password("secret").with_timeout(10);
        let cancelled = Arc::new(AtomicBool::new(false));
        open_route(std::slice::from_ref(target), &options, &cancelled)
            .await
            .map(|_| ())
    }

    async fn connect(files: &KnownHostsFiles, port: u16) -> VfsResult<()> {
        connect_to(&target_for("127.0.0.1", local(port), files)).await
    }

    fn described(port: u16) -> crate::types::HostKey {
        let name = host_token("127.0.0.1", port);
        describe("127.0.0.1", port, "u", &name, host_key().public_key())
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
        assert_eq!(*key, described(port));

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
                known_as,
                recorded_in: Some(location),
            } => {
                assert_eq!((host.as_str(), p), ("127.0.0.1", port));
                assert_eq!(known_as, format!("[127.0.0.1]:{port}"));
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
        let key = described(port);
        // The process-wide store, as `trust_host_key(.., false)` uses;
        // the ephemeral port keeps it apart from other tests.
        trust(&files, &SESSION_KEYS, &key, false).unwrap();
        assert_eq!(std::fs::read_to_string(&files.read[0]).unwrap(), "");

        let err = connect(&files, port).await.unwrap_err();
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
        assert!(auths.load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn strict_checking_refuses_an_unknown_key_without_asking() {
        let (port, auths) = serve().await;
        let (_dir, files) = files_with("");
        let config = SshHostConfig {
            strict_host_key_checking: Some(true),
            ..local(port)
        };
        let err = connect_to(&target_for("127.0.0.1", config, &files))
            .await
            .unwrap_err();
        let VfsError::HostKeyNotRecorded(key) = err else {
            panic!("expected a strict refusal, got {err:?}");
        };
        assert_eq!(*key, described(port));
        assert_eq!(auths.load(Ordering::SeqCst), 0);

        // A recorded key still connects under strict checking.
        let (_dir, files) = files_with(&format!("[127.0.0.1]:{port} {}\n", key.openssh));
        let config = SshHostConfig {
            strict_host_key_checking: Some(true),
            ..local(port)
        };
        let err = connect_to(&target_for("127.0.0.1", config, &files))
            .await
            .unwrap_err();
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
    }

    #[tokio::test]
    async fn hostname_port_and_alias_come_from_the_config() {
        let (port, auths) = serve().await;
        let openssh = described(port).openssh;
        // Recorded under the alias only: the address and port reach the
        // server, the key is looked up as `build-alias`.
        let (_dir, files) = files_with(&format!("build-alias {openssh}\n"));
        let config = SshHostConfig {
            hostname: Some("127.0.0.1".into()),
            host_key_alias: Some("Build-Alias".into()),
            ..local(port)
        };
        let target = target_for("build", config, &files);
        assert_eq!(target.known_as, "build-alias");
        let err = connect_to(&target).await.unwrap_err();
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
        assert!(auths.load(Ordering::SeqCst) > 0);

        // Without the alias the key is looked up under the HostName and
        // reported with the host as given.
        let config = SshHostConfig {
            hostname: Some("127.0.0.1".into()),
            ..local(port)
        };
        let err = connect_to(&target_for("build", config, &files))
            .await
            .unwrap_err();
        let VfsError::UnknownHostKey(key) = err else {
            panic!("expected an unknown key, got {err:?}");
        };
        assert_eq!(key.host, "build");
        assert_eq!(key.known_as, format!("[127.0.0.1]:{port}"));
    }

    #[tokio::test]
    async fn a_remembered_key_is_written_hashed_and_found_again() {
        let (port, _auths) = serve().await;
        let (_dir, mut files) = files_with("");
        files.hash = true;
        trust(&files, &SessionKeys::new(), &described(port), true).unwrap();
        let text = std::fs::read_to_string(&files.read[0]).unwrap();
        assert!(text.starts_with("|1|"), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
        let err = connect(&files, port).await.unwrap_err();
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
    }

    #[test]
    fn given_port_and_user_win_over_the_config_and_fill_the_tokens() {
        let config = SshHostConfig {
            hostname: Some("10.0.0.5".into()),
            port: Some(2222),
            user: Some("deploy".into()),
            ..Default::default()
        };
        let t = Target::resolve("build", Some(22), Some("me"), config.clone());
        assert_eq!((t.address.as_str(), t.port), ("10.0.0.5", 22));
        assert_eq!(t.username, "me");
        assert_eq!(t.known_as, "10.0.0.5");

        let t = Target::resolve("build", None, Some(""), config.clone());
        assert_eq!(t.port, 2222);
        assert_eq!(t.username, "deploy");
        assert_eq!(t.known_as, "[10.0.0.5]:2222");

        // Files are named with the port and user in effect.
        let config = SshHostConfig {
            identity_files: vec![PathBuf::from("/k/%r@%h:%p-%n")],
            user_known_hosts_files: Some(vec![PathBuf::from("/kh/%r")]),
            ..config
        };
        let t = Target::resolve("build", Some(22), Some("me"), config);
        assert_eq!(
            t.config.identity_files,
            vec![PathBuf::from("/k/me@10.0.0.5:22-build")]
        );
        assert_eq!(t.files.read.first(), Some(&PathBuf::from("/kh/me")));

        let t = Target::resolve("Plain", None, None, SshHostConfig::default());
        assert_eq!((t.address.as_str(), t.port), ("Plain", 22));
        assert_eq!(t.username, fallback_username());
        assert_eq!(t.known_as, "plain");
        assert!(!t.strict);
    }

    // ---- ProxyJump ----

    /// Host key of the jump servers.
    const JUMP_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCNqLWIC4FAC0KlVFRXkPariBCeGt5OjVowoiOlvRiTcAAAAIhwRUmpcEVJ
qQAAAAtzc2gtZWQyNTUxOQAAACCNqLWIC4FAC0KlVFRXkPariBCeGt5OjVowoiOlvRiTcA
AAAEDx9ht0N/EDfVqpGPgyf6d7lhy0Vy+46CSs1QkNBHfvoI2otYgLgUALQqVUVFeQ9quI
EJ4a3k6NWjCiI6W9GJNwAAAAAAECAwQF
-----END OPENSSH PRIVATE KEY-----
";
    /// The key the client logs in to jump servers with.
    const CLIENT_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBX1D+Pv33nPwZoIyvg26Y7BCuTPTHOYhv+Z4vEQZiUGAAAAIjIMkrDyDJK
wwAAAAtzc2gtZWQyNTUxOQAAACBX1D+Pv33nPwZoIyvg26Y7BCuTPTHOYhv+Z4vEQZiUGA
AAAEBNdlD8VvQh6w8Oj+FvVXzufTfTEU/bDe2ySF6IZ5EPuVfUP4+/fec/BmgjK+DbpjsE
K5M9Mc5iG/5ni8RBmJQYAAAAAAECAwQF
-----END OPENSSH PRIVATE KEY-----
";

    /// What a jump server saw: logins and the tunnels asked for.
    #[derive(Default)]
    struct JumpLog {
        logins: Mutex<Vec<String>>,
        tunnels: Mutex<Vec<(String, u32)>>,
    }

    /// Accepts any public key, refuses passwords, and forwards
    /// `direct-tcpip` channels to the address asked for.
    #[derive(Clone)]
    struct JumpServer(Arc<JumpLog>);

    impl server::Handler for JumpServer {
        type Error = russh::Error;

        async fn auth_publickey(
            &mut self,
            user: &str,
            _key: &russh::keys::PublicKey,
        ) -> Result<Auth, Self::Error> {
            self.0.logins.lock().unwrap().push(user.to_string());
            Ok(Auth::Accept)
        }

        async fn auth_password(&mut self, user: &str, _pw: &str) -> Result<Auth, Self::Error> {
            self.0
                .logins
                .lock()
                .unwrap()
                .push(format!("{user} (password)"));
            Ok(Auth::reject())
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: russh::Channel<server::Msg>,
            host: &str,
            port: u32,
            _originator_address: &str,
            _originator_port: u32,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.0
                .tunnels
                .lock()
                .unwrap()
                .push((host.to_string(), port));
            let Ok(mut tcp) = tokio::net::TcpStream::connect((host, port as u16)).await else {
                return Ok(()); // the dropped reply refuses the channel
            };
            reply.accept().await;
            tokio::spawn(async move {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
            });
            Ok(())
        }
    }

    async fn serve_jump() -> (u16, Arc<JumpLog>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(JumpLog::default());
        let config = Arc::new(server::Config {
            keys: vec![decode_secret_key(JUMP_HOST_KEY, None).unwrap()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let handler = JumpServer(Arc::clone(&log));
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
        (port, log)
    }

    fn jump_public_key() -> String {
        let key = decode_secret_key(JUMP_HOST_KEY, None).unwrap();
        key.public_key().to_openssh().unwrap()
    }

    /// A network of named hosts on 127.0.0.1, all checked against one
    /// `known_hosts` file; jump hosts log in with [`CLIENT_KEY`] only.
    struct Net {
        dir: tempfile::TempDir,
        hosts: Vec<(&'static str, SshHostConfig)>,
    }

    impl Net {
        fn new(known_hosts: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("known_hosts"), known_hosts).unwrap();
            std::fs::write(dir.path().join("client_key"), CLIENT_KEY).unwrap();
            Self {
                dir,
                hosts: Vec::new(),
            }
        }

        fn host(mut self, name: &'static str, port: u16, jump: Option<&str>) -> Self {
            let config = SshHostConfig {
                hostname: Some("127.0.0.1".into()),
                port: Some(port),
                user: Some(format!("{name}-user")),
                proxy_jump: jump.map(String::from),
                identity_files: vec![self.dir.path().join("client_key")],
                identities_only: Some(true),
                user_known_hosts_files: Some(vec![self.dir.path().join("known_hosts")]),
                global_known_hosts_files: Some(Vec::new()),
                ..Default::default()
            };
            self.hosts.push((name, config));
            self
        }

        fn lookup(&self) -> impl Fn(&str) -> SshHostConfig + '_ {
            |name| {
                self.hosts
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_default()
            }
        }

        async fn connect(&self, host: &str) -> VfsResult<()> {
            let route = route(host, None, None, &self.lookup())?;
            let options = ConnectOptions::with_password("secret").with_timeout(10);
            let cancelled = Arc::new(AtomicBool::new(false));
            open_route(&route, &options, &cancelled).await.map(|_| ())
        }
    }

    fn recorded(port: u16, openssh: &str) -> String {
        format!("[127.0.0.1]:{port} {openssh}\n")
    }

    #[tokio::test]
    async fn the_target_is_reached_through_the_jump_host() {
        let (target_port, target_auths) = serve().await;
        let (jump_port, jump) = serve_jump().await;
        let known = recorded(target_port, &described(target_port).openssh)
            + &recorded(jump_port, &jump_public_key());
        let net = Net::new(&known)
            .host("target", target_port, Some("jump"))
            .host("jump", jump_port, None);

        let err = net.connect("target").await.unwrap_err();
        // The password goes to the target, through the tunnel, only.
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
        assert!(target_auths.load(Ordering::SeqCst) > 0);
        assert_eq!(*jump.logins.lock().unwrap(), vec!["jump-user".to_string()]);
        assert_eq!(
            *jump.tunnels.lock().unwrap(),
            vec![("127.0.0.1".to_string(), u32::from(target_port))]
        );
    }

    #[tokio::test]
    async fn an_unknown_jump_host_key_is_asked_about_naming_the_jump_host() {
        let (target_port, target_auths) = serve().await;
        let (jump_port, jump) = serve_jump().await;
        let net = Net::new(&recorded(target_port, &described(target_port).openssh))
            .host("target", target_port, Some("jump"))
            .host("jump", jump_port, None);

        let err = net.connect("target").await.unwrap_err();
        let VfsError::UnknownHostKey(key) = err else {
            panic!("expected an unknown host key, got {err:?}");
        };
        assert_eq!((key.host.as_str(), key.port), ("jump", jump_port));
        assert_eq!(key.user, "jump-user");
        assert_eq!(key.known_as, format!("[127.0.0.1]:{jump_port}"));
        assert_eq!(key.openssh, jump_public_key());
        assert!(jump.logins.lock().unwrap().is_empty(), "nothing sent");
        assert!(jump.tunnels.lock().unwrap().is_empty());
        assert_eq!(target_auths.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_changed_jump_host_key_is_refused() {
        let (target_port, target_auths) = serve().await;
        let (jump_port, jump) = serve_jump().await;
        let known = recorded(target_port, &described(target_port).openssh)
            + &recorded(jump_port, OTHER_KEY);
        let net = Net::new(&known)
            .host("target", target_port, Some("jump"))
            .host("jump", jump_port, None);

        let err = net.connect("target").await.unwrap_err();
        let VfsError::HostKeyChanged { host, port, .. } = err else {
            panic!("expected a changed host key, got {err:?}");
        };
        assert_eq!((host.as_str(), port), ("jump", jump_port));
        assert!(jump.logins.lock().unwrap().is_empty());
        assert_eq!(target_auths.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_unknown_target_key_behind_a_jump_host_names_the_target() {
        let (target_port, target_auths) = serve().await;
        let (jump_port, _jump) = serve_jump().await;
        let net = Net::new(&recorded(jump_port, &jump_public_key()))
            .host("target", target_port, Some("jump"))
            .host("jump", jump_port, None);

        let err = net.connect("target").await.unwrap_err();
        let VfsError::UnknownHostKey(key) = err else {
            panic!("expected an unknown host key, got {err:?}");
        };
        assert_eq!((key.host.as_str(), key.port), ("target", target_port));
        assert_eq!(target_auths.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_refused_jump_login_is_no_password_question() {
        let (target_port, target_auths) = serve().await;
        let (jump_port, jump) = serve_jump().await;
        let mut net = Net::new(&recorded(jump_port, &jump_public_key()))
            .host("target", target_port, Some("jump"))
            .host("jump", jump_port, None);
        // No key to offer: keys and agent only, so the login fails.
        net.hosts[1].1.identity_files.clear();

        let err = net.connect("target").await.unwrap_err();
        let VfsError::ConnectionFailed(message) = err else {
            panic!("expected a connection failure, got {err:?}");
        };
        assert!(message.starts_with("Jump host jump: "), "{message}");
        assert!(jump.logins.lock().unwrap().is_empty(), "no password sent");
        assert_eq!(target_auths.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn two_jump_hosts_are_visited_in_order() {
        let (target_port, target_auths) = serve().await;
        let (first_port, first) = serve_jump().await;
        let (second_port, second) = serve_jump().await;
        let known = recorded(target_port, &described(target_port).openssh)
            + &recorded(first_port, &jump_public_key())
            + &recorded(second_port, &jump_public_key());
        let net = Net::new(&known)
            .host("target", target_port, Some("first,second"))
            .host("first", first_port, None)
            .host("second", second_port, None);

        let err = net.connect("target").await.unwrap_err();
        assert!(matches!(err, VfsError::AuthenticationFailed(_)), "{err:?}");
        assert!(target_auths.load(Ordering::SeqCst) > 0);
        assert_eq!(
            *first.tunnels.lock().unwrap(),
            vec![("127.0.0.1".to_string(), u32::from(second_port))]
        );
        assert_eq!(
            *second.tunnels.lock().unwrap(),
            vec![("127.0.0.1".to_string(), u32::from(target_port))]
        );
        assert_eq!(
            *second.logins.lock().unwrap(),
            vec!["second-user".to_string()]
        );
    }

    fn hop_names(route: &[Target]) -> Vec<String> {
        route
            .iter()
            .map(|t| format!("{}@{}:{}", t.username, t.host, t.port))
            .collect()
    }

    #[test]
    fn route_follows_ssh_dash_j() {
        let net = Net::new("")
            .host("t", 1, Some("a,b"))
            .host("a", 2, Some("z"))
            .host("b", 3, Some("never"))
            .host("z", 4, None);
        let hops = route("t", None, None, &net.lookup()).unwrap();
        // `b` is reached through `a` alone, `a` through its own `z`.
        assert_eq!(
            hop_names(&hops),
            ["z-user@z:4", "a-user@a:2", "b-user@b:3", "t-user@t:1"]
        );

        // `user@host:port` wins over the jump host's `User` and `Port`;
        // tokens are the target's.
        let net = Net::new("")
            .host("t", 1, Some("me@%n-gw:2200"))
            .host("t-gw", 5, None);
        let hops = route("t", Some(9), Some("you"), &net.lookup()).unwrap();
        assert_eq!(hop_names(&hops), ["me@t-gw:2200", "you@t:9"]);

        let net = Net::new("").host("t", 1, Some("none"));
        assert_eq!(route("t", None, None, &net.lookup()).unwrap().len(), 1);
    }

    #[test]
    fn route_rejects_loops_and_bad_values() {
        let net = Net::new("").host("a", 1, Some("b")).host("b", 2, Some("a"));
        let Err(VfsError::ConnectionFailed(message)) = route("a", None, None, &net.lookup()) else {
            panic!("a loop must fail");
        };
        assert!(message.contains("more than 8 jump hosts"), "{message}");

        // Eight jump hosts are fine.
        let names = ["h0", "h1", "h2", "h3", "h4", "h5", "h6", "h7", "h8"];
        let mut net = Net::new("");
        for (i, name) in names.iter().enumerate() {
            net = net.host(name, 1, names.get(i + 1).copied());
        }
        assert_eq!(route("h0", None, None, &net.lookup()).unwrap().len(), 9);

        let net = Net::new("").host("t", 1, Some("gw:x"));
        assert!(matches!(
            route("t", None, None, &net.lookup()),
            Err(VfsError::ConnectionFailed(_))
        ));
    }
}
