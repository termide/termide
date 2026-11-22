//! VFS URL parsing utilities.

use std::path::PathBuf;

use crate::error::{VfsError, VfsResult};
use crate::types::{VfsPath, VfsProtocol};

/// Parse a URL string into a VfsPath.
///
/// Supported formats:
/// - `/local/path` - local filesystem
/// - `sftp://user@host:port/path` - SFTP
/// - `sftp://host/path` - SFTP (user from SSH config)
/// - `ftp://host/path` - FTP
/// - `smb://server/share/path` - SMB/CIFS
/// - `nfs://server/export/path` - NFS
/// - `archive://<container>!<inner>` - a path inside an archive file, e.g.
///   `archive:///home/x/a.zip!/docs`; `<container>` is any URL above with
///   `!` and `%` percent-encoded, a missing `!<inner>` means the archive root
pub fn parse_vfs_url(url: &str) -> VfsResult<VfsPath> {
    let url = url.trim();

    // Handle empty URL
    if url.is_empty() {
        return Err(VfsError::InvalidUrl("Empty URL".to_string()));
    }

    // Handle local paths (no scheme)
    if url.starts_with('/') || url.starts_with('.') {
        return Ok(VfsPath::local(PathBuf::from(url)));
    }

    // Handle Windows-style paths (C:\...)
    #[cfg(windows)]
    if url.len() >= 2 && url.chars().nth(1) == Some(':') {
        return Ok(VfsPath::local(PathBuf::from(url)));
    }

    // Handle tilde expansion for home directory
    if url.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            let path = if url == "~" {
                home
            } else if let Some(rest) = url.strip_prefix("~/") {
                home.join(rest)
            } else {
                // ~user/path - not supported, treat as literal
                return Ok(VfsPath::local(PathBuf::from(url)));
            };
            return Ok(VfsPath::local(path));
        }
        return Ok(VfsPath::local(PathBuf::from(url)));
    }

    if let Some(rest) = strip_archive_scheme(url) {
        return parse_archive_url(rest);
    }

    // Parse URL with scheme
    let parsed = url::Url::parse(url).map_err(|e| VfsError::InvalidUrl(e.to_string()))?;

    // Get protocol from scheme
    let protocol = VfsProtocol::from_scheme(parsed.scheme())
        .ok_or_else(|| VfsError::UnsupportedProtocol(parsed.scheme().to_string()))?;

    // Handle file:// URLs
    if protocol == VfsProtocol::Local {
        let path = parsed
            .to_file_path()
            .map_err(|()| VfsError::InvalidUrl("Invalid file URL".to_string()))?;
        return Ok(VfsPath::local(path));
    }

    // Extract host
    let host = parsed
        .host_str()
        .ok_or_else(|| VfsError::InvalidUrl("Missing host".to_string()))?
        .to_string();

    // Extract port (if specified)
    let port = parsed.port();

    // Extract username (if specified)
    // Percent-encoded like the path (an `@` in the name is `%40`).
    let username = if parsed.username().is_empty() {
        None
    } else {
        let decoded = percent_encoding::percent_decode_str(parsed.username())
            .decode_utf8()
            .map_err(|e| VfsError::InvalidUrl(format!("username is not valid UTF-8: {e}")))?;
        Some(decoded.into_owned())
    };

    // Extract path. `url::Url::path()` returns percent-encoded bytes
    // (e.g. "%D1%82%D0%B8" instead of "ти"), but the remote protocols
    // — SFTP/FTP — want the original UTF-8 path. Decode here so the
    // VfsPath stays in the same representation regardless of whether
    // it was constructed directly or round-tripped through a URL.
    let decoded = percent_encoding::percent_decode_str(parsed.path())
        .decode_utf8()
        .map_err(|e| VfsError::InvalidUrl(format!("path is not valid UTF-8: {e}")))?;
    let path = PathBuf::from(decoded.as_ref());

    let mut vfs_path = VfsPath::remote(protocol, host, path);

    if let Some(p) = port {
        vfs_path = vfs_path.with_port(p);
    }

    if let Some(u) = username {
        vfs_path = vfs_path.with_username(u);
    }

    Ok(vfs_path)
}

