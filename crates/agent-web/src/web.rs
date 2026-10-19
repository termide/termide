//! The service behind the web tools: which backend does the work, the one
//! browser every agent of the process shares, and the cache of pages read.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use termide_agent_core::CancelToken;
use termide_html_markdown::html_to_markdown;

use crate::browser::{find_chrome, Browser, Display, Page};
use crate::engine::{Engine, SearchResult};

/// The browser quits after this long without a call.
const IDLE_SHUTDOWN: Duration = Duration::from_secs(5 * 60);
/// How often the idle check runs.
const IDLE_CHECK: Duration = Duration::from_secs(15);
/// How long the user has to get through a challenge.
const CHALLENGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a search results page may take to show results or a challenge.
const RESULTS_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a results list may keep growing once it has appeared.
const RESULTS_SETTLE: Duration = Duration::from_secs(3);
/// Pages kept for paging through with `offset`.
const CACHE_PAGES: usize = 8;
/// How long a cached page stays fresh.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// Which backend does the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Chrome when one is found, plain HTTP otherwise.
    Auto,
    Chrome,
    Http,
}

impl Backend {
    /// The setting's value; unknown text falls back to `auto`.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "chrome" => Self::Chrome,
            "http" => Self::Http,
            _ => Self::Auto,
        }
    }
}

impl Display {
    /// The setting's value; unknown text falls back to `headless`.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "minimized" => Self::Minimized,
            "visible" => Self::Visible,
            _ => Self::Headless,
        }
    }
}

/// Everything the service needs, resolved by the app from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    pub backend: Backend,
    /// The engine for `web_search`; `None` leaves the tool out.
    pub engine: Option<Engine>,
    /// An explicit browser executable; `None` looks in the usual places.
    pub chrome_path: Option<PathBuf>,
    pub display: Display,
    /// The agent's own browser profile.
    pub profile: PathBuf,
}

/// A page as `fetch` returns it, before paging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPage {
    pub url: String,
    pub title: String,
    /// Markdown for HTML, the text itself for other text types.
    pub content: String,
}

struct BrowserSlot {
    browser: Option<Browser>,
    last_used: Instant,
    /// The display to launch with; a headless browser that met a challenge
    /// switches this to a minimized window until the browser next closes.
    display: Display,
    /// The display the settings ask for, restored when the browser closes.
    configured: Display,
    reaper_started: bool,
}

struct CachedPage {
    requested: String,
    page: FetchedPage,
    at: Instant,
}

pub struct Web {
    chrome: Option<PathBuf>,
    engine: Option<Engine>,
    profile: PathBuf,
    slot: Mutex<BrowserSlot>,
    cache: Mutex<VecDeque<CachedPage>>,
    /// The user asked to watch the browser: it runs in a visible window
    /// whatever the display setting says, until they stop watching.
    watching: AtomicBool,
}

impl Web {
    #[must_use]
    pub fn new(config: WebConfig) -> Arc<Self> {
        let chrome = match config.backend {
            Backend::Http => None,
            Backend::Auto | Backend::Chrome => config
                .chrome_path
                .filter(|path| path.is_file())
                .or_else(find_chrome)
                .filter(|_| cfg!(unix)),
        };
        if config.backend == Backend::Chrome && chrome.is_none() {
            log::warn!("web backend is chrome but no usable browser was found; using http");
        }
        // `visible` in the settings starts out watched; stopping then falls
        // back to no window.
        let display = effective_display(config.display);
        let unwatched = if display == Display::Visible {
            Display::Headless
        } else {
            display
        };
        Arc::new(Self {
            chrome,
            engine: config.engine,
            profile: config.profile,
            slot: Mutex::new(BrowserSlot {
                browser: None,
                last_used: Instant::now(),
                display: unwatched,
                configured: unwatched,
                reaper_started: false,
            }),
            cache: Mutex::new(VecDeque::new()),
            watching: AtomicBool::new(display == Display::Visible),
        })
    }

    /// Whether the browser can be shown in a window at all: there is one to
    /// drive and a display server to show it on.
    #[must_use]
    pub fn can_show(&self) -> bool {
        self.chrome.is_some() && effective_display(Display::Visible) == Display::Visible
    }

    /// Whether the browser runs in a visible window for the user to watch.
    #[must_use]
    pub fn is_watching(&self) -> bool {
        self.watching.load(Ordering::Relaxed)
    }

