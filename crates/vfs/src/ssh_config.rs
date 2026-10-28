//! OpenSSH client configuration for one host: the options an SFTP
//! connection takes from `~/.ssh/config` and `/etc/ssh/ssh_config`.
//!
//! Follows `ssh_config(5)` for what it reads: the user file is read before
//! the system one, the first value obtained for an option wins (only
//! `IdentityFile` accumulates), `Host` lines take `*`/`?` wildcards and
//! `!` negations, and `Include` reads files (globs allowed, relative paths
//! taken from `~/.ssh` or `/etc/ssh`) in place, conditionally when inside
//! a `Host` block.
//!
//! `Match` blocks are skipped as a whole: their criteria (`exec`,
//! `localnetwork`, `canonical`, ...) cannot all be judged here, and
//! applying a block whose condition is false would be worse than not
//! applying one whose condition is true. Options not listed in
//! [`SshHostConfig`] are ignored.
//!
//! `%` tokens are expanded where `ssh_config(5)` allows them, once the
//! port and user are known: see [`Tokens`].

use std::fs;
use std::path::{Path, PathBuf};

/// `ReadConfig` nesting limit, as OpenSSH's `READCONF_MAX_DEPTH`.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Settings for one host, as obtained from the configuration files.
/// `None` means the files do not set the option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshHostConfig {
    /// `HostName`, with `%h` replaced by the host as given.
    pub hostname: Option<String>,
    /// `ProxyJump`, unexpanded; `none` is kept as written. Not set when a
    /// `ProxyCommand` came first, as in OpenSSH.
    pub proxy_jump: Option<String>,
    /// `ProxyCommand`, recorded only because whichever of it and
    /// `ProxyJump` comes first wins; it is never run.
    pub proxy_command: Option<String>,
    /// `User`.
    pub user: Option<String>,
    /// `Port`.
    pub port: Option<u16>,
    /// Every `IdentityFile` of every matching block, in order, with `~`
    /// expanded; `%` tokens are left for [`Self::expand_tokens`].
    pub identity_files: Vec<PathBuf>,
    /// `IdentitiesOnly`.
    pub identities_only: Option<bool>,
    /// `UserKnownHostsFile`; empty for `none`. Tokens as in
    /// `identity_files`.
    pub user_known_hosts_files: Option<Vec<PathBuf>>,
    /// `GlobalKnownHostsFile`; empty for `none`.
    pub global_known_hosts_files: Option<Vec<PathBuf>>,
    /// `HashKnownHosts`.
    pub hash_known_hosts: Option<bool>,
    /// `HostKeyAlias`.
    pub host_key_alias: Option<String>,
    /// `StrictHostKeyChecking`: `true` for `yes`, `false` for `no`, `ask`,
    /// `accept-new` and `off`.
    pub strict_host_key_checking: Option<bool>,
}

/// A configuration file and the directory its relative `Include` paths
/// are taken from.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub include_base: PathBuf,
}

impl ConfigFile {
    /// `~/.ssh/config`, then `/etc/ssh/ssh_config` on Unix.
    pub fn openssh_defaults() -> Vec<Self> {
        let mut files = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let ssh = home.join(".ssh");
            files.push(Self {
                path: ssh.join("config"),
                include_base: ssh,
            });
        }
        if cfg!(unix) {
            files.push(Self {
                path: PathBuf::from("/etc/ssh/ssh_config"),
                include_base: PathBuf::from("/etc/ssh"),
            });
        }
        files
    }
}

impl SshHostConfig {
    /// Settings for `host` from the user's and the system's files.
    pub fn for_host(host: &str) -> Self {
        Self::from_files(host, &ConfigFile::openssh_defaults())
    }

