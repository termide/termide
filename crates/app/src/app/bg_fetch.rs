//! Viewer URL fetching (a viewer's `Ctrl+G` or a followed link): spawns the
//! blocking GET on a worker thread, then opens the result in the viewer that
//! matches its Content-Type. Includes the URL/HTML helper functions.

use std::sync::mpsc::TryRecvError;

use super::App;

impl App {
    /// Start a background fetch of `url` from a viewer's `Ctrl+G`, opening the
    /// result in a *new* viewer.
    pub(super) fn start_url_fetch(&mut self, url: String) {
        self.start_fetch(url, false);
    }

    /// Start a background fetch that replaces the *active* viewer in place
    /// (a followed link or a history step inside a fetched page).
    pub(super) fn start_url_fetch_in_place(&mut self, url: String) {
        self.start_fetch(url, true);
    }

    /// Spawn the blocking GET on a worker thread; the result is delivered over
    /// a channel and picked up by [`check_view_fetch`](App::check_view_fetch).
    fn start_fetch(&mut self, url: String, in_place: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.state.view_fetch_receiver = Some(rx);
        self.state.view_fetch_in_place = in_place;
        self.state.set_info(format!("Fetching {url}…"));
        std::thread::spawn(move || {
            let _ = tx.send(termide_fetch::fetch(&url));
        });
        self.state.needs_redraw = true;
    }

    /// Poll the in-flight URL fetch, if any, and open the result on completion.
    pub(super) fn check_view_fetch(&mut self) {
        let Some(rx) = self.state.view_fetch_receiver.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.state.view_fetch_receiver = None;
                let in_place = self.state.view_fetch_in_place;
                match result {
                    Ok(fetched) if in_place => self.apply_fetched_in_place(fetched),
                    Ok(fetched) => self.open_fetched(fetched),
                    Err(e) => self.show_error_modal(format!("Fetch failed: {e}")),
                }
                self.state.needs_redraw = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state.view_fetch_receiver = None;
            }
        }
    }

    /// Open a fetched document in a new viewer, routed by Content-Type.
    fn open_fetched(&mut self, fetched: termide_fetch::Fetched) {
        let title = fetch_title(&fetched.final_url);
        let url = fetched.final_url.clone();
        match classify(&fetched) {
            Some(ViewKind::Html(src)) => {
                self.add_panel(Box::new(termide_panel_html::HtmlPanel::from_source(
                    title,
                    src,
                    Some(url),
                )));
            }
            Some(ViewKind::Markdown(src)) => {
                self.add_panel(Box::new(
                    termide_panel_markdown::MarkdownPanel::from_source(title, src, Some(url)),
                ));
            }
            Some(ViewKind::Image(bytes, ext)) => {
                self.open_fetched_image(&bytes, &ext, &url);
                return;
            }
            None => {
                self.show_error_modal(format!(
                    "Unsupported content type: {}",
                    fetched.content_type
                ));
                return;
            }
        }
        self.auto_save_session();
    }

    /// Cache fetched image bytes to a temp file and open them in the image
    /// preview (which handles graphics-protocol display or an external fallback).
    fn open_fetched_image(&mut self, bytes: &[u8], ext: &str, url: &str) {
        let title = fetch_title(url);
        let raw_stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
        let mut stem = sanitize_filename(raw_stem);
        if stem.is_empty() {
            stem = "image".to_string();
        }
        let path = std::env::temp_dir().join(format!("termide-web-{stem}.{ext}"));
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                self.close_help_panels();
                let _ = self.event_preview_media(path);
            }
            Err(e) => self.show_error_modal(format!("Failed to cache image: {e}")),
        }
    }

    /// Apply a navigation result to the active viewer in place when its type
    /// matches the content; otherwise fall back to opening a new viewer (so the
    /// browsing panel's history isn't clobbered by a type switch).
    fn apply_fetched_in_place(&mut self, fetched: termide_fetch::Fetched) {
        let title = fetch_title(&fetched.final_url);
        let url = fetched.final_url.clone();
        match classify(&fetched) {
            Some(ViewKind::Html(src)) => {
                if let Some(p) = self.layout_manager.active_panel_mut().and_then(|p| {
                    p.as_any_mut()
                        .downcast_mut::<termide_panel_html::HtmlPanel>()
                }) {
                    p.apply_fetched(title, src, url);
                    return;
                }
                self.add_panel(Box::new(termide_panel_html::HtmlPanel::from_source(
                    title,
                    src,
                    Some(url),
                )));
            }
            Some(ViewKind::Markdown(src)) => {
                if let Some(p) = self.layout_manager.active_panel_mut().and_then(|p| {
                    p.as_any_mut()
                        .downcast_mut::<termide_panel_markdown::MarkdownPanel>()
                }) {
                    p.apply_fetched(title, src, url);
                    return;
                }
                self.add_panel(Box::new(
                    termide_panel_markdown::MarkdownPanel::from_source(title, src, Some(url)),
                ));
            }
            Some(ViewKind::Image(bytes, ext)) => self.open_fetched_image(&bytes, &ext, &url),
            None => self.show_error_modal(format!(
                "Unsupported content type: {}",
                fetched.content_type
            )),
        }
    }
}

/// Which viewer a fetched document maps to, with its content prepared.
enum ViewKind {
    Html(String),
    Markdown(String),
    /// Raw image bytes plus a file extension for the image preview.
    Image(Vec<u8>, String),
}

/// Classify a fetched document by Content-Type. `None` for unsupported types.
fn classify(fetched: &termide_fetch::Fetched) -> Option<ViewKind> {
    let ct = fetched.content_type.as_str();
    if let Some(ext) = image_ext(ct) {
        return Some(ViewKind::Image(fetched.body.clone(), ext.to_string()));
    }
    match ct {
        "text/html" | "application/xhtml+xml" => Some(ViewKind::Html(fetched.text())),
        "text/markdown" | "text/x-markdown" => Some(ViewKind::Markdown(fetched.text())),
        ct if ct.starts_with("text/") || ct == "application/json" || ct == "application/xml" => {
            // Plain text → shown verbatim in the HTML viewer via <pre>.
            Some(ViewKind::Html(format!(
                "<pre>{}</pre>",
                escape_html(&fetched.text())
            )))
        }
        _ => None,
    }
}

/// File extension for an image Content-Type the image preview can show.
fn image_ext(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        _ => None,
    }
}

/// A short display title from a URL: its last path segment, else the host.
fn fetch_title(url: &str) -> String {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    no_scheme
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(no_scheme)
        .to_string()
}

/// Keep a filename stem to a safe, bounded set of characters for a temp path.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Minimal HTML text escaping for wrapping plain text in `<pre>`.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
