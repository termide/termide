//! Minimal, security-bounded HTTP(S) GET for the viewer panels.
//!
//! This is deliberately *not* a general HTTP client: it does one thing —
//! fetch a document for rendering — under a fixed safety policy:
//!
//! - **Schemes:** `http` and `https` only.
//! - **Timeout:** [`TIMEOUT`] for connect and read.
//! - **Redirects:** at most [`MAX_REDIRECTS`], followed manually so an
//!   `https` origin is never downgraded to `http`.
//! - **Body cap:** [`MAX_BODY`] bytes; larger responses are rejected.
//! - **TLS:** verified against the bundled and the system roots (see
//!   `termide_http`); there is no switch to disable verification.
//!
//! Embedded resources (e.g. `<img>`) are *not* fetched — the viewer renders
//! images as a pictogram, so a page cannot phone home through this client.

use std::io::Read;
use std::time::Duration;

/// Connect + read timeout for a request.
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum number of redirects followed before giving up.
pub const MAX_REDIRECTS: usize = 5;
/// Maximum response body size accepted, in bytes.
pub const MAX_BODY: usize = 8 * 1024 * 1024;

/// User-Agent sent with every request.
const USER_AGENT: &str = concat!("termide/", env!("CARGO_PKG_VERSION"));

/// A successfully fetched document.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// The final URL after following redirects (the document's base URL).
    pub final_url: String,
    /// The response `Content-Type` (media type, lowercased, without parameters).
    pub content_type: String,
    /// The `charset` parameter of the response `Content-Type`, if any.
    pub charset: Option<String>,
    /// The raw response body (capped at [`MAX_BODY`]).
    pub body: Vec<u8>,
}

impl Fetched {
    /// Body decoded as text, lossily: in the encoding a byte order mark
    /// names, else the `Content-Type` charset, else (for HTML) a
    /// `<meta charset>` or `<meta http-equiv>` near the top, else UTF-8.
    #[must_use]
    pub fn text(&self) -> String {
        let declared = self
            .charset
            .as_deref()
            .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
            .or_else(|| {
                (self.content_type.contains("html") || self.content_type.is_empty())
                    .then(|| meta_charset(&self.body))
                    .flatten()
            })
            .unwrap_or(encoding_rs::UTF_8);
        // `decode` lets a byte order mark override the declared encoding.
        let (text, _, _) = declared.decode(&self.body);
        text.into_owned()
    }
}

