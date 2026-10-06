//! Links in text and where a followed one leads: the one model the terminal,
//! the agent panel and the viewers share. A panel finds the link under the
//! pointer — one a document marked up, or a web address or path written out
//! in plain text — and hands its [`LinkTarget`] to the app in
//! [`PanelEvent::OpenLink`](crate::PanelEvent::OpenLink), which opens it the
//! same way whichever panel it came from.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Where a link leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// An `http(s)://` or `ftp://` address.
    Url(String),
    /// A local file or directory, absolute.
    Path(PathBuf),
}

impl LinkTarget {
    /// The target of a link a document gives as `href` (a Markdown
    /// `[text](href)`): a web address as it is, a `file://` URL or a path as
    /// a local path, taken from `base` when relative, its `#fragment` and
    /// `?query` dropped. `None` for a same-page `#anchor` or another scheme
    /// (`mailto:`), which a click does not open.
    #[must_use]
    pub fn from_href(href: &str, base: &Path) -> Option<Self> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') {
            return None;
        }
        if is_web(href) {
            return Some(Self::Url(href.to_string()));
        }
        let local = href.strip_prefix("file://").unwrap_or(href);
        if has_scheme(local) {
            return None;
        }
        let local = local.split(['#', '?']).next().unwrap_or(local);
        if local.is_empty() {
            return None;
        }
        Some(Self::Path(expand_path(local, base)))
    }

    /// The target as text, for a status line or the clipboard.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Url(url) => url.clone(),
            Self::Path(path) => path.display().to_string(),
        }
    }
}

/// A web address as plain text writes one: a scheme, then up to the first
/// space or closing delimiter.
static URL_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?:https?|ftp)://[^\s)>\]\}"'`<]+"#).expect("URL regex pattern is valid")
});

