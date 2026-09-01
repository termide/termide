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
    let username = if parsed.username().is_empty() {
        None
    } else {
        Some(parsed.username().to_string())
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
}