    /// Settings for `host` from `files`, read in order. Missing or
    /// unreadable files contribute nothing.
    pub fn from_files(host: &str, files: &[ConfigFile]) -> Self {
        let mut reader = Reader {
            host: host.to_ascii_lowercase(),
            out: Self::default(),
        };
        for file in files {
            let mut active = true;
            reader.read_file(&file.path, &file.include_base, &mut active, 0);
        }
        let mut out = reader.out;
        if let Some(name) = &out.hostname {
            out.hostname = Some(expand(name, "HostName", &[('h', host.to_string())]));
        }
        out
    }

    /// Expand the `%` tokens of `IdentityFile` and `UserKnownHostsFile`.
    pub fn expand_tokens(&mut self, tokens: &Tokens) {
        let table = tokens.file_tokens();
        let expand_path = |path: &PathBuf, option: &str| match path.to_str() {
            Some(text) if text.contains('%') => PathBuf::from(expand(text, option, &table)),
            _ => path.clone(),
        };
        for path in &mut self.identity_files {
            *path = expand_path(path, "IdentityFile");
        }
        if let Some(files) = &mut self.user_known_hosts_files {
            for path in files {
                *path = expand_path(path, "UserKnownHostsFile");
            }
        }
    }

    /// The jump hosts of `ProxyJump`, tokens expanded, in the order they
    /// are visited; empty when unset or `none`.
    pub fn jump_hosts(&self, tokens: &Tokens) -> Result<Vec<JumpHost>, String> {
        match &self.proxy_jump {
            Some(value) => parse_proxy_jump(&expand(value, "ProxyJump", &tokens.jump_tokens())),
            None => Ok(Vec::new()),
        }
    }

    /// The `HostName` to connect to: the configured one, else `host`.
    pub fn address<'a>(&'a self, host: &'a str) -> &'a str {
        self.hostname.as_deref().unwrap_or(host)
    }
}

struct Reader {
    /// The host as given, lower-cased, which `Host` patterns match.
    host: String,
    out: SshHostConfig,
}

impl Reader {
    fn read_file(&mut self, path: &Path, base: &Path, active: &mut bool, depth: usize) {
        match fs::read_to_string(path) {
            Ok(text) => self.read_text(&text, base, active, depth),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("ssh_config: cannot read {}: {e}", path.display()),
        }
    }

    /// Process `text`. `active` is whether options apply at this point;
    /// it is shared with an including file, as in OpenSSH, so a `Host` line
    /// in an included file governs the lines after it there, and the
    /// including file's state is restored once the include is done.
    fn read_text(&mut self, text: &str, base: &Path, active: &mut bool, depth: usize) {
        for line in text.lines() {
            let Some((keyword, args)) = split_line(line) else {
                continue;
            };
            match keyword.as_str() {
                "host" => *active = host_matches(&self.host, &args),
                // Skipped as a whole; see the module docs.
                "match" => *active = false,
                "include" if *active => {
                    if depth >= MAX_INCLUDE_DEPTH {
                        log::warn!("ssh_config: Include nested too deeply, ignored");
                        continue;
                    }
                    for pattern in &args {
                        for path in include_paths(pattern, base) {
                            self.read_file(&path, base, active, depth + 1);
                            *active = true;
                        }
                    }
                }
                _ if *active => self.apply(&keyword, &args),
                _ => {}
            }
        }
    }

