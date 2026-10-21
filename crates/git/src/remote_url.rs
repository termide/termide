//! Convert git remote URLs (ssh/scp/https) into browsable web URLs.

use std::path::Path;

use crate::command::git_command_stdout;

/// Get web URL for a remote.
///
/// Converts various remote URL formats to HTTPS web URLs:
/// - `git@host:user/repo.git` → `https://host/user/repo`
/// - `ssh://git@host/user/repo.git` → `https://host/user/repo`
/// - `https://host/user/repo.git` → `https://host/user/repo`
/// - `http://host/user/repo.git` → `http://host/user/repo`
///
/// Returns `None` if remote is not found or URL format is not recognized.
fn get_remote_web_url(repo: &Path, remote: &str) -> Option<String> {
    let raw = git_command_stdout(repo, &["remote", "get-url", remote])?;
    let url = raw.trim();
    parse_remote_url(url)
}

/// Web URL for a specific commit: `<web_url>/commit/<hash>`.
pub fn get_commit_web_url(repo: &Path, hash: &str) -> Option<String> {
    let base = get_remote_web_url(repo, "origin")?;
    Some(format!("{}/commit/{}", base, hash))
}

/// Host of the remote a bare `git fetch`/`pull`/`push` talks to: the current
/// branch's configured remote, else `origin`. Used to say which server a
/// failed or timed-out network operation was waiting on.
///
/// Returns `None` for a detached or remote-less repository, or a local-path
/// remote, which has no host.
pub fn remote_host(repo: &Path) -> Option<String> {
    let remote = git_command_stdout(repo, &["symbolic-ref", "--short", "-q", "HEAD"])
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
        .and_then(|branch| {
            git_command_stdout(
                repo,
                &["config", "--get", &format!("branch.{branch}.remote")],
            )
        })
        .map(|remote| remote.trim().to_string())
        .filter(|remote| !remote.is_empty() && remote != ".")
        .unwrap_or_else(|| "origin".to_string());
    let raw = git_command_stdout(repo, &["remote", "get-url", &remote])?;
    parse_remote_host(raw.trim())
}

/// Host part of a remote URL: `scheme://[user@]host[:port]/path` or the scp
/// form `[user@]host:path`. `None` for local paths and `file://` URLs.
fn parse_remote_host(url: &str) -> Option<String> {
    let authority = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme == "file" {
            return None;
        }
        let authority = rest.split('/').next()?;
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        // Drop a port, keeping a bracketed IPv6 address whole.
        match authority.strip_prefix('[') {
            Some(v6) => v6.split(']').next()?,
            None => authority.split(':').next()?,
        }
    } else {
        // scp-like syntax needs a colon before any slash; otherwise a path
        // (a Windows drive letter `C:\…` included).
        let colon = url.find(':')?;
        if colon == 1 || url[..colon].contains(['/', '\\']) {
            return None;
        }
        let host = &url[..colon];
        host.rsplit_once('@').map_or(host, |(_, h)| h)
    };
    (!authority.is_empty()).then(|| authority.to_string())
}

/// Parse a git remote URL into a web URL.
fn parse_remote_url(url: &str) -> Option<String> {
    let url = url.trim();

    if let Some(rest) = url.strip_prefix("ssh://") {
        // ssh://git@host/user/repo.git → https://host/user/repo
        let rest = rest.strip_prefix("git@").unwrap_or(rest);
        let stripped = rest.strip_suffix(".git").unwrap_or(rest);
        return Some(format!("https://{}", stripped));
    }

    if url.starts_with("https://") || url.starts_with("http://") {
        // https://host/user/repo.git → https://host/user/repo
        let stripped = url.strip_suffix(".git").unwrap_or(url);
        return Some(stripped.to_string());
    }

    // git@host:user/repo.git → https://host/user/repo
    if let Some(rest) = url.strip_prefix("git@") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        // Replace first ':' with '/'
        if let Some(colon_pos) = rest.find(':') {
            let host = &rest[..colon_pos];
            let path = &rest[colon_pos + 1..];
            return Some(format!("https://{}/{}", host, path));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_remote_url_git_at() {
        assert_eq!(
            parse_remote_url("git@github.com:user/repo.git"),
            Some("https://github.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_git_at_no_suffix() {
        assert_eq!(
            parse_remote_url("git@github.com:user/repo"),
            Some("https://github.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_ssh() {
        assert_eq!(
            parse_remote_url("ssh://git@bitbucket.org/user/repo.git"),
            Some("https://bitbucket.org/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_ssh_no_git_prefix() {
        assert_eq!(
            parse_remote_url("ssh://bitbucket.org/user/repo.git"),
            Some("https://bitbucket.org/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_https() {
        assert_eq!(
            parse_remote_url("https://gitlab.com/user/repo.git"),
            Some("https://gitlab.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_https_no_suffix() {
        assert_eq!(
            parse_remote_url("https://gitlab.com/user/repo"),
            Some("https://gitlab.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_http() {
        assert_eq!(
            parse_remote_url("http://example.com/user/repo.git"),
            Some("http://example.com/user/repo".to_string())
        );
    }

    #[test]
    fn remote_host_comes_from_every_url_form() {
        let cases = [
            ("git@github.com:user/repo.git", Some("github.com")),
            ("github.com:user/repo", Some("github.com")),
            (
                "ssh://git@gitlab.corp:2222/team/repo.git",
                Some("gitlab.corp"),
            ),
            (
                "https://user@bitbucket.org/user/repo",
                Some("bitbucket.org"),
            ),
            ("https://[::1]:8080/repo.git", Some("::1")),
            ("file:///srv/git/repo.git", None),
            ("/srv/git/repo.git", None),
            ("../repo", None),
        ];
        for (url, host) in cases {
            assert_eq!(parse_remote_host(url).as_deref(), host, "{url}");
        }
    }

    #[test]
    fn test_parse_remote_url_unrecognized() {
        assert_eq!(parse_remote_url("some-random-string"), None);
    }
}