    /// Show the browser in a visible window, or stop showing it. Returns at
    /// once; the browser is relaunched on a thread of its own (after any call
    /// in progress), so the window opens or closes right away rather than on
    /// the agent's next page. Profile and cookies carry over.
    pub fn set_watching(self: &Arc<Self>, watching: bool) {
        if !self.can_show() || self.watching.swap(watching, Ordering::Relaxed) == watching {
            return;
        }
        let web = Arc::clone(self);
        std::thread::spawn(move || {
            if watching {
                if let Err(error) = web.with_browser(&CancelToken::new(), |_, _| Ok(())) {
                    log::warn!("cannot show the web browser: {error}");
                }
                return;
            }
            let mut slot = web.slot.lock().unwrap_or_else(PoisonError::into_inner);
            let shown = slot
                .browser
                .as_ref()
                .is_some_and(|browser| browser.display() == Display::Visible);
            if shown {
                if let Some(browser) = slot.browser.take() {
                    browser.close();
                }
            }
        });
    }

    /// The display a browser launched now gets.
    fn launch_display(&self, slot: &BrowserSlot) -> Display {
        if self.is_watching() {
            Display::Visible
        } else {
            slot.display
        }
    }

    /// Whether `web_search` can work: it needs an engine. Without the
    /// browser the results page is fetched over plain HTTP, which only an
    /// engine serving plain markup (DuckDuckGo, the default) answers well.
    #[must_use]
    pub fn can_search(&self) -> bool {
        self.engine.is_some()
    }

    /// The name of the backend in use, for messages.
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        if self.chrome.is_some() {
            "chrome"
        } else {
            "http"
        }
    }

    /// Load `url` (or take it from the cache).
    pub fn fetch(self: &Arc<Self>, url: &str, cancel: &CancelToken) -> Result<FetchedPage, String> {
        if let Some(page) = self.cached(url) {
            return Ok(page);
        }
        let page = match &self.chrome {
            Some(_) => {
                self.with_browser(cancel, |browser, _| fetch_in_browser(browser, url, cancel))?
            }
            None => fetch_over_http(url)?,
        };
        self.remember(url, &page);
        Ok(page)
    }

    /// Ask the engine for `query`. `on_wait` is told once when the user has
    /// to get through a challenge in the browser window.
    pub fn search(
        self: &Arc<Self>,
        query: &str,
        limit: usize,
        cancel: &CancelToken,
        on_wait: &mut dyn FnMut(String),
    ) -> Result<Vec<SearchResult>, String> {
        let engine = self
            .engine
            .as_ref()
            .ok_or("no search engine is configured")?;
        let url = engine.search_url(query);
        if self.chrome.is_none() {
            return search_over_http(engine, &url, limit);
        }
        let raw = self.with_browser(cancel, |browser, display| {
            results_page(browser, display, engine, &url, cancel, on_wait)
        });
        let raw = match raw {
            // A headless browser has no window to show the challenge in:
            // relaunch with one and ask again.
            Err(error) if error == NEEDS_WINDOW => {
                self.switch_to_window();
                self.with_browser(cancel, |browser, display| {
                    results_page(browser, display, engine, &url, cancel, on_wait)
                })?
            }
            other => other?,
        };
        Ok(engine.results(&raw, limit))
    }

    /// Run `work` with the shared browser, starting it when needed. Calls
    /// from several agents take turns.
    fn with_browser<T>(
        self: &Arc<Self>,
        cancel: &CancelToken,
        work: impl FnOnce(&Browser, Display) -> Result<T, String>,
    ) -> Result<T, String> {
        let chrome = self.chrome.as_ref().ok_or("no browser")?;
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let display = self.launch_display(&slot);
        let usable = slot
            .browser
            .as_mut()
            .is_some_and(|browser| browser.is_alive() && browser.display() == display);
        if !usable {
            // Dead, or started with another display (watching was switched).
            if let Some(browser) = slot.browser.take() {
                browser.close();
            }
            slot.browser = Some(Browser::launch(chrome, &self.profile, display)?);
            if !slot.reaper_started {
                slot.reaper_started = true;
                start_reaper(Arc::downgrade(self));
            }
        }
        slot.last_used = Instant::now();
        let result = work(slot.browser.as_ref().expect("launched above"), display);
        slot.last_used = Instant::now();
        result
    }

    /// Close a headless browser and launch a minimized window instead, until
    /// that browser closes; what the user solves there stays in the profile
    /// for the headless browser after it.
    fn switch_to_window(&self) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.display = Display::Minimized;
        if let Some(browser) = slot.browser.take() {
            browser.close();
        }
    }

    fn cached(&self, url: &str) -> Option<FetchedPage> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.retain(|entry| entry.at.elapsed() < CACHE_TTL);
        cache
            .iter()
            .find(|entry| entry.requested == url || entry.page.url == url)
            .map(|entry| entry.page.clone())
    }

    pub(crate) fn remember(&self, url: &str, page: &FetchedPage) {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.retain(|entry| entry.requested != url);
        cache.push_back(CachedPage {
            requested: url.to_string(),
            page: page.clone(),
            at: Instant::now(),
        });
        while cache.len() > CACHE_PAGES {
            cache.pop_front();
        }
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.slot.lock() {
            if let Some(browser) = slot.browser.take() {
                browser.close();
            }
        }
    }
}

