//! Normalised origins: the address a secret is filed under.
//!
//! An origin is `scheme://host[:port]` with a lowercase scheme and host and
//! the scheme's default port dropped. A database login belongs to the
//! server, not to one database on it, so the database name is not part of
//! the origin: switching databases in a panel keeps finding the password.
//! The user is kept apart from the origin, as git credential helpers and
//! browser password managers do.

use std::path::Path;

use url::Url;
use zeroize::Zeroizing;

use crate::Secret;

/// Credentials-relevant parts of a URL.
#[derive(Debug)]
pub struct UrlParts {
    pub origin: String,
    /// Percent-decoded user name, if any.
    pub user: Option<String>,
    /// Percent-decoded password embedded in the URL, if any.
    pub password: Option<Secret>,
}

fn canonical_scheme(scheme: &str) -> String {
    match scheme.to_ascii_lowercase().as_str() {
        "postgresql" => "postgres".into(),
        "mariadb" => "mysql".into(),
        s => s.into(),
    }
}

fn default_port(scheme: &str) -> Option<u16> {
    Some(match scheme {
        "sftp" | "ssh" => 22,
        "ftp" | "ftps" => 21,
        "smb" => 445,
        "postgres" => 5432,
        "mysql" => 3306,
        "http" => 80,
        "https" => 443,
        _ => return None,
    })
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// Split a network URL into origin, user and embedded password.
/// `None` for URLs without a host (local paths, `sqlite:` files).
pub fn parse_url(url: &str) -> Option<UrlParts> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str().filter(|h| !h.is_empty())?.to_ascii_lowercase();
    let scheme = canonical_scheme(u.scheme());
    let mut origin = format!("{scheme}://{host}");
    if let Some(port) = u.port().filter(|p| Some(*p) != default_port(&scheme)) {
        origin.push_str(&format!(":{port}"));
    }
    let user = Some(u.username()).filter(|s| !s.is_empty()).map(decode);
    let password = u.password().map(|p| Zeroizing::new(decode(p)));
    Some(UrlParts {
        origin,
        user,
        password,
    })
}

/// The URL with its password removed; `None` if it cannot be parsed or has
/// no password.
pub fn strip_password(url: &str) -> Option<String> {
    let mut u = Url::parse(url).ok()?;
    u.password()?;
    u.set_password(None).ok()?;
    Some(u.into())
}

/// The URL with `password` filled in (percent-encoded). `None` if the URL
/// cannot carry credentials.
pub fn with_password(url: &str, password: &str) -> Option<Zeroizing<String>> {
    let mut u = Url::parse(url).ok()?;
    u.set_password(Some(password)).ok()?;
    Some(Zeroizing::new(u.into()))
}

/// Origin of an SSH private key's passphrase: its absolute path.
pub fn ssh_key(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_normalises_scheme_host_and_default_port() {
        let p = parse_url("SFTP://Bob@Example.COM:22/home/bob").unwrap();
        assert_eq!(p.origin, "sftp://example.com");
        assert_eq!(p.user.as_deref(), Some("Bob"));
        assert!(p.password.is_none());

        let p = parse_url("ftp://h:2121/x").unwrap();
        assert_eq!(p.origin, "ftp://h:2121");
    }

    #[test]
    fn database_origin_drops_db_name_and_folds_aliases() {
        let p = parse_url("postgresql://u:p%40ss@db:5432/app").unwrap();
        assert_eq!(p.origin, "postgres://db");
        assert_eq!(p.user.as_deref(), Some("u"));
        assert_eq!(p.password.as_deref().map(String::as_str), Some("p@ss"));
        assert_eq!(
            parse_url("mariadb://u@db:3307/x").unwrap().origin,
            "mysql://db:3307"
        );
    }

    #[test]
    fn local_urls_have_no_origin() {
        assert!(parse_url("sqlite:///tmp/a.db").is_none());
        assert!(parse_url("/tmp/x").is_none());
    }

    #[test]
    fn strip_and_inject_password_round_trip() {
        let stripped = strip_password("postgres://u:secret@db/app").unwrap();
        assert_eq!(stripped, "postgres://u@db/app");
        assert!(strip_password("postgres://u@db/app").is_none());
        let back = with_password(&stripped, "p@ss:w/rd").unwrap();
        let p = parse_url(&back).unwrap();
        assert_eq!(p.password.as_deref().map(String::as_str), Some("p@ss:w/rd"));
    }
}
