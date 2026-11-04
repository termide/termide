//! The headers that identify termide to a hosted endpoint.
//!
//! A server with no per-app notion ignores them, so they are added only when
//! the connection both names an endpoint that keeps a breakdown by app and has
//! the user switched the identification on — see `Connection::attribution` in
//! termide-config. The endpoint's own headers are set by the caller and are
//! left alone here.

use termide_agent_core::Attribution;

/// Add `attribution`'s headers to `http`, which is returned either way.
/// Nothing is added for `None`, so a caller passes what it has and does not
/// have to know why there may be none.
pub(crate) fn apply(http: ureq::Request, attribution: Option<Attribution>) -> ureq::Request {
    let Some(attribution) = attribution else {
        return http;
    };
    http.set("HTTP-Referer", attribution.referer)
        .set("X-OpenRouter-Title", attribution.title)
        .set("X-OpenRouter-Categories", attribution.categories)
}