/// The part after `archive://`, if `url` has that scheme.
fn strip_archive_scheme(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    (VfsProtocol::from_scheme(scheme) == Some(VfsProtocol::Archive)).then_some(rest)
}

/// Parse `<container>!<inner>`, the part of an archive URL after the scheme.
/// The inner path is taken verbatim: only the container part is encoded.
fn parse_archive_url(rest: &str) -> VfsResult<VfsPath> {
    let (container, inner) = rest.split_once('!').unwrap_or((rest, "/"));
    let container = decode_archive_container(container)?;
    if container.is_empty() {
        return Err(VfsError::InvalidUrl(
            "Archive URL without an archive file".to_string(),
        ));
    }
    let container = parse_vfs_url(&container)?;
    if container.file_name().is_none() {
        return Err(VfsError::InvalidUrl(format!(
            "Archive container is not a file: {container}"
        )));
    }
    Ok(VfsPath::archive(container, inner))
}

/// Percent-encode a remote path for [`VfsPath::to_url_string`] so that
/// [`parse_vfs_url`], which decodes it, gets the same path back: `%` starts
/// an escape, `#` and `?` would start a fragment or a query, control
/// characters are dropped by the URL parser and so is trailing whitespace
/// (the whole URL is trimmed). Other characters, spaces and non-ASCII
/// letters included, stay as they are so the URL stays readable where it is
/// shown.
pub fn encode_url_path(path: &str) -> String {
    let keep_len = path.trim_end().len();
    let mut out = String::with_capacity(path.len());
    for (i, c) in path.char_indices() {
        if matches!(c, '%' | '#' | '?') || c.is_control() || i >= keep_len {
            push_escaped(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out
}

/// Percent-encode a username for the userinfo part of a URL: `@`, `:` and
/// `/` would end it early, the rest of the set as in [`encode_url_path`].
/// [`parse_vfs_url`] decodes it.
pub(crate) fn encode_userinfo(user: &str) -> String {
    let mut out = String::with_capacity(user.len());
    for c in user.chars() {
        if matches!(c, '%' | '@' | ':' | '/' | '#' | '?' | '[' | ']' | '\\')
            || c.is_control()
            || c.is_whitespace()
        {
            push_escaped(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out
}

fn push_escaped(out: &mut String, c: char) {
    let mut buf = [0u8; 4];
    for byte in c.encode_utf8(&mut buf).bytes() {
        out.push_str(&format!("%{byte:02X}"));
    }
}

/// Percent-encode the two characters that would make an archive URL's
/// container part ambiguous: `!` ends it, `%` starts an escape.
pub(crate) fn encode_archive_container(container: &str) -> String {
    let mut out = String::with_capacity(container.len());
    for c in container.chars() {
        match c {
            '%' => out.push_str("%25"),
            '!' => out.push_str("%21"),
            _ => out.push(c),
        }
    }
    out
}

/// Reverse [`encode_archive_container`].
fn decode_archive_container(container: &str) -> VfsResult<String> {
    let mut out = String::with_capacity(container.len());
    let mut rest = container;
    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let escape = rest.get(pos..pos + 3);
        match escape {
            Some("%25") => out.push('%'),
            Some("%21") => out.push('!'),
            _ => {
                return Err(VfsError::InvalidUrl(format!(
                    "Unexpected escape in archive container: {}",
                    escape.unwrap_or(&rest[pos..])
                )))
            }
        }
        rest = &rest[pos + 3..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Check if a string looks like a VFS URL (vs plain local path).
pub fn is_vfs_url(s: &str) -> bool {
    let s = s.trim();

    // Check for URL scheme
    if let Some(colon_pos) = s.find(':') {
        if colon_pos > 0 && colon_pos < 10 {
            let scheme = &s[..colon_pos];
            // Check if it's a valid VFS scheme
            return VfsProtocol::from_scheme(scheme).is_some()
                && s.len() > colon_pos + 2
                && s[colon_pos + 1..].starts_with("//");
        }
    }

    false
}

/// Extract connection components from a URL for display.
pub struct UrlComponents {
    pub protocol: VfsProtocol,
    pub display_host: String,
    pub display_path: String,
}

impl UrlComponents {
    /// Parse URL and extract display components.
    pub fn from_url(url: &str) -> Option<Self> {
        let vfs_path = parse_vfs_url(url).ok()?;

        if vfs_path.is_local() {
            return Some(Self {
                protocol: VfsProtocol::Local,
                display_host: String::new(),
                display_path: vfs_path.path.display().to_string(),
            });
        }

        if let Some(container) = vfs_path.container() {
            return Some(Self {
                protocol: VfsProtocol::Archive,
                display_host: container.to_url_string(),
                display_path: vfs_path.path.display().to_string(),
            });
        }

        let display_host = if let Some(ref user) = vfs_path.username {
            format!("{}@{}", user, vfs_path.host.as_deref().unwrap_or("unknown"))
        } else {
            vfs_path
                .host
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        };

        Some(Self {
            protocol: vfs_path.protocol,
            display_host,
            display_path: vfs_path.path.display().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_local_path() {
        let path = parse_vfs_url("/home/user/file.txt").unwrap();
        assert!(path.is_local());
        assert_eq!(path.path, PathBuf::from("/home/user/file.txt"));
    }

    #[test]
    fn test_parse_sftp_url() {
        let path = parse_vfs_url("sftp://user@host.example.com:2222/home/user/file.txt").unwrap();
        assert!(!path.is_local());
        assert_eq!(path.protocol, VfsProtocol::Sftp);
        assert_eq!(path.host, Some("host.example.com".to_string()));
        assert_eq!(path.port, Some(2222));
        assert_eq!(path.username, Some("user".to_string()));
        assert_eq!(path.path, PathBuf::from("/home/user/file.txt"));
    }

    #[test]
    fn test_parse_sftp_url_no_port() {
        let path = parse_vfs_url("sftp://host/path/to/file").unwrap();
        assert_eq!(path.protocol, VfsProtocol::Sftp);
        assert_eq!(path.host, Some("host".to_string()));
        assert_eq!(path.port, None);
        assert_eq!(path.effective_port(), Some(22)); // default SFTP port
    }

    #[test]
    fn test_parse_smb_url() {
        let path = parse_vfs_url("smb://server/share/folder/file.txt").unwrap();
        assert_eq!(path.protocol, VfsProtocol::Smb);
        assert_eq!(path.host, Some("server".to_string()));
        assert_eq!(path.path, PathBuf::from("/share/folder/file.txt"));
    }

    #[test]
    fn test_parse_ftp_url() {
        let path = parse_vfs_url("ftp://ftp.example.com/pub/file.zip").unwrap();
        assert_eq!(path.protocol, VfsProtocol::Ftp);
        assert_eq!(path.effective_port(), Some(21)); // default FTP port
    }

    #[test]
    fn test_parse_ftps_url() {
        let path = parse_vfs_url("ftps://ftp.example.com/pub/file.zip").unwrap();
        assert_eq!(path.protocol, VfsProtocol::Ftps);
        assert_eq!(path.host, Some("ftp.example.com".to_string()));
        assert_eq!(path.effective_port(), Some(990)); // default FTPS port
        assert_eq!(path.path, PathBuf::from("/pub/file.zip"));
    }

    #[test]
    fn test_parse_ftps_url_with_port() {
        let path = parse_vfs_url("ftps://user@host:2121/data").unwrap();
        assert_eq!(path.protocol, VfsProtocol::Ftps);
        assert_eq!(path.port, Some(2121));
        assert_eq!(path.username, Some("user".to_string()));
    }

    #[test]
    fn test_is_vfs_url() {
        assert!(!is_vfs_url("/local/path"));
        assert!(!is_vfs_url("./relative/path"));
        assert!(is_vfs_url("sftp://host/path"));
        assert!(is_vfs_url("ftp://host/path"));
        assert!(is_vfs_url("ftps://host/path"));
        assert!(is_vfs_url("smb://server/share"));
        assert!(!is_vfs_url("unknown://host/path"));
    }

    #[test]
    fn test_vfs_path_to_url_string() {
        let path = VfsPath::remote(VfsProtocol::Sftp, "host", "/path/to/file")
            .with_username("user")
            .with_port(2222);

        assert_eq!(path.to_url_string(), "sftp://user@host:2222/path/to/file");
    }

    #[test]
    fn remote_urls_round_trip_awkward_paths_and_usernames() {
        for path in [
            "/a#b/c?d",
            "/100%41/50%",
            "/My Docs/file name.txt",
            "/тест/文件",
            "/trailing ",
            "/tab\there",
        ] {
            let vfs = VfsPath::remote(VfsProtocol::Sftp, "host", path)
                .with_username("me@corp.example")
                .with_port(2222);
            let url = vfs.to_url_string();
            assert_eq!(parse_vfs_url(&url).unwrap(), vfs, "{path} via {url}");
        }

        let vfs = VfsPath::remote(VfsProtocol::Ftp, "host", "/x").with_username("a:b%c/d");
        assert_eq!(parse_vfs_url(&vfs.to_url_string()).unwrap(), vfs);
    }

    #[test]
    fn remote_urls_stay_readable() {
        let vfs =
            VfsPath::remote(VfsProtocol::Sftp, "host", "/My Docs/тест#1").with_username("me@corp");
        assert_eq!(
            vfs.to_url_string(),
            "sftp://me%40corp@host/My Docs/тест%231"
        );
    }

    #[test]
    fn archive_url_on_a_remote_path_with_a_percent_round_trips() {
        let container = VfsPath::remote(VfsProtocol::Sftp, "host", "/srv/50%41 #1.zip");
        let path = VfsPath::archive(container, "/docs");
        assert_eq!(parse_vfs_url(&path.to_url_string()).unwrap(), path);
    }

    #[test]
    fn test_vfs_path_join() {
        let base = VfsPath::remote(VfsProtocol::Sftp, "host", "/home/user");
        let joined = base.join("subdir/file.txt");

        assert_eq!(joined.path, PathBuf::from("/home/user/subdir/file.txt"));
        assert_eq!(joined.host, Some("host".to_string()));
    }

    #[test]
    fn test_vfs_path_parent() {
        let path = VfsPath::remote(VfsProtocol::Sftp, "host", "/home/user/file.txt");
        let parent = path.parent().unwrap();

        assert_eq!(parent.path, PathBuf::from("/home/user"));
        assert_eq!(parent.host, Some("host".to_string()));
    }

    #[test]
    fn test_connection_key() {
        let path1 = VfsPath::remote(VfsProtocol::Sftp, "host1", "/path").with_username("user");
        let path2 =
            VfsPath::remote(VfsProtocol::Sftp, "host1", "/different/path").with_username("user");
        let path3 = VfsPath::remote(VfsProtocol::Sftp, "host2", "/path").with_username("user");

        // Same host/user should have same connection key
        assert_eq!(path1.connection_key(), path2.connection_key());
        // Different host should have different key
        assert_ne!(path1.connection_key(), path3.connection_key());
    }

    #[test]
    fn log_safe_key_redacts_the_username() {
        let path = VfsPath::remote(VfsProtocol::Sftp, "host1", "/path").with_username("alice");

        // The lookup key identifies the account, so it carries the username;
        // anything that reaches a log must go through `log_safe_key`.
        assert!(path.connection_key().contains("alice"));
        assert!(!path.log_safe_key().contains("alice"));
        assert_eq!(path.log_safe_key(), "sftp://***@host1:22");
    }

    #[test]
    fn archive_url_names_the_container_and_the_inner_path() {
        let path = parse_vfs_url("archive:///home/x/a.zip!/docs/readme.md").unwrap();
        assert!(path.is_archive());
        assert!(path.is_remote());
        assert_eq!(path.container(), Some(&VfsPath::local("/home/x/a.zip")));
        assert_eq!(path.path, PathBuf::from("/docs/readme.md"));
        assert_eq!(path.host, None);
        assert_eq!(
            path.to_url_string(),
            "archive:///home/x/a.zip!/docs/readme.md"
        );
    }

    #[test]
    fn archive_url_without_inner_path_is_the_archive_root() {
        let path = parse_vfs_url("archive:///a.tar.gz").unwrap();
        assert_eq!(path.path, PathBuf::from("/"));
        assert_eq!(path.to_url_string(), "archive:///a.tar.gz!/");
        assert_eq!(parse_vfs_url("archive:///a.tar.gz!").unwrap(), path);
    }

    #[test]
    fn archive_url_round_trips_bang_and_percent_in_names() {
        let container = VfsPath::local("/tmp/wow!50%.zip");
        let path = VfsPath::archive(container, "/a!b/100%");
        let url = path.to_url_string();
        assert_eq!(url, "archive:///tmp/wow%2150%25.zip!/a!b/100%");
        assert_eq!(parse_vfs_url(&url).unwrap(), path);
    }

    #[test]
    fn archive_url_on_a_remote_host() {
        let url = "archive://sftp://user@host:2222/srv/a.zip!/etc";
        let path = parse_vfs_url(url).unwrap();
        let container = path.container().unwrap();
        assert_eq!(container.protocol, VfsProtocol::Sftp);
        assert_eq!(container.username.as_deref(), Some("user"));
        assert_eq!(container.path, PathBuf::from("/srv/a.zip"));
        assert_eq!(path.to_url_string(), url);
        assert!(is_vfs_url(url));
    }

    #[test]
    fn archive_inside_an_archive_round_trips() {
        let outer = VfsPath::archive(VfsPath::local("/a.zip"), "/lib/b.tar");
        let inner = VfsPath::archive(outer, "/src");
        let url = inner.to_url_string();
        assert_eq!(url, "archive://archive:///a.zip%21/lib/b.tar!/src");
        assert_eq!(parse_vfs_url(&url).unwrap(), inner);
    }

    #[test]
    fn archive_url_rejects_a_missing_or_bad_container() {
        assert!(parse_vfs_url("archive://!/x").is_err());
        assert!(parse_vfs_url("archive:///").is_err());
        assert!(parse_vfs_url("archive:///a%2.zip!/").is_err());
        assert!(parse_vfs_url("archive:///a%41.zip!/").is_err());
    }

    #[test]
    fn archive_path_navigation_stays_inside_the_archive() {
        let root = VfsPath::archive(VfsPath::local("/x/a.zip"), "docs");
        assert_eq!(root.path, PathBuf::from("/docs"));

        let file = root.join("readme.md");
        assert_eq!(file.container(), root.container());
        assert_eq!(file.parent(), Some(root.clone()));

        let top = root.parent().unwrap();
        assert_eq!(top.path, PathBuf::from("/"));
        assert_eq!(top.parent(), None);
        assert_eq!(top.container(), Some(&VfsPath::local("/x/a.zip")));
    }

    #[test]
    fn archive_connection_key_is_per_archive_file() {
        let a = VfsPath::local("/x/a.zip");
        let in_a = VfsPath::archive(a.clone(), "/one");
        let elsewhere_in_a = VfsPath::archive(a, "/two/three");
        let in_b = VfsPath::archive(VfsPath::local("/x/b.zip"), "/one");

        assert_eq!(in_a.connection_key(), elsewhere_in_a.connection_key());
        assert_ne!(in_a.connection_key(), in_b.connection_key());
        assert_ne!(
            in_a.connection_key(),
            VfsPath::local("/x/a.zip").connection_key()
        );
    }

    #[test]
    fn archive_log_safe_key_redacts_the_container_username() {
        let container =
            VfsPath::remote(VfsProtocol::Sftp, "host1", "/srv/a.zip").with_username("alice");
        let path = VfsPath::archive(container, "/");
        assert!(path.connection_key().contains("alice"));
        assert_eq!(path.log_safe_key(), "archive:sftp://***@host1:22/a.zip");
    }
}