    /// Record an option unless an earlier line already set it.
    fn apply(&mut self, keyword: &str, args: &[String]) {
        let Some(first) = args.first() else {
            return;
        };
        let out = &mut self.out;
        match keyword {
            "hostname" => {
                out.hostname.get_or_insert_with(|| first.clone());
            }
            // Whichever of the two comes first wins, `none` included.
            "proxyjump" => {
                if out.proxy_command.is_none() {
                    out.proxy_jump.get_or_insert_with(|| first.clone());
                }
            }
            "proxycommand" => {
                if out.proxy_jump.is_none() {
                    out.proxy_command.get_or_insert_with(|| args.join(" "));
                }
            }
            "user" => {
                out.user.get_or_insert_with(|| first.clone());
            }
            "port" => {
                if out.port.is_none() {
                    match first.parse::<u16>() {
                        Ok(port) if port != 0 => out.port = Some(port),
                        _ => log::warn!("ssh_config: bad Port {first:?}, ignored"),
                    }
                }
            }
            "identityfile" => {
                let path = expand_tilde(first);
                if !out.identity_files.contains(&path) {
                    out.identity_files.push(path);
                }
            }
            "identitiesonly" => set_flag(&mut out.identities_only, first),
            "userknownhostsfile" => {
                out.user_known_hosts_files
                    .get_or_insert_with(|| path_list(args));
            }
            "globalknownhostsfile" => {
                out.global_known_hosts_files
                    .get_or_insert_with(|| path_list(args));
            }
            "hashknownhosts" => set_flag(&mut out.hash_known_hosts, first),
            "hostkeyalias" => {
                out.host_key_alias.get_or_insert_with(|| first.clone());
            }
            "stricthostkeychecking" => {
                out.strict_host_key_checking
                    .get_or_insert_with(|| is_yes(first));
            }
            _ => {}
        }
    }
}

fn is_yes(value: &str) -> bool {
    value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true")
}

fn set_flag(slot: &mut Option<bool>, value: &str) {
    slot.get_or_insert_with(|| is_yes(value));
}

/// The files of a `UserKnownHostsFile` / `GlobalKnownHostsFile` line;
/// `none` gives no files.
fn path_list(args: &[String]) -> Vec<PathBuf> {
    if args.len() == 1 && args[0].eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    args.iter().map(|a| expand_tilde(a)).collect()
}

/// Split a line into its lower-cased keyword and arguments; `None` for a
/// blank line or a comment. The keyword may be followed by `=`, and
/// arguments may be quoted.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..end].to_ascii_lowercase();
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    if keyword.is_empty() {
        return None;
    }
    Some((keyword, split_args(rest)))
}

/// Whitespace-separated arguments; `"…"` or `'…'` keeps blanks together,
/// and an unquoted `#` starting an argument ends the line.
fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };
        if first == '#' {
            break;
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        while let Some(&c) = chars.peek() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => arg.push(c),
                None if c == '"' || c == '\'' => quote = Some(c),
                None if c.is_whitespace() => break,
                None => arg.push(c),
            }
            chars.next();
        }
        args.push(arg);
    }
    args
}

/// Whether a `Host` line's patterns select `host`: a matching negated
/// pattern rules the line out, otherwise one positive match is needed.
fn host_matches(host: &str, patterns: &[String]) -> bool {
    let mut matched = false;
    for pattern in patterns {
        let pattern = pattern.to_ascii_lowercase();
        if let Some(negated) = pattern.strip_prefix('!') {
            if match_pattern(negated.as_bytes(), host.as_bytes()) {
                return false;
            }
        } else if match_pattern(pattern.as_bytes(), host.as_bytes()) {
            matched = true;
        }
    }
    matched
}

/// Glob match with `*` (any run) and `?` (any one byte), as OpenSSH's
/// `match_pattern`.
pub(crate) fn match_pattern(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// The files an `Include` argument names, sorted as `glob(3)` returns
/// them. Wildcards are allowed in any component; hidden entries match
/// only a pattern that starts with `.`.
fn include_paths(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let path = expand_path(pattern);
    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    let mut found = vec![PathBuf::new()];
    for component in path.components() {
        let part = component.as_os_str();
        let text = part.to_string_lossy();
        if !text.contains(['*', '?']) {
            for p in &mut found {
                p.push(part);
            }
            continue;
        }
        let mut next = Vec::new();
        for dir in &found {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.') || text.starts_with('.'))
                .filter(|n| match_pattern(text.as_bytes(), n.as_bytes()))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| dir.join(n)));
        }
        found = next;
    }
    found.retain(|p| p.is_file());
    found
}

/// Expand a leading `~` to the home directory, as OpenSSH does before
/// expanding `%` tokens.
fn expand_tilde(value: &str) -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        if value == "~" {
            return home;
        }
        if let Some(rest) = value.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

