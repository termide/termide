//! Server host-key verification against OpenSSH `known_hosts` files.
//!
//! Trust on first use, as OpenSSH does it: a key recorded for the host
//! connects silently, a different key of the same type recorded for the
//! host refuses the connection, and an unknown host is put to the user,
//! who may record the key in the first `UserKnownHostsFile` or trust it
//! until termide exits; with `StrictHostKeyChecking yes` it is refused.
//!
//! A host is looked up under its `HostKeyAlias`, else its `HostName`
//! (`[name]:port` off port 22), as OpenSSH does.
//!
//! Entries are parsed by `ssh-key`; host matching (wildcards, negation,
//! `[host]:port`, hashed `|1|salt|hash` names) follows OpenSSH's
//! `match_hostname`, which `ssh-key` leaves to the caller. A line that
//! does not parse is skipped rather than failing the whole check, so one
//! unsupported key type cannot lock the user out of every host.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::known_hosts::{Entry, HostPatterns, Marker};
use russh::keys::{Algorithm, HashAlg, PublicKey};
use sha1::Sha1;

use crate::error::{VfsError, VfsResult};
use crate::ssh_config::{match_pattern, SshHostConfig};
use crate::types::HostKey;

/// What the known keys say about a server key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The key is recorded for the host.
    Trusted,
    /// No key of this type is recorded for the host.
    Unknown,
    /// Another key of the same type is recorded for the host, at
    /// `file:line`; `None` when it was trusted earlier in this run.
    Changed { recorded_in: Option<String> },
    /// The key is marked `@revoked` at `file:line`.
    Revoked { location: String },
}

/// The `known_hosts` files to consult, and the one to record keys in.
#[derive(Debug, Clone)]
pub(crate) struct KnownHostsFiles {
    /// Every file read, user files first.
    pub read: Vec<PathBuf>,
    /// Where "accept and remember" appends: the first
    /// `UserKnownHostsFile`; `None` for `none` or without a home directory.
    pub user: Option<PathBuf>,
    /// Record new names hashed (`HashKnownHosts yes`).
    pub hash: bool,
}

impl KnownHostsFiles {
    /// `UserKnownHostsFile`, `GlobalKnownHostsFile` and `HashKnownHosts`
    /// from `config`, with OpenSSH's defaults for what it leaves unset.
    pub fn from_config(config: &SshHostConfig) -> Self {
        let user_files = config.user_known_hosts_files.clone().unwrap_or_else(|| {
            dirs::home_dir()
                .map(|home| {
                    let ssh = home.join(".ssh");
                    vec![ssh.join("known_hosts"), ssh.join("known_hosts2")]
                })
                .unwrap_or_default()
        });
        let global_files = config.global_known_hosts_files.clone().unwrap_or_else(|| {
            if cfg!(unix) {
                vec![
                    PathBuf::from("/etc/ssh/ssh_known_hosts"),
                    PathBuf::from("/etc/ssh/ssh_known_hosts2"),
                ]
            } else {
                Vec::new()
            }
        });
        Self {
            user: user_files.first().cloned(),
            read: user_files.into_iter().chain(global_files).collect(),
            hash: config.hash_known_hosts.unwrap_or(false),
        }
    }
}

/// Keys the user trusted for the rest of this run ("accept once"), keyed
/// by [`host_token`]. Shared by every connection, so a reconnect does not
/// ask again.
#[derive(Debug, Default)]
pub(crate) struct SessionKeys(Mutex<Vec<(String, PublicKey)>>);

impl SessionKeys {
    pub const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn insert(&self, token: String, key: PublicKey) {
        let mut keys = self.0.lock().unwrap_or_else(|e| e.into_inner());
        keys.retain(|(t, k)| !(t == &token && k.algorithm() == key.algorithm()));
        keys.push((token, key));
    }

