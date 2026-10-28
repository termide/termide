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
    /// `User`.
    pub user: Option<String>,
    /// `Port`.
    pub port: Option<u16>,
    /// Every `IdentityFile` of every matching block, in order.
    pub identity_files: Vec<PathBuf>,
    /// `IdentitiesOnly`.
    pub identities_only: Option<bool>,
    /// `UserKnownHostsFile`; empty for `none`.
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
            out.hostname = Some(expand_percent(name, host));
        }
        out
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
                let path = expand_path(first);
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
    args.iter().map(|a| expand_path(a)).collect()
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

/// Expand a leading `~` (or `%d`) to the home directory.
fn expand_path(value: &str) -> PathBuf {
    let home = dirs::home_dir();
    if let Some(home) = &home {
        if value == "~" {
            return home.clone();
        }
        for prefix in ["~/", "%d/"] {
            if let Some(rest) = value.strip_prefix(prefix) {
                return home.join(rest);
            }
        }
    }
    PathBuf::from(value)
}

/// Replace `%h` with `host` and `%%` with `%` in a `HostName` value.
fn expand_percent(value: &str, host: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(host),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
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
}