/// An `Include` path: `~` or a leading `%d` stand for the home directory.
fn expand_path(value: &str) -> PathBuf {
    match (value.strip_prefix("%d/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => expand_tilde(value),
    }
}

/// What the `%` tokens of `ssh_config(5)` stand for in one connection.
/// Supported: `%%`, `%d`, `%h`, `%k`, `%n`, `%p`, `%r` and `%u`; any other
/// token is left as written, with a warning.
#[derive(Debug, Clone)]
pub struct Tokens {
    /// `%n`: the host as given.
    pub original_host: String,
    /// `%h`: the host connected to, after `HostName`.
    pub hostname: String,
    /// `%p`: the port connected to.
    pub port: u16,
    /// `%r`: the user logged in as.
    pub remote_user: String,
    /// `%u`: the local user name.
    pub local_user: String,
    /// `%d`: the local home directory, if known.
    pub home: Option<String>,
    /// `%k`: `HostKeyAlias`, else the host as given.
    pub host_key_alias: String,
}

impl Tokens {
    /// For `IdentityFile`, `UserKnownHostsFile` and the other file options.
    fn file_tokens(&self) -> Vec<(char, String)> {
        let mut table = self.jump_tokens();
        table.push(('u', self.local_user.clone()));
        table.push(('k', self.host_key_alias.clone()));
        if let Some(home) = &self.home {
            table.push(('d', home.clone()));
        }
        table
    }

    /// For `ProxyJump`: `%h`, `%n`, `%p` and `%r`.
    fn jump_tokens(&self) -> Vec<(char, String)> {
        vec![
            ('h', self.hostname.clone()),
            ('n', self.original_host.clone()),
            ('p', self.port.to_string()),
            ('r', self.remote_user.clone()),
        ]
    }
}

/// Replace `%%` and the `tokens` of `option` in `value`; any other token
/// is kept as written.
fn expand(value: &str, option: &str, tokens: &[(char, String)]) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(t) => match tokens.iter().find(|(k, _)| *k == t) {
                Some((_, v)) => out.push_str(v),
                None => {
                    log::warn!("ssh_config: {option}: token %{t} not supported, left as is");
                    out.push('%');
                    out.push(t);
                }
            },
            None => out.push('%'),
        }
    }
    out
}

/// One host of a `ProxyJump` list: `[user@]host[:port]` or
/// `ssh://[user@]host[:port]`; IPv6 addresses go in brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpHost {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// The hosts of a `ProxyJump` value, in the order they are visited;
/// empty for `none`.
pub fn parse_proxy_jump(value: &str) -> Result<Vec<JumpHost>, String> {
    if value.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    value.split(',').map(parse_jump_host).collect()
}