/// Marker error: a headless browser met a challenge it cannot show.
const NEEDS_WINDOW: &str = "\u{0}needs-window";

/// A window cannot open where there is no display server; a Linux box
/// without one runs headless whatever the setting says.
fn effective_display(display: Display) -> Display {
    if cfg!(target_os = "linux")
        && display != Display::Headless
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return Display::Headless;
    }
    display
}

/// Open the results page at `url`, get through a challenge when the display
/// allows it, and return the raw results once the list has stopped growing.
fn results_page(
    browser: &Browser,
    display: Display,
    engine: &Engine,
    url: &str,
    cancel: &CancelToken,
    on_wait: &mut dyn FnMut(String),
) -> Result<serde_json::Value, String> {
    let page = browser.open_until(url, &engine.ready_script(), RESULTS_TIMEOUT, cancel)?;
    if is_challenge(&page, engine, cancel) {
        if display == Display::Headless {
            return Err(NEEDS_WINDOW.to_string());
        }
        get_through_challenge(&page, engine, display, cancel, on_wait)?;
    }
    // Some engines render the list in pieces.
    page.wait_until_steady(&engine.count_script(), RESULTS_SETTLE, cancel);
    page.eval(&engine.extract_script(), cancel)
}

fn is_challenge(page: &Page<'_>, engine: &Engine, cancel: &CancelToken) -> bool {
    page.eval(&engine.challenge_script(), cancel)
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Show the window, wait for the user to solve the challenge, then put the
/// window back the way it was.
fn get_through_challenge(
    page: &Page<'_>,
    engine: &Engine,
    display: Display,
    cancel: &CancelToken,
    on_wait: &mut dyn FnMut(String),
) -> Result<(), String> {
    let _ = page.set_window_state("normal", cancel);
    page.bring_to_front(cancel);
    on_wait(format!(
        "{} asks to confirm a human is searching: solve the check in the browser window",
        engine.name
    ));
    let passed = page.wait_while(CHALLENGE_TIMEOUT, cancel, |page| {
        is_challenge(page, engine, cancel)
    })?;
    if display == Display::Minimized {
        let _ = page.set_window_state("minimized", cancel);
    }
    if !passed {
        return Err(format!(
            "{} kept asking for a human check; nobody solved it within {} minutes",
            engine.name,
            CHALLENGE_TIMEOUT.as_secs() / 60
        ));
    }
    // The engine sends the user back to the results once the check passes.
    page.wait_for(&engine.ready_script(), RESULTS_TIMEOUT, cancel);
    Ok(())
}

/// Close the browser once it has been idle long enough; ends with the service.
fn start_reaper(web: Weak<Web>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(IDLE_CHECK);
        let Some(web) = web.upgrade() else { return };
        // A window the user is watching stays until they stop watching.
        let idle = match web.slot.try_lock() {
            Ok(mut slot) if slot.last_used.elapsed() >= IDLE_SHUTDOWN && !web.is_watching() => {
                slot.display = slot.configured;
                slot.browser.take()
            }
            _ => None,
        };
        if let Some(browser) = idle {
            log::debug!("closing the idle web browser");
            browser.close();
        }
    });
}

/// What the page in the browser holds: markdown for HTML, the text for other
/// text types, an error for the rest.
fn fetch_in_browser(
    browser: &Browser,
    url: &str,
    cancel: &CancelToken,
) -> Result<FetchedPage, String> {
    let page = browser.open(url, cancel)?;
    let info = page.eval(
        "({ type: document.contentType, url: location.href, title: document.title, \
          html: document.contentType.includes('html') ? document.documentElement.outerHTML : null, \
          text: document.body ? document.body.innerText : '' })",
        cancel,
    )?;
    let final_url = info["url"].as_str().unwrap_or(url).to_string();
    let content_type = info["type"].as_str().unwrap_or("");
    if let Some(html) = info["html"].as_str() {
        let converted = html_to_markdown(html, &final_url);
        return Ok(FetchedPage {
            url: final_url,
            title: converted.title,
            content: converted.markdown,
        });
    }
    if is_text_type(content_type) {
        return Ok(FetchedPage {
            url: final_url,
            title: info["title"].as_str().unwrap_or("").to_string(),
            content: info["text"].as_str().unwrap_or("").to_string(),
        });
    }
    Err(format!(
        "{final_url} is {content_type}, not text; `fetch` reads text only"
    ))
}

/// The search without a browser: the results page over plain HTTP, read
/// with the engine's selectors.
fn search_over_http(engine: &Engine, url: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
    let fetched = termide_fetch::fetch(url)?;
    let results = engine.results_from_html(&fetched.text(), &fetched.final_url, limit)?;
    if results.is_empty() {
        return Err(format!(
            "{} returned no results for this query without a browser; an engine that \
             builds its results with JavaScript needs Chrome or Chromium, DuckDuckGo does not",
            engine.name
        ));
    }
    Ok(results)
}

