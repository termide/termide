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
pub fn get_remote_web_url(repo: &Path, remote: &str) -> Option<String> {
    let raw = git_command_stdout(repo, &["remote", "get-url", remote])?;
    let url = raw.trim();
    parse_remote_url(url)
}

/// Web URL for a specific commit: `<web_url>/commit/<hash>`.
pub fn get_commit_web_url(repo: &Path, hash: &str) -> Option<String> {
    let base = get_remote_web_url(repo, "origin")?;
    Some(format!("{}/commit/{}", base, hash))
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
    fn test_parse_remote_url_unrecognized() {
        assert_eq!(parse_remote_url("some-random-string"), None);
    }
}