fn parse_jump_host(spec: &str) -> Result<JumpHost, String> {
    let bad = || format!("invalid ProxyJump host {spec:?}");
    let rest = spec.trim();
    let rest = match rest.strip_prefix("ssh://") {
        Some(uri) => uri.strip_suffix('/').unwrap_or(uri),
        None => rest,
    };
    let (user, rest) = match rest.rsplit_once('@') {
        Some((user, rest)) if !user.is_empty() => (Some(user.to_string()), rest),
        Some(_) => return Err(bad()),
        None => (None, rest),
    };
    let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']').ok_or_else(bad)?;
        match after {
            "" => (host, None),
            _ => (host, Some(after.strip_prefix(':').ok_or_else(bad)?)),
        }
    } else {
        match rest.split_once(':') {
            Some((host, port)) if !port.contains(':') => (host, Some(port)),
            // A bare IPv6 address.
            Some(_) => (rest, None),
            None => (rest, None),
        }
    };
    if host.is_empty() || host.contains(['/', ' ']) {
        return Err(bad());
    }
    let port = match port {
        Some(p) => match p.parse::<u16>() {
            Ok(port) if port != 0 => Some(port),
            _ => return Err(bad()),
        },
        None => None,
    };
    Ok(JumpHost {
        user,
        host: host.to_string(),
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(host: &str, text: &str) -> SshHostConfig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, text).unwrap();
        SshHostConfig::from_files(
            host,
            &[ConfigFile {
                path,
                include_base: dir.path().to_path_buf(),
            }],
        )
    }

    #[test]
    fn first_value_wins_and_identity_files_accumulate() {
        let text = "\
Host build
    HostName 10.0.0.5
    IdentityFile /keys/build

Host *
    User deploy
    Port 2222
    HostName ignored.example
    IdentityFile /keys/default
";
        let cfg = resolve("build", text);
        assert_eq!(cfg.hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(cfg.user.as_deref(), Some("deploy"));
        assert_eq!(cfg.port, Some(2222));
        assert_eq!(
            cfg.identity_files,
            vec![PathBuf::from("/keys/build"), PathBuf::from("/keys/default")]
        );
        let other = resolve("elsewhere", text);
        assert_eq!(other.hostname.as_deref(), Some("ignored.example"));
        assert_eq!(other.identity_files, vec![PathBuf::from("/keys/default")]);
    }

    #[test]
    fn options_before_any_host_line_apply_to_every_host() {
        let cfg = resolve("x", "User early\nHost x\n  User late\n");
        assert_eq!(cfg.user.as_deref(), Some("early"));
    }

    #[test]
    fn host_patterns_wildcards_negation_and_case() {
        let text = "\
Host *.lan !bad.lan
    User lan
Host web? db
    Port 2200
";
        assert_eq!(resolve("a.lan", text).user.as_deref(), Some("lan"));
        assert_eq!(resolve("A.LAN", text).user.as_deref(), Some("lan"));
        assert_eq!(resolve("bad.lan", text).user, None);
        assert_eq!(resolve("web1", text).port, Some(2200));
        assert_eq!(resolve("db", text).port, Some(2200));
        assert_eq!(resolve("web12", text).port, None);
        // A negation alone matches nothing.
        assert_eq!(resolve("h", "Host !x\n  User u\n").user, None);
    }

    #[test]
    fn match_blocks_are_skipped() {
        let text = "\
Match host build exec \"true\"
    User from-match
    StrictHostKeyChecking yes
Host build
    Port 2201
";
        let cfg = resolve("build", text);
        assert_eq!(cfg.user, None);
        assert_eq!(cfg.strict_host_key_checking, None);
        assert_eq!(cfg.port, Some(2201));
    }

    #[test]
    fn equals_sign_quotes_and_trailing_comments() {
        let text = "\
Host=build
  User=\"deploy\"   # comment
  IdentityFile \"/keys/with space\"
  Port = 2202
";
        let cfg = resolve("build", text);
        assert_eq!(cfg.user.as_deref(), Some("deploy"));
        assert_eq!(cfg.port, Some(2202));
        assert_eq!(cfg.identity_files, vec![PathBuf::from("/keys/with space")]);
    }

    #[test]
    fn hostname_expands_percent_h() {
        let cfg = resolve("box", "Host *\n  HostName %h.internal.example\n");
        assert_eq!(cfg.hostname.as_deref(), Some("box.internal.example"));
        assert_eq!(cfg.address("box"), "box.internal.example");
        assert_eq!(SshHostConfig::default().address("box"), "box");
    }

    #[test]
    fn known_hosts_options() {
        let text = "\
Host build
  UserKnownHostsFile /kh/one /kh/two
  GlobalKnownHostsFile none
  HashKnownHosts yes
  HostKeyAlias build-alias
  StrictHostKeyChecking yes
Host *
  UserKnownHostsFile /kh/late
  StrictHostKeyChecking no
";
        let cfg = resolve("build", text);
        assert_eq!(
            cfg.user_known_hosts_files,
            Some(vec![PathBuf::from("/kh/one"), PathBuf::from("/kh/two")])
        );
        assert_eq!(cfg.global_known_hosts_files, Some(vec![]));
        assert_eq!(cfg.hash_known_hosts, Some(true));
        assert_eq!(cfg.host_key_alias.as_deref(), Some("build-alias"));
        assert_eq!(cfg.strict_host_key_checking, Some(true));
        let other = resolve("other", text);
        assert_eq!(other.strict_host_key_checking, Some(false));
        for value in ["accept-new", "ask", "off"] {
            let cfg = resolve("h", &format!("StrictHostKeyChecking {value}\n"));
            assert_eq!(cfg.strict_host_key_checking, Some(false), "{value}");
        }
    }

    #[test]
    fn include_reads_globbed_files_in_place_and_restores_the_block() {
        let dir = tempfile::tempdir().unwrap();
        let conf_d = dir.path().join("conf.d");
        fs::create_dir(&conf_d).unwrap();
        fs::write(conf_d.join("10-build"), "Host build\n  Port 2203\n").unwrap();
        fs::write(conf_d.join("20-all"), "User included\n").unwrap();
        fs::write(conf_d.join(".hidden"), "User hidden\n").unwrap();
        let main = dir.path().join("config");
        fs::write(
            &main,
            "\
Include conf.d/*
Host build
  Include extra
  Port 1
Host other
  Include conf.d/20-all
",
        )
        .unwrap();
        fs::write(dir.path().join("extra"), "HashKnownHosts yes\n").unwrap();
        let files = [ConfigFile {
            path: main,
            include_base: dir.path().to_path_buf(),
        }];

        // `10-build` sets the port before `Port 1` of the outer block.
        let cfg = SshHostConfig::from_files("build", &files);
        assert_eq!(cfg.port, Some(2203));
        assert_eq!(cfg.user.as_deref(), Some("included"));
        assert_eq!(cfg.hash_known_hosts, Some(true));

        // `10-build` ends on a `Host build` that is inactive for "other";
        // the include restores the top-level state, so `20-all` applies.
        let cfg = SshHostConfig::from_files("other", &files);
        assert_eq!(cfg.user.as_deref(), Some("included"));
        assert_eq!(cfg.port, None);
        assert_eq!(cfg.hash_known_hosts, None);
    }

    #[test]
    fn include_in_an_inactive_block_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("inc"), "User from-include\n").unwrap();
        let cfg = resolve_in(dir.path(), "x", "Host other\n  Include inc\n");
        assert_eq!(cfg.user, None);
        let cfg = resolve_in(dir.path(), "other", "Host other\n  Include inc\n");
        assert_eq!(cfg.user.as_deref(), Some("from-include"));
    }

    #[test]
    fn self_include_stops_at_the_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = resolve_in(dir.path(), "x", "Include config\nUser u\n");
        assert_eq!(cfg.user.as_deref(), Some("u"));
    }

    #[test]
    fn user_file_comes_before_the_system_file() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let system = dir.path().join("system");
        fs::write(&user, "Host h\n  Port 2204\n").unwrap();
        fs::write(&system, "Host *\n  Port 2205\n  User sys\n").unwrap();
        let files = [&user, &system, &dir.path().join("missing")].map(|p| ConfigFile {
            path: p.clone(),
            include_base: dir.path().to_path_buf(),
        });
        let cfg = SshHostConfig::from_files("h", &files);
        assert_eq!(cfg.port, Some(2204));
        assert_eq!(cfg.user.as_deref(), Some("sys"));
    }

    fn resolve_in(dir: &Path, host: &str, text: &str) -> SshHostConfig {
        let path = dir.join("config");
        fs::write(&path, text).unwrap();
        SshHostConfig::from_files(
            host,
            &[ConfigFile {
                path,
                include_base: dir.to_path_buf(),
            }],
        )
    }

    #[test]
    fn match_pattern_like_openssh() {
        assert!(match_pattern(b"*", b"anything"));
        assert!(match_pattern(b"a*c", b"abbbc"));
        assert!(match_pattern(b"[h]:22*", b"[h]:2222"));
        assert!(!match_pattern(b"a*c", b"abd"));
        assert!(!match_pattern(b"a?", b"a"));
    }

    fn tokens() -> Tokens {
        Tokens {
            original_host: "build".into(),
            hostname: "10.0.0.5".into(),
            port: 2222,
            remote_user: "deploy".into(),
            local_user: "me".into(),
            home: Some("/home/me".into()),
            host_key_alias: "build-alias".into(),
        }
    }

    #[test]
    fn file_options_expand_tokens() {
        let mut cfg = resolve(
            "build",
            "\
Host build
  IdentityFile /keys/%n-%h-%p-%r-%u-%k
  IdentityFile %d/.ssh/%r@%h
  IdentityFile /keys/100%%-%x
  UserKnownHostsFile %d/kh/%n /plain/kh
  GlobalKnownHostsFile /global/%h
",
        );
        cfg.expand_tokens(&tokens());
        assert_eq!(
            cfg.identity_files,
            vec![
                PathBuf::from("/keys/build-10.0.0.5-2222-deploy-me-build-alias"),
                PathBuf::from("/home/me/.ssh/deploy@10.0.0.5"),
                // Unsupported tokens are left as written.
                PathBuf::from("/keys/100%-%x"),
            ]
        );
        assert_eq!(
            cfg.user_known_hosts_files,
            Some(vec![
                PathBuf::from("/home/me/kh/build"),
                PathBuf::from("/plain/kh")
            ])
        );
        // GlobalKnownHostsFile takes no tokens in ssh_config(5).
        assert_eq!(
            cfg.global_known_hosts_files,
            Some(vec![PathBuf::from("/global/%h")])
        );
    }

    #[test]
    fn hostname_takes_only_percent_h() {
        let cfg = resolve("box", "HostName %h-%p.%%\n");
        assert_eq!(cfg.hostname.as_deref(), Some("box-%p.%"));
    }

    #[test]
    fn proxy_jump_parses_and_expands() {
        let cfg = resolve(
            "build",
            "Host build\n  ProxyJump alice@gw:2200,ssh://[::1]:2201,%r@%n-jump,[fe80::1]\n",
        );
        let hops = cfg.jump_hosts(&tokens()).unwrap();
        let hop = |user: Option<&str>, host: &str, port| JumpHost {
            user: user.map(String::from),
            host: host.into(),
            port,
        };
        assert_eq!(
            hops,
            vec![
                hop(Some("alice"), "gw", Some(2200)),
                hop(None, "::1", Some(2201)),
                hop(Some("deploy"), "build-jump", None),
                hop(None, "fe80::1", None),
            ]
        );
        assert_eq!(parse_proxy_jump("NONE"), Ok(vec![]));
        assert_eq!(
            parse_proxy_jump("fe80::2").unwrap(),
            vec![hop(None, "fe80::2", None)]
        );
        for bad in ["", "gw,", "@gw", "gw:0", "gw:x", "[::1", "[::1]x", "a b"] {
            assert!(parse_proxy_jump(bad).is_err(), "{bad:?}");
        }
        assert_eq!(SshHostConfig::default().jump_hosts(&tokens()), Ok(vec![]));
    }

    #[test]
    fn proxy_jump_and_proxy_command_first_one_wins() {
        let cfg = resolve("h", "ProxyCommand nc %h %p\nProxyJump gw\n");
        assert_eq!(cfg.proxy_jump, None);
        assert_eq!(cfg.proxy_command.as_deref(), Some("nc %h %p"));
        let cfg = resolve("h", "ProxyJump none\nProxyCommand nc %h %p\nProxyJump gw\n");
        assert_eq!(cfg.proxy_jump.as_deref(), Some("none"));
        assert_eq!(cfg.proxy_command, None);
        assert_eq!(cfg.jump_hosts(&tokens()), Ok(vec![]));
    }
}