    fn get(&self, token: &str) -> Vec<PublicKey> {
        let keys = self.0.lock().unwrap_or_else(|e| e.into_inner());
        keys.iter()
            .filter(|(t, _)| t == token)
            .map(|(_, k)| k.clone())
            .collect()
    }
}

/// The process-wide "accept once" store.
pub(crate) static SESSION_KEYS: SessionKeys = SessionKeys::new();

/// The name a host is recorded under: `host` on port 22, `[host]:port`
/// otherwise, lower-cased as OpenSSH does.
pub(crate) fn host_token(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// The name `address:port` is recorded under: `alias` (`HostKeyAlias`)
/// verbatim but lower-cased, else [`host_token`].
pub(crate) fn known_name(alias: Option<&str>, address: &str, port: u16) -> String {
    match alias {
        Some(alias) => alias.to_ascii_lowercase(),
        None => host_token(address, port),
    }
}

/// The user-facing description of `key`, offered on `port` by the host
/// given as `host` and known as `known_as`.
pub(crate) fn describe(host: &str, port: u16, known_as: &str, key: &PublicKey) -> HostKey {
    let mut bare = key.clone();
    bare.set_comment("");
    HostKey {
        host: host.to_string(),
        port,
        known_as: known_as.to_string(),
        algorithm: key.algorithm().as_str().to_string(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        openssh: bare.to_openssh().unwrap_or_default(),
    }
}

/// Check `key` offered by the host known as `token` against `files` and
/// `session`.
pub(crate) fn check(
    files: &KnownHostsFiles,
    session: &SessionKeys,
    token: &str,
    key: &PublicKey,
) -> Verdict {
    let mut trusted = false;
    let mut changed: Option<String> = None;
    for path in &files.read {
        for (line, entry) in matching_entries(path, token) {
            let location = format!("{}:{line}", path.display());
            let same_key = entry.public_key().key_data() == key.key_data();
            match entry.marker() {
                Some(Marker::Revoked) if same_key => return Verdict::Revoked { location },
                Some(_) => {}
                None if same_key => trusted = true,
                None if entry.public_key().algorithm() == key.algorithm() => {
                    changed.get_or_insert(location);
                }
                None => {}
            }
        }
    }
    if trusted {
        return Verdict::Trusted;
    }
    if let Some(location) = changed {
        return Verdict::Changed {
            recorded_in: Some(location),
        };
    }
    let mut verdict = Verdict::Unknown;
    for known in session.get(token) {
        if known.key_data() == key.key_data() {
            return Verdict::Trusted;
        }
        if known.algorithm() == key.algorithm() {
            verdict = Verdict::Changed { recorded_in: None };
        }
    }
    verdict
}

/// Turn a non-trusted verdict on `key` into the error the connection
/// fails with. An unknown key is put to the user unless `strict`
/// (`StrictHostKeyChecking yes`) refuses it outright.
pub(crate) fn verdict_error(verdict: Verdict, key: HostKey, strict: bool) -> VfsError {
    match verdict {
        Verdict::Trusted | Verdict::Unknown if strict => {
            VfsError::HostKeyNotRecorded(Box::new(key))
        }
        Verdict::Trusted | Verdict::Unknown => VfsError::UnknownHostKey(Box::new(key)),
        Verdict::Changed { recorded_in } => VfsError::HostKeyChanged {
            host: key.host,
            port: key.port,
            known_as: key.known_as,
            recorded_in,
        },
        Verdict::Revoked { location } => VfsError::HostKeyRevoked {
            host: key.host,
            port: key.port,
            known_as: key.known_as,
            location,
        },
    }
}

/// Key types recorded for the host, so key exchange can ask for one of
/// them first, as OpenSSH does; otherwise a host known by its ECDSA key
/// would offer Ed25519 and be reported unknown.
pub(crate) fn known_algorithms(
    files: &KnownHostsFiles,
    session: &SessionKeys,
    token: &str,
) -> Vec<Algorithm> {
    let mut algorithms: Vec<Algorithm> = Vec::new();
    let recorded = files
        .read
        .iter()
        .flat_map(|path| matching_entries(path, token))
        .filter(|(_, entry)| entry.marker().is_none())
        .map(|(_, entry)| entry.public_key().algorithm())
        .chain(session.get(token).into_iter().map(|k| k.algorithm()));
    for algorithm in recorded {
        if !algorithms.contains(&algorithm) {
            algorithms.push(algorithm);
        }
    }
    algorithms
}

/// `preferred` with the algorithms of `known` keys moved to the front,
/// keeping the relative order of both groups. RSA keys are recorded as
/// `ssh-rsa` but negotiated as `rsa-sha2-*`, so they match by family.
pub(crate) fn prefer_known(preferred: &[Algorithm], known: &[Algorithm]) -> Vec<Algorithm> {
    let is_known = |a: &Algorithm| {
        known.iter().any(|k| match (a, k) {
            (Algorithm::Rsa { .. }, Algorithm::Rsa { .. }) => true,
            _ => a == k,
        })
    };
    let (mut first, rest): (Vec<_>, Vec<_>) = preferred.iter().cloned().partition(is_known);
    first.extend(rest);
    first
}

/// Record a key the user accepted under its `known_as` name. `remember`
/// appends it to the user's `known_hosts`, hashed when `files.hash`;
/// either way it is trusted until termide exits.
pub(crate) fn trust(
    files: &KnownHostsFiles,
    session: &SessionKeys,
    host_key: &HostKey,
    remember: bool,
) -> VfsResult<()> {
    let key = PublicKey::from_openssh(&host_key.openssh).map_err(|e| VfsError::RemoteError {
        message: format!("invalid host key: {e}"),
    })?;
    let token = host_key.known_as.clone();
    if remember {
        let path = files.user.as_deref().ok_or_else(|| {
            VfsError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no user known_hosts file to record the key in",
            ))
        })?;
        let name = if files.hash {
            hashed_name(&token)?
        } else {
            token.clone()
        };
        append_entry(path, &name, &host_key.openssh)?;
    }
    session.insert(token, key);
    Ok(())
}

