//! Search engines as data: where to send the query and how to read the
//! results page. The selectors run inside the browser's page, or over a page
//! fetched without one; a markup change is fixed by editing the engine's file.

use base64::Engine as _;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

/// One engine, as `ai/web/engines/<name>.toml` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    /// Display name.
    pub name: String,
    /// Results page; `{query}` is replaced by the URL-encoded query.
    pub url: String,
    /// One element per result.
    pub item: String,
    /// The title, relative to the item; empty is the item itself.
    pub title: String,
    /// The element whose `href` is the result's URL; empty is the item.
    pub link: String,
    /// The snippet, relative to the item; none when absent.
    #[serde(default)]
    pub snippet: Option<String>,
    /// When the engine links through a redirect, the query parameter of the
    /// href that holds the real URL.
    #[serde(default)]
    pub link_param: Option<String>,
    /// A prefix to strip from that parameter before decoding.
    #[serde(default)]
    pub link_param_prefix: Option<String>,
    /// The parameter is base64 (URL-safe alphabet, padding optional).
    #[serde(default)]
    pub link_param_base64: bool,
    /// Results whose URL contains any of these are dropped (ads).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Selectors whose presence means the page is a challenge (a captcha).
    #[serde(default)]
    pub challenge: Vec<String>,
    /// URL fragments that mean the same when the page was redirected.
    #[serde(default)]
    pub challenge_url: Vec<String>,
}

/// A result as the model sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