fn fetch_over_http(url: &str) -> Result<FetchedPage, String> {
    let fetched = termide_fetch::fetch(url)?;
    let content_type = fetched.content_type.as_str();
    if content_type.contains("html") || (content_type.is_empty() && looks_like_html(&fetched.body))
    {
        let converted = html_to_markdown(&fetched.text(), &fetched.final_url);
        return Ok(FetchedPage {
            url: fetched.final_url,
            title: converted.title,
            content: converted.markdown,
        });
    }
    if is_text_type(content_type) || (content_type.is_empty() && !fetched.body.contains(&0)) {
        return Ok(FetchedPage {
            title: String::new(),
            content: fetched.text(),
            url: fetched.final_url,
        });
    }
    Err(format!(
        "{} is {content_type}, not text; `fetch` reads text only",
        fetched.final_url
    ))
}

fn is_text_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || ["json", "xml", "javascript", "yaml", "toml", "csv"]
            .iter()
            .any(|kind| content_type.contains(kind))
}

fn looks_like_html(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_values_parse_with_safe_fallbacks() {
        assert_eq!(Backend::parse("chrome"), Backend::Chrome);
        assert_eq!(Backend::parse("http"), Backend::Http);
        assert_eq!(Backend::parse(""), Backend::Auto);
        assert_eq!(Display::parse("minimized"), Display::Minimized);
        assert_eq!(Display::parse("visible"), Display::Visible);
        assert_eq!(Display::parse("nonsense"), Display::Headless);
    }

    #[test]
    fn text_types_are_recognised() {
        for kind in [
            "text/plain",
            "application/json",
            "application/xml",
            "text/markdown",
        ] {
            assert!(is_text_type(kind), "{kind}");
        }
        for kind in ["application/pdf", "image/png", "application/octet-stream"] {
            assert!(!is_text_type(kind), "{kind}");
        }
        assert!(looks_like_html(b"  <!DOCTYPE html><html>"));
        assert!(!looks_like_html(b"{\"a\": 1}"));
    }

    #[test]
    fn a_visible_setting_starts_watched_and_stops_to_no_window() {
        let web = Web::new(WebConfig {
            backend: Backend::Http,
            engine: None,
            chrome_path: None,
            display: Display::Visible,
            profile: std::env::temp_dir(),
        });
        let slot = web.slot.lock().unwrap();
        // Without a display server the setting itself falls back to headless.
        if effective_display(Display::Visible) == Display::Visible {
            assert!(web.is_watching());
            assert_eq!(web.launch_display(&slot), Display::Visible);
        }
        web.watching.store(false, Ordering::Relaxed);
        assert_eq!(web.launch_display(&slot), Display::Headless);
    }

    #[test]
    fn watching_needs_a_browser() {
        let web = Web::new(WebConfig {
            backend: Backend::Http,
            engine: None,
            chrome_path: None,
            display: Display::Headless,
            profile: std::env::temp_dir(),
        });
        assert!(!web.can_show());
        web.set_watching(true);
        assert!(!web.is_watching(), "nothing to show without a browser");
    }

    #[test]
    fn the_http_backend_searches_with_an_engine() {
        let config = WebConfig {
            backend: Backend::Http,
            engine: None,
            chrome_path: None,
            display: Display::Headless,
            profile: std::env::temp_dir(),
        };
        let web = Web::new(config.clone());
        assert!(!web.can_search(), "no engine, no search");
        assert_eq!(web.backend_name(), "http");
        let engine = Engine::from_toml(include_str!(
            "../../agent-core/assets/web/engines/duckduckgo.toml"
        ))
        .unwrap();
        let web = Web::new(WebConfig {
            engine: Some(engine),
            ..config
        });
        assert!(web.can_search());
        assert_eq!(web.backend_name(), "http");
    }

    #[test]
    fn the_cache_keeps_the_latest_pages_by_either_url() {
        let web = Web::new(WebConfig {
            backend: Backend::Http,
            engine: None,
            chrome_path: None,
            display: Display::Headless,
            profile: std::env::temp_dir(),
        });
        for n in 0..=CACHE_PAGES {
            web.remember(
                &format!("https://a/{n}"),
                &FetchedPage {
                    url: format!("https://b/{n}"),
                    title: String::new(),
                    content: n.to_string(),
                },
            );
        }
        assert!(web.cached("https://a/0").is_none(), "the oldest is evicted");
        assert_eq!(web.cached("https://a/1").unwrap().content, "1");
        assert_eq!(web.cached("https://b/2").unwrap().content, "2");
    }
}