/// `token` as `ssh-keygen -H` writes it: `|1|salt|HMAC-SHA1(salt, token)`
/// with a fresh 20-byte salt, both base64.
fn hashed_name(token: &str) -> io::Result<String> {
    let mut salt = vec![0u8; 20];
    getrandom::fill(&mut salt).map_err(|e| io::Error::other(format!("no random salt: {e}")))?;
    let mac = Hmac::<Sha1>::new_from_slice(&salt)
        .map_err(|e| io::Error::other(e.to_string()))?
        .chain_update(token.as_bytes())
        .finalize()
        .into_bytes();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&mac);
    Ok(HostPatterns::HashedName { salt, hash }.to_string())
}

/// Append `token key` to `path` in OpenSSH format, creating the directory
/// (0700) and the file (0600) when missing.
fn append_entry(path: &Path, token: &str, openssh_key: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir)?;
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut line = String::new();
    if file.seek(SeekFrom::End(-1)).is_ok() {
        let mut last = [0u8; 1];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            line.push('\n');
        }
    }
    line.push_str(token);
    line.push(' ');
    line.push_str(openssh_key.trim());
    line.push('\n');
    // One write, so a concurrent `ssh` appending to the same file cannot
    // interleave with half a line.
    file.write_all(line.as_bytes())
}

/// Entries of `path` whose host patterns match `token`, with their
/// 1-based line numbers. A missing or unreadable file has none.
fn matching_entries(path: &Path, token: &str) -> Vec<(usize, Entry)> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            log::warn!("known_hosts: cannot read {}: {e}", path.display());
            return Vec::new();
        }
    };
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // `ssh-key` splits fields on single spaces; OpenSSH allows any
            // run of blanks.
            let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
            match normalized.parse::<Entry>() {
                Ok(entry) if host_matches(entry.host_patterns(), token) => Some((index + 1, entry)),
                Ok(_) => None,
                Err(e) => {
                    log::debug!(
                        "known_hosts: skipping {}:{}: {e}",
                        path.display(),
                        index + 1
                    );
                    None
                }
            }
        })
        .collect()
}