/// The encoding a `<meta charset>` or `<meta http-equiv="Content-Type"
/// content="…; charset=…">` in the first 1024 bytes of an HTML page names,
/// as a browser's prescan finds it. A UTF-16 label there means UTF-8 (a page
/// that can be read this far as ASCII is not UTF-16).
fn meta_charset(body: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let head = body[..body.len().min(1024)].to_ascii_lowercase();
    let mut rest = head.as_slice();
    while let Some(at) = find(rest, b"<meta") {
        let tag = &rest[at + 5..];
        let tag = &tag[..find(tag, b">").unwrap_or(tag.len())];
        if let Some(at) = find(tag, b"charset") {
            let value = tag[at + 7..]
                .iter()
                .skip_while(|byte| byte.is_ascii_whitespace())
                .skip_while(|byte| **byte == b'=')
                .skip_while(|byte| byte.is_ascii_whitespace() || **byte == b'"' || **byte == b'\'')
                .take_while(|byte| {
                    !matches!(byte, b'"' | b'\'' | b';' | b'/' | b'>')
                        && !byte.is_ascii_whitespace()
                })
                .copied()
                .collect::<Vec<u8>>();
            if let Some(encoding) = encoding_rs::Encoding::for_label(&value) {
                // UTF-16 (and the replacement encoding) become UTF-8.
                return Some(encoding.output_encoding());
            }
        }
        rest = &rest[at + 5..];
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Whether a redirect from `from` scheme to `to` scheme is permitted. Upgrades
/// (`http` → `https`) and same-scheme hops are fine; a downgrade
/// (`https` → `http`) is refused so a secure origin can't be silently
/// stripped to plaintext.
fn redirect_allowed(from: &str, to: &str) -> bool {
    match (from, to) {
        ("https", "https") => true,
        ("https", "http") => false,
        ("http", "https") | ("http", "http") => true,
        _ => false,
    }
}

/// Reject non-`http(s)` schemes early.
fn scheme_ok(scheme: &str) -> bool {
    scheme == "http" || scheme == "https"
}

/// Fetch `raw_url` under the safety policy. Returns a human-readable error
/// string on any failure (network, policy violation, oversized body, …).
pub fn fetch(raw_url: &str) -> Result<Fetched, String> {
    fetch_checked(raw_url, |_| Ok(()))
}

/// [`fetch`], with `allow_hop` judging each redirect target before it is
/// requested: an `Err` refuses the redirect with that message. The caller's
/// own policy for the first URL is its business; this holds the hops it did
/// not see to it.
pub fn fetch_checked(
    raw_url: &str,
    mut allow_hop: impl FnMut(&url::Url) -> Result<(), String>,
) -> Result<Fetched, String> {
    let mut current = url::Url::parse(raw_url).map_err(|e| format!("invalid URL: {e}"))?;
    if !scheme_ok(current.scheme()) {
        return Err(format!("unsupported scheme: {}", current.scheme()));
    }

    // Manual redirect handling (redirects(0)) so each hop's scheme is checked.
    let agent = termide_http::agent_builder()
        .timeout(TIMEOUT)
        .redirects(0)
        .user_agent(USER_AGENT)
        .build();

    for _ in 0..=MAX_REDIRECTS {
        // With `redirects(0)` ureq hands a 3xx back as `Ok`; only 4xx and
        // 5xx come as `Error::Status`.
        match agent.get(current.as_str()).call() {
            Ok(resp) if !(300..400).contains(&resp.status()) => {
                return read_response(&current, resp)
            }
            Ok(resp) => {
                let code = resp.status();
                let location = resp
                    .header("location")
                    .ok_or_else(|| format!("redirect {code} without Location"))?;
                let next = current
                    .join(location)
                    .map_err(|e| format!("bad redirect target: {e}"))?;
                if !scheme_ok(next.scheme()) || !redirect_allowed(current.scheme(), next.scheme()) {
                    return Err(format!(
                        "refused redirect {} -> {}",
                        current.scheme(),
                        next.scheme()
                    ));
                }
                allow_hop(&next)?;
                current = next;
            }
            Err(ureq::Error::Status(code, _)) => {
                return Err(format!("HTTP {code}"));
            }
            Err(ureq::Error::Transport(t)) => {
                return Err(format!("request failed: {t}"));
            }
        }
    }
    Err(format!("too many redirects (> {MAX_REDIRECTS})"))
}

/// Read a 2xx response into a [`Fetched`], enforcing the body-size cap.
fn read_response(url: &url::Url, resp: ureq::Response) -> Result<Fetched, String> {
    // Media type only, lowercased (ureq's, which defaults to `text/plain`).
    let content_type = resp
        .content_type()
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let charset = resp.header("content-type").and_then(charset_of);

    // Read one byte past the cap to detect overflow without trusting
    // Content-Length.
    let mut body = Vec::new();
    if let Err(e) = resp
        .into_reader()
        .take((MAX_BODY as u64) + 1)
        .read_to_end(&mut body)
    {
        // Many servers close the TLS connection without a `close_notify`,
        // which rustls surfaces as `UnexpectedEof` even though the body
        // arrived in full. Browsers and curl tolerate this; so do we, using
        // the bytes already read. Any other error is fatal.
        if e.kind() != std::io::ErrorKind::UnexpectedEof {
            return Err(format!("read failed: {e}"));
        }
    }
    if body.len() > MAX_BODY {
        return Err(format!(
            "response exceeds {} MiB cap",
            MAX_BODY / (1024 * 1024)
        ));
    }

    Ok(Fetched {
        final_url: url.to_string(),
        content_type,
        charset,
        body,
    })
}

/// The `charset` parameter of a `Content-Type` header, unquoted.
fn charset_of(header: &str) -> Option<String> {
    header.split(';').skip(1).find_map(|param| {
        let (key, value) = param.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes() {
        for u in [
            "file:///etc/passwd",
            "ftp://h/x",
            "gopher://h",
            "data:text/plain,hi",
        ] {
            assert!(fetch(u).is_err(), "scheme should be rejected: {u}");
        }
        assert!(!scheme_ok("file"));
        assert!(scheme_ok("http"));
        assert!(scheme_ok("https"));
    }

    #[test]
    fn redirect_downgrade_refused() {
        assert!(redirect_allowed("http", "https"));
        assert!(redirect_allowed("https", "https"));
        assert!(redirect_allowed("http", "http"));
        assert!(
            !redirect_allowed("https", "http"),
            "https->http must be refused"
        );
        assert!(!redirect_allowed("https", "ftp"));
    }

    /// A server answering every request on its port with `response`, once
    /// per connection, counting the requests it got.
    fn serve(response: String) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (addr, hits)
    }

    #[test]
    fn a_refused_hop_is_never_requested() {
        let (target, target_hits) = serve(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi"
                .to_string(),
        );
        let (origin, _) = serve(format!(
            "HTTP/1.1 302 Found\r\nLocation: {target}/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ));
        let mut seen = Vec::new();
        let error = fetch_checked(&origin, |next| {
            seen.push(next.to_string());
            Err("refused".into())
        })
        .unwrap_err();
        assert_eq!(error, "refused");
        assert_eq!(seen, [format!("{target}/secret")]);
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

        let fetched = fetch_checked(&origin, |_| Ok(())).unwrap();
        assert_eq!(fetched.body, b"hi");
        assert_eq!(fetched.final_url, format!("{target}/secret"));
    }

    #[test]
    fn the_content_type_charset_decodes_the_body() {
        use std::io::Write;
        // "Привет" in windows-1251.
        let body: &[u8] = &[0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2];
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=\"Windows-1251\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.write_all(body).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(&response);
            }
        });
        let fetched = fetch(&addr).unwrap();
        assert_eq!(fetched.content_type, "text/plain");
        assert_eq!(fetched.charset.as_deref(), Some("Windows-1251"));
        assert_eq!(fetched.text(), "Привет");
    }

    fn fetched(content_type: &str, charset: Option<&str>, body: &[u8]) -> Fetched {
        Fetched {
            final_url: "https://example.com/".into(),
            content_type: content_type.into(),
            charset: charset.map(str::to_string),
            body: body.to_vec(),
        }
    }

    #[test]
    fn an_html_meta_charset_decodes_the_body_when_the_header_names_none() {
        let mut page = b"<html><head><meta charset='koi8-r'></head><body>".to_vec();
        page.extend_from_slice(&[0xf0, 0xd2, 0xc9, 0xd7, 0xc5, 0xd4]); // "Привет"
        assert!(fetched("text/html", None, &page).text().ends_with("Привет"));

        let mut page =
            br#"<meta http-equiv="Content-Type" content="text/html; charset=iso-8859-1">"#.to_vec();
        page.push(0xe9); // "é"
        assert!(fetched("text/html", None, &page).text().ends_with('é'));

        // The header wins over the page; a byte order mark over both.
        let page = b"<meta charset=koi8-r>\xc3\xa9";
        assert!(fetched("text/html", Some("utf-8"), page)
            .text()
            .ends_with('é'));
        let page = b"\xef\xbb\xbf<meta charset=koi8-r>\xc3\xa9";
        assert_eq!(
            fetched("text/html", Some("windows-1251"), page).text(),
            "<meta charset=koi8-r>é"
        );
        // No declaration at all: UTF-8, and no meta lookup for plain text.
        assert_eq!(
            fetched("text/plain", None, b"<meta charset=koi8-r>\xc3\xa9").text(),
            "<meta charset=koi8-r>é"
        );
    }

    #[test]
    fn invalid_url_errors() {
        assert!(fetch("not a url").is_err());
    }
}