impl Engine {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.to_string())
    }

    /// The results page for `query`.
    #[must_use]
    pub fn search_url(&self, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        self.url.replace("{query}", &encoded)
    }

    /// A page-side expression that is true once the page shows results or a
    /// challenge, i.e. once there is something to read.
    #[must_use]
    pub fn ready_script(&self) -> String {
        let mut selectors = vec![self.item.clone()];
        selectors.extend(self.challenge.iter().cloned());
        format!(
            "{}.some(s => {{ try {{ return !!document.querySelector(s); }} catch (e) {{ return false; }} }})",
            json!(selectors)
        )
    }

    /// A page-side expression counting the results shown so far.
    #[must_use]
    pub fn count_script(&self) -> String {
        format!(
            "(() => {{ try {{ return document.querySelectorAll({}).length; }} catch (e) {{ return 0; }} }})()",
            json!(self.item)
        )
    }

    /// A page-side expression that is true while the page is a challenge.
    #[must_use]
    pub fn challenge_script(&self) -> String {
        format!(
            "{}.some(s => {{ try {{ return !!document.querySelector(s); }} catch (e) {{ return false; }} }}) \
             || {}.some(u => location.href.includes(u))",
            json!(self.challenge),
            json!(self.challenge_url)
        )
    }

    /// A page-side expression returning the raw results as a JSON array of
    /// `{title, url, snippet}`. The configuration is embedded as JSON, so no
    /// selector can break out of its string.
    #[must_use]
    pub fn extract_script(&self) -> String {
        let config = json!({
            "item": self.item,
            "title": self.title,
            "link": self.link,
            "snippet": self.snippet,
        });
        format!(
            r"(() => {{
  const c = {config};
  const pick = (root, s) => s ? root.querySelector(s) : root;
  const text = (e) => e ? (e.innerText || e.textContent || '').replace(/\s+/g, ' ').trim() : '';
  return [...document.querySelectorAll(c.item)].map(item => {{
    const link = pick(item, c.link);
    return {{
      title: text(pick(item, c.title)),
      url: link ? (link.href || link.getAttribute('href') || '') : '',
      snippet: c.snippet ? text(item.querySelector(c.snippet)) : '',
    }};
  }});
}})()"
        )
    }

    /// Read the results out of a page fetched without a browser, the way
    /// [`Engine::extract_script`] reads them in one: the same selectors, hrefs
    /// resolved against `page_url`, then [`Engine::results`]. Only an engine
    /// whose results page is plain markup (DuckDuckGo's HTML endpoint) has
    /// anything to read here; a challenge page is an error.
    pub fn results_from_html(
        &self,
        html: &str,
        page_url: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let selector = |s: &str| {
            Selector::parse(s).map_err(|error| {
                format!(
                    "{}: selector `{s}` cannot be read without a browser: {error}",
                    self.name
                )
            })
        };
        let document = Html::parse_document(html);
        let challenged = self
            .challenge_url
            .iter()
            .any(|u| page_url.contains(u.as_str()))
            || self
                .challenge
                .iter()
                .filter_map(|s| Selector::parse(s).ok())
                .any(|s| document.select(&s).next().is_some());
        if challenged {
            return Err(format!(
                "{} asks to confirm a human is searching, and without a browser there is no \
                 window to do that in; try again later",
                self.name
            ));
        }
        let item = selector(&self.item)?;
        let title = (!self.title.is_empty())
            .then(|| selector(&self.title))
            .transpose()?;
        let link = (!self.link.is_empty())
            .then(|| selector(&self.link))
            .transpose()?;
        let snippet = self.snippet.as_deref().map(selector).transpose()?;
        let base = Url::parse(page_url).ok();
        fn text(element: Option<ElementRef<'_>>) -> String {
            element.map_or_else(String::new, |e| {
                e.text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        }
        fn pick<'a>(root: ElementRef<'a>, s: Option<&Selector>) -> Option<ElementRef<'a>> {
            match s {
                Some(s) => root.select(s).next(),
                None => Some(root),
            }
        }
        let raw: Vec<Value> = document
            .select(&item)
            .map(|item| {
                let href = pick(item, link.as_ref())
                    .and_then(|a| a.value().attr("href"))
                    .unwrap_or("");
                let url = base
                    .as_ref()
                    .and_then(|base| base.join(href).ok())
                    .map_or_else(|| href.to_string(), |url| url.to_string());
                json!({
                    "title": text(pick(item, title.as_ref())),
                    "url": url,
                    "snippet": text(snippet.as_ref().and_then(|s| item.select(s).next())),
                })
            })
            .collect();
        Ok(self.results(&Value::Array(raw), limit))
    }

    /// Clean up what [`Engine::extract_script`] returned: unwrap redirect
    /// links, drop results without a title or a web URL, excluded and
    /// repeated URLs, and keep at most `limit`.
    #[must_use]
    pub fn results(&self, raw: &Value, limit: usize) -> Vec<SearchResult> {
        let mut seen = std::collections::HashSet::new();
        raw.as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let title = entry["title"].as_str()?.trim().to_string();
                let url = self.unwrap_link(entry["url"].as_str()?)?;
                if title.is_empty() || self.exclude.iter().any(|x| url.contains(x.as_str())) {
                    return None;
                }
                let snippet = entry["snippet"].as_str().unwrap_or("").trim().to_string();
                Some(SearchResult {
                    title,
                    url,
                    snippet,
                })
            })
            .filter(|result| seen.insert(result.url.clone()))
            .take(limit)
            .collect()
    }

    /// The result's real URL: the redirect parameter decoded when the engine
    /// has one, the href itself otherwise; `None` for anything not http(s).
    fn unwrap_link(&self, href: &str) -> Option<String> {
        let href = if href.starts_with("//") {
            format!("https:{href}")
        } else {
            href.to_string()
        };
        let parsed = Url::parse(&href).ok()?;
        let target = match &self.link_param {
            Some(param) => match parsed.query_pairs().find(|(key, _)| key == param.as_str()) {
                Some((_, value)) => {
                    let value = value.into_owned();
                    let value = match &self.link_param_prefix {
                        Some(prefix) => value
                            .strip_prefix(prefix.as_str())
                            .unwrap_or(&value)
                            .to_string(),
                        None => value,
                    };
                    if self.link_param_base64 {
                        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .decode(value.trim_end_matches('='))
                            .ok()?;
                        String::from_utf8(bytes).ok()?
                    } else {
                        value
                    }
                }
                // A direct link among redirected ones.
                None => href,
            },
            None => href,
        };
        let target = Url::parse(&target).ok()?;
        matches!(target.scheme(), "http" | "https").then(|| target.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duckduckgo() -> Engine {
        Engine::from_toml(
            r##"
name = "DuckDuckGo"
url = "https://html.duckduckgo.com/html/?q={query}"
item = ".result:not(.result--ad)"
title = ".result__a"
link = ".result__a"
link_param = "uddg"
snippet = ".result__snippet"
challenge = ["#challenge-form"]
"##,
        )
        .unwrap()
    }

    /// DuckDuckGo's HTML endpoint, cut down to two results and an ad.
    const DUCKDUCKGO_PAGE: &str = r#"<html><body>
<div class="result results_links web-result result--ad">
  <h2 class="result__title"><a class="result__a" href="https://ads.example/x">Ad</a></h2>
</div>
<div class="result results_links web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fgithub.com%2Fservo%2Fhtml5ever&amp;rut=2e87">GitHub - servo/html5ever:
      <b>HTML5</b> parser</a>
  </h2>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x">High-performance   browser-grade
    <b>HTML5</b> parser.</a>
</div>
<div class="result results_links web-result">
  <h2 class="result__title"><a class="result__a" href="/l/?uddg=https%3A%2F%2Fdocs.rs%2Fhtml5ever">html5ever - Rust</a></h2>
</div>
</body></html>"#;

    #[test]
    fn a_page_fetched_without_a_browser_reads_like_one_in_it() {
        let shipped = Engine::from_toml(include_str!(
            "../../agent-core/assets/web/engines/duckduckgo.toml"
        ))
        .unwrap();
        let results = shipped
            .results_from_html(
                DUCKDUCKGO_PAGE,
                "https://html.duckduckgo.com/html/?q=html5ever",
                10,
            )
            .unwrap();
        assert_eq!(
            results,
            vec![
                SearchResult {
                    title: "GitHub - servo/html5ever: HTML5 parser".into(),
                    url: "https://github.com/servo/html5ever".into(),
                    snippet: "High-performance browser-grade HTML5 parser.".into()
                },
                SearchResult {
                    title: "html5ever - Rust".into(),
                    url: "https://docs.rs/html5ever".into(),
                    snippet: String::new()
                },
            ]
        );
    }

    #[test]
    fn a_challenge_without_a_browser_is_an_error() {
        let page = r#"<html><body><form id="challenge-form"></form></body></html>"#;
        let error = duckduckgo()
            .results_from_html(page, "https://html.duckduckgo.com/html/?q=x", 10)
            .unwrap_err();
        assert!(error.contains("confirm a human"), "{error}");
        let bad = Engine {
            item: "div[".into(),
            ..duckduckgo()
        };
        assert!(bad.results_from_html("<p></p>", "https://a/", 10).is_err());
    }

    #[test]
    fn the_query_is_url_encoded_into_the_template() {
        assert_eq!(
            duckduckgo().search_url("rust & c++ ?"),
            "https://html.duckduckgo.com/html/?q=rust+%26+c%2B%2B+%3F"
        );
    }

    #[test]
    fn redirect_links_are_unwrapped_and_bad_entries_dropped() {
        let raw = json!([
            { "title": "html5ever", "url": "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fgithub.com%2Fservo%2Fhtml5ever&rut=x", "snippet": " HTML parser " },
            { "title": "direct", "url": "https://docs.rs/html5ever", "snippet": "" },
            { "title": "again", "url": "//duckduckgo.com/l/?uddg=https%3A%2F%2Fgithub.com%2Fservo%2Fhtml5ever", "snippet": "" },
            { "title": "", "url": "https://empty.title", "snippet": "" },
            { "title": "js", "url": "javascript:void(0)", "snippet": "" },
            { "title": "third", "url": "https://example.org", "snippet": "" }
        ]);
        let results = duckduckgo().results(&raw, 2);
        assert_eq!(
            results,
            vec![
                SearchResult {
                    title: "html5ever".into(),
                    url: "https://github.com/servo/html5ever".into(),
                    snippet: "HTML parser".into()
                },
                SearchResult {
                    title: "direct".into(),
                    url: "https://docs.rs/html5ever".into(),
                    snippet: String::new()
                },
            ]
        );
    }

    #[test]
    fn base64_redirects_with_a_prefix_decode() {
        let engine = Engine {
            link_param: Some("u".into()),
            link_param_prefix: Some("a1".into()),
            link_param_base64: true,
            exclude: vec!["ads.example".into()],
            ..duckduckgo()
        };
        let raw = json!([
            { "title": "Rust", "url": "https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&ntb=1" },
            { "title": "Ad", "url": "https://ads.example/click" }
        ]);
        let results = engine.results(&raw, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://rust-lang.org/");
    }

    #[test]
    fn scripts_embed_selectors_as_json() {
        let engine = Engine {
            item: r#"a[title="x"]"#.into(),
            ..duckduckgo()
        };
        assert!(engine
            .extract_script()
            .contains(r#""item":"a[title=\"x\"]""#));
        assert!(engine
            .ready_script()
            .contains(r##"["a[title=\"x\"]","#challenge-form"]"##));
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(Engine::from_toml(
            "name = \"x\"\nurl = \"u\"\nitem = \"i\"\ntitle = \"\"\nlink = \"\"\nsnipet = \"typo\""
        )
        .is_err());
    }
}