/// OpenSSH's `match_hostname`: a negated pattern that matches excludes the
/// entry outright; otherwise any positive match includes it.
fn host_matches(patterns: &HostPatterns, token: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => {
            let Ok(mac) = Hmac::<Sha1>::new_from_slice(salt) else {
                return false;
            };
            mac.chain_update(token.as_bytes())
                .verify_slice(hash)
                .is_ok()
        }
        HostPatterns::Patterns(patterns) => {
            let mut matched = false;
            for pattern in patterns {
                let pattern = pattern.to_ascii_lowercase();
                if let Some(negated) = pattern.strip_prefix('!') {
                    if match_pattern(negated.as_bytes(), token.as_bytes()) {
                        return false;
                    }
                } else if match_pattern(pattern.as_bytes(), token.as_bytes()) {
                    matched = true;
                }
            }
            matched
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two distinct Ed25519 keys and an ECDSA key.
    const ED_A: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const ED_B: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";
    const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBOFHQfgMFdApoe10JaEdRhWNgKKiX6hch1cSgZ4NKN9xP1d6dZ997uVWBDmCbsQTDx5WViQ7Q4EWrPgvTigKD2M=";

    fn key(s: &str) -> PublicKey {
        PublicKey::from_openssh(s).unwrap()
    }

    fn files_with(content: &str) -> (tempfile::TempDir, KnownHostsFiles) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        fs::write(&path, content).unwrap();
        let files = KnownHostsFiles {
            read: vec![path.clone()],
            user: Some(path),
            hash: false,
        };
        (dir, files)
    }

    fn verdict(content: &str, host: &str, port: u16, offered: &str) -> Verdict {
        let (_dir, files) = files_with(content);
        check(
            &files,
            &SessionKeys::new(),
            &host_token(host, port),
            &key(offered),
        )
    }

    #[test]
    fn recorded_key_is_trusted() {
        let content = format!("# comment\n\nexample.com {ED_A}\n");
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Trusted);
        // Host names compare case-insensitively.
        assert_eq!(verdict(&content, "Example.COM", 22, ED_A), Verdict::Trusted);
    }

    #[test]
    fn tabs_and_runs_of_blanks_separate_fields() {
        let (alg, b64) = ED_A.split_once(' ').unwrap();
        let content = format!("example.com\t{alg}   {b64}  a comment\n");
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Trusted);
    }

    #[test]
    fn unknown_host_and_unknown_key_type_are_unknown() {
        let content = format!("other.org {ED_A}\nexample.com {ECDSA}\n");
        assert_eq!(verdict(&content, "nowhere.net", 22, ED_A), Verdict::Unknown);
        // Only an ECDSA key is recorded: an Ed25519 key is new, not changed.
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Unknown);
    }

    #[test]
    fn different_key_of_the_same_type_is_changed_with_its_line() {
        let content = format!("other.org {ED_B}\nexample.com {ED_B}\n");
        let (_dir, files) = files_with(&content);
        let v = check(
            &files,
            &SessionKeys::new(),
            &host_token("example.com", 22),
            &key(ED_A),
        );
        let expected = format!("{}:2", files.read[0].display());
        assert_eq!(
            v,
            Verdict::Changed {
                recorded_in: Some(expected)
            }
        );
    }

    #[test]
    fn any_matching_line_with_the_key_wins_over_a_stale_one() {
        let content = format!("example.com {ED_B}\nexample.com {ED_A}\n");
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Trusted);
    }

    #[test]
    fn non_default_port_uses_bracketed_name() {
        let content = format!("[example.com]:2222 {ED_A}\n");
        assert_eq!(
            verdict(&content, "example.com", 2222, ED_A),
            Verdict::Trusted
        );
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Unknown);
        let plain = format!("example.com {ED_A}\n");
        assert_eq!(verdict(&plain, "example.com", 2222, ED_A), Verdict::Unknown);
    }

    #[test]
    fn hashed_host_names_match() {
        // `ssh-keygen -H` style entry for "example.com" (from russh's tests).
        let content =
            format!("|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= {ED_B}\n");
        assert_eq!(verdict(&content, "example.com", 22, ED_B), Verdict::Trusted);
        assert_eq!(verdict(&content, "example.org", 22, ED_B), Verdict::Unknown);
    }

    #[test]
    fn wildcards_and_negation() {
        let content = format!("*.example.com,!bad.example.com {ED_A}\n");
        assert_eq!(
            verdict(&content, "a.example.com", 22, ED_A),
            Verdict::Trusted
        );
        assert_eq!(
            verdict(&content, "bad.example.com", 22, ED_A),
            Verdict::Unknown
        );
        let content = format!("host?.lan {ED_A}\n");
        assert_eq!(verdict(&content, "host1.lan", 22, ED_A), Verdict::Trusted);
        assert_eq!(verdict(&content, "host12.lan", 22, ED_A), Verdict::Unknown);
    }

    #[test]
    fn revoked_key_is_refused_and_cert_authority_is_ignored() {
        let content = format!("example.com {ED_A}\n@revoked example.com {ED_A}\n");
        let v = verdict(&content, "example.com", 22, ED_A);
        assert!(matches!(v, Verdict::Revoked { ref location } if location.ends_with(":2")));
        let content = format!("@cert-authority example.com {ED_A}\n");
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Unknown);
    }

    #[test]
    fn unparseable_lines_are_skipped() {
        let content = format!("example.com ssh-ed25519 !!!notbase64\nexample.com {ED_A}\n");
        assert_eq!(verdict(&content, "example.com", 22, ED_A), Verdict::Trusted);
    }

    #[test]
    fn missing_file_means_unknown() {
        let files = KnownHostsFiles {
            read: vec![PathBuf::from("/nonexistent/known_hosts")],
            user: None,
            hash: false,
        };
        let v = check(
            &files,
            &SessionKeys::new(),
            &host_token("h", 22),
            &key(ED_A),
        );
        assert_eq!(v, Verdict::Unknown);
    }

    #[test]
    fn accept_once_trusts_until_exit_and_catches_a_change() {
        let (_dir, files) = files_with("");
        let session = SessionKeys::new();
        let host_key = describe(
            "example.com",
            22,
            &host_token("example.com", 22),
            &key(ED_A),
        );
        trust(&files, &session, &host_key, false).unwrap();
        assert_eq!(fs::read_to_string(&files.read[0]).unwrap(), "");
        assert_eq!(
            check(&files, &session, &host_token("example.com", 22), &key(ED_A)),
            Verdict::Trusted
        );
        assert_eq!(
            check(&files, &session, &host_token("example.com", 22), &key(ED_B)),
            Verdict::Changed { recorded_in: None }
        );
        assert_eq!(
            check(
                &files,
                &session,
                &host_token("example.com", 2222),
                &key(ED_A)
            ),
            Verdict::Unknown
        );
    }

    #[test]
    fn remember_appends_an_openssh_line_after_a_missing_newline() {
        let (_dir, files) = files_with(&format!("other.org {ED_B}"));
        let host_key = describe(
            "Example.com",
            2222,
            &host_token("Example.com", 2222),
            &key(ED_A),
        );
        trust(&files, &SessionKeys::new(), &host_key, true).unwrap();
        let text = fs::read_to_string(&files.read[0]).unwrap();
        assert_eq!(
            text,
            format!("other.org {ED_B}\n[example.com]:2222 {ED_A}\n")
        );
        // A fresh run (empty session) trusts it from the file.
        let v = check(
            &files,
            &SessionKeys::new(),
            &host_token("example.com", 2222),
            &key(ED_A),
        );
        assert_eq!(v, Verdict::Trusted);
    }

    #[cfg(unix)]
    #[test]
    fn remember_creates_ssh_dir_0700_and_file_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        let path = ssh.join("known_hosts");
        let files = KnownHostsFiles {
            read: vec![path.clone()],
            user: Some(path.clone()),
            hash: false,
        };
        let host_key = describe(
            "example.com",
            22,
            &host_token("example.com", 22),
            &key(ED_A),
        );
        trust(&files, &SessionKeys::new(), &host_key, true).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&ssh), 0o700);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("example.com {ED_A}\n")
        );
    }

    #[test]
    fn files_follow_the_config() {
        let config = SshHostConfig {
            user_known_hosts_files: Some(vec!["/u/one".into(), "/u/two".into()]),
            global_known_hosts_files: Some(vec!["/g".into()]),
            hash_known_hosts: Some(true),
            ..Default::default()
        };
        let files = KnownHostsFiles::from_config(&config);
        assert_eq!(files.user, Some(PathBuf::from("/u/one")));
        let read: Vec<PathBuf> = ["/u/one", "/u/two", "/g"].map(PathBuf::from).into();
        assert_eq!(files.read, read);
        assert!(files.hash);

        // `UserKnownHostsFile none`: nothing to record in, and remembering
        // fails instead of writing elsewhere.
        let config = SshHostConfig {
            user_known_hosts_files: Some(vec![]),
            global_known_hosts_files: Some(vec![]),
            ..Default::default()
        };
        let files = KnownHostsFiles::from_config(&config);
        assert!(files.user.is_none() && files.read.is_empty() && !files.hash);
        let host_key = describe("h", 22, "h", &key(ED_A));
        assert!(trust(&files, &SessionKeys::new(), &host_key, true).is_err());

        let defaults = KnownHostsFiles::from_config(&SshHostConfig::default());
        assert!(defaults
            .read
            .iter()
            .any(|p| p.ends_with(".ssh/known_hosts")));
        assert_eq!(defaults.user, defaults.read.first().cloned());
    }

    #[test]
    fn alias_is_used_verbatim_without_the_port() {
        assert_eq!(known_name(Some("Build"), "10.0.0.5", 2222), "build");
        assert_eq!(known_name(None, "10.0.0.5", 2222), "[10.0.0.5]:2222");
        assert_eq!(known_name(None, "Box", 22), "box");
    }

    #[test]
    fn describe_gives_openssh_type_and_sha256_fingerprint() {
        let d = describe(
            "h",
            22,
            &host_token("h", 22),
            &key(&format!("{ED_A} comment")),
        );
        assert_eq!(d.algorithm, "ssh-ed25519");
        assert!(d.fingerprint.starts_with("SHA256:"), "{}", d.fingerprint);
        assert_eq!(d.openssh, ED_A);
    }

    #[test]
    fn known_algorithms_reorder_key_preference() {
        let content = format!("example.com {ECDSA}\n");
        let (_dir, files) = files_with(&content);
        let known = known_algorithms(&files, &SessionKeys::new(), "example.com");
        assert_eq!(known, vec![key(ECDSA).algorithm()]);
        let preferred = russh::Preferred::default().key.to_vec();
        let ordered = prefer_known(&preferred, &known);
        assert_eq!(ordered[0], key(ECDSA).algorithm());
        assert_eq!(ordered.len(), preferred.len());
        // RSA recorded as ssh-rsa lifts every rsa-sha2-* variant.
        let ordered = prefer_known(&preferred, &[Algorithm::Rsa { hash: None }]);
        assert!(ordered[..3]
            .iter()
            .all(|a| matches!(a, Algorithm::Rsa { .. })));
    }
}