/// A path as plain text writes one: Unix `/a`, `./a`, `../a`, `~/a`, Windows
/// `C:\a`, `C:/a` and `\\server\share`. A `:` ends it, so a compiler's
/// `src/a.rs:12:5` gives the file.
static PATH_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?:[A-Za-z]:[/\\][^\s)>\]\}"'`<:*?|]*|\\\\[^\s)>\]\}"'`<:*?|]+|(?:~|\.\.?)?/[^\s)>\]\}"'`<:*?|]+)"#,
    )
    .expect("Path regex pattern is valid")
});

/// Sentence punctuation a link written in prose ends with, not part of it.
const TRAILING: [char; 6] = ['.', ',', ';', ':', '!', '?'];

/// The web addresses written out in `text`, as byte ranges, less any sentence
/// punctuation they end with.
#[must_use]
pub fn url_ranges(text: &str) -> Vec<Range<usize>> {
    URL_REGEX
        .find_iter(text)
        .map(|m| m.start()..m.start() + m.as_str().trim_end_matches(TRAILING).len())
        .filter(|range| !range.is_empty())
        .collect()
}

/// The links written out in `text` — web addresses, and paths that exist,
/// relative ones taken from `cwd` — as char ranges with their targets. A
/// path is only looked for where no address is.
#[must_use]
pub fn links_in(text: &str, cwd: &Path) -> Vec<(Range<usize>, LinkTarget)> {
    let char_at = |byte: usize| text[..byte].chars().count();
    let mut found: Vec<(Range<usize>, LinkTarget)> = url_ranges(text)
        .into_iter()
        .map(|range| {
            let url = text[range.clone()].to_string();
            (
                char_at(range.start)..char_at(range.end),
                LinkTarget::Url(url),
            )
        })
        .collect();
    let urls: Vec<Range<usize>> = found.iter().map(|(range, _)| range.clone()).collect();
    for m in PATH_REGEX.find_iter(text) {
        let start = char_at(m.start());
        if urls.iter().any(|url| url.contains(&start)) {
            continue;
        }
        // The path as written, else without the punctuation of the sentence
        // it closes.
        let raw = m.as_str();
        let trimmed = raw.trim_end_matches(TRAILING);
        for candidate in [raw, trimmed] {
            if candidate.is_empty() || candidate == "/" {
                continue;
            }
            let path = expand_path(candidate, cwd);
            if path.exists() {
                let end = start + candidate.chars().count();
                found.push((start..end, LinkTarget::Path(path)));
                break;
            }
        }
    }
    found.sort_by_key(|(range, _)| range.start);
    found
}

/// The link written out in `text` that covers char `at`, if any.
#[must_use]
pub fn link_at(text: &str, at: usize, cwd: &Path) -> Option<(Range<usize>, LinkTarget)> {
    links_in(text, cwd)
        .into_iter()
        .find(|(range, _)| range.contains(&at))
}

fn is_web(s: &str) -> bool {
    ["http://", "https://", "ftp://"]
        .iter()
        .any(|scheme| s.len() > scheme.len() && s[..scheme.len()].eq_ignore_ascii_case(scheme))
}

/// Whether `href` uses a scheme the app does not open itself (`mailto:`,
/// `tel:`) — neither a web address nor a local file — so it is the system
/// opener's to handle.
#[must_use]
pub fn is_foreign_scheme(href: &str) -> bool {
    let href = href.trim();
    has_scheme(href) && !is_web(href) && !href.starts_with("file://")
}

/// Whether `s` opens with a URL scheme (`mailto:`, `data:`), not counting a
/// Windows drive letter (`C:`).
fn has_scheme(s: &str) -> bool {
    s.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// `raw` as an absolute path: `~` is the home directory, a relative path is
/// taken from `base`.
fn expand_path(raw: &str, base: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with(['/', '\\']) {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest.trim_start_matches(['/', '\\']));
            }
        }
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_drop_the_punctuation_of_their_sentence() {
        let text = "see https://docs.rs/x. and (http://a.b/c), or ftp://f.g";
        let found: Vec<&str> = url_ranges(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["https://docs.rs/x", "http://a.b/c", "ftp://f.g"]);
    }

    #[test]
    fn paths_are_links_only_when_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        let text = "error at ./a.rs:12:5, not ./missing.rs.";
        let found = links_in(text, dir.path());
        assert_eq!(found.len(), 1, "{found:?}");
        let (range, target) = &found[0];
        assert_eq!(
            text.chars()
                .skip(range.start)
                .take(range.len())
                .collect::<String>(),
            "./a.rs"
        );
        assert_eq!(target, &LinkTarget::Path(dir.path().join("./a.rs")));
    }

    #[test]
    fn a_path_inside_an_address_is_the_address() {
        let found = links_in("https://x.y/tmp", Path::new("/"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, LinkTarget::Url("https://x.y/tmp".into()));
    }

    #[test]
    fn link_at_counts_chars_not_bytes() {
        let text = "ссылка https://x.y";
        assert_eq!(link_at(text, 6, Path::new("/")), None);
        let (range, _) = link_at(text, 7, Path::new("/")).unwrap();
        assert_eq!(range, 7..18);
    }

    #[test]
    fn document_hrefs_resolve_against_their_base() {
        let base = Path::new("/doc");
        assert_eq!(
            LinkTarget::from_href("https://x.y/#a", base),
            Some(LinkTarget::Url("https://x.y/#a".into()))
        );
        assert_eq!(
            LinkTarget::from_href("en/agent.md#links", base),
            Some(LinkTarget::Path(PathBuf::from("/doc/en/agent.md")))
        );
        assert_eq!(
            LinkTarget::from_href("file:///etc/hosts", base),
            Some(LinkTarget::Path(PathBuf::from("/etc/hosts")))
        );
        assert_eq!(LinkTarget::from_href("#links", base), None);
        assert_eq!(LinkTarget::from_href("mailto:a@b.c", base), None);
        assert!(is_foreign_scheme("mailto:a@b.c"));
        assert!(!is_foreign_scheme("https://a.b"));
        assert!(!is_foreign_scheme("C:/a.md"));
        assert!(!is_foreign_scheme("a.md"));
    }
}
