//! Viewer URL fetching (a viewer's `Ctrl+G` or a followed link): spawns the
//! blocking GET on a worker thread and hands the result to the viewer waiting
//! for it, in the viewer that matches its Content-Type. Includes the URL/HTML
//! helper functions.
//!
//! The viewer exists while the fetch runs, its title spinning: `Ctrl+G` opens
//! a placeholder HTML viewer at once, and a followed link keeps its page until
//! the new one arrives. Each fetch has an id its viewer waits under, so a
//! result finds its viewer wherever focus went, and one whose viewer was
//! closed is dropped.

use std::sync::mpsc::TryRecvError;

use termide_panel_html::HtmlPanel;
use termide_panel_markdown::MarkdownPanel;

use super::App;
use crate::state::ViewFetch;

/// What delivering a result leaves for the app to do once the waiting
/// viewer is no longer borrowed.
enum FollowUp {
    Done,
    ErrorModal(String),
    /// The document belongs in another kind of viewer than the waiting one.
    OpenNew(ViewKind, String),
    /// Image bytes, for the image preview.
    OpenImage(Vec<u8>, String, String),
    /// The placeholder became an image preview: save the layout.
    SaveLayout,
}

impl App {
    /// Start a background fetch of `url` from a viewer's `Ctrl+G`, opening a
    /// *new* viewer that shows the page once it arrives.
    pub(super) fn start_url_fetch(&mut self, url: String) {
        let id = self.spawn_view_fetch(url.clone());
        self.add_panel(Box::new(HtmlPanel::loading(id, url)));
    }

    /// Start a background fetch that replaces the *active* viewer's page in
    /// place (a followed link or a history step inside a fetched page).
    pub(super) fn start_url_fetch_in_place(&mut self, url: String) {
        let id = self.state.next_view_fetch_id;
        let waiting = match self.layout_manager.active_panel_mut() {
            Some(panel) => {
                let any = panel.as_any_mut();
                if let Some(html) = any.downcast_mut::<HtmlPanel>() {
                    html.start_loading(id, url.clone());
                    true
                } else if let Some(md) = any.downcast_mut::<MarkdownPanel>() {
                    md.start_loading(id, url.clone());
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if waiting {
            self.spawn_view_fetch(url);
        } else {
            self.start_url_fetch(url);
        }
    }

    /// Spawn the blocking GET on a worker thread under a new id; the result is
    /// picked up by [`check_view_fetch`](App::check_view_fetch).
    fn spawn_view_fetch(&mut self, url: String) -> u64 {
        let id = self.state.next_view_fetch_id;
        self.state.next_view_fetch_id += 1;
        let (tx, receiver) = std::sync::mpsc::channel();
        self.state.view_fetches.push(ViewFetch { id, receiver });
        std::thread::spawn(move || {
            let _ = tx.send(termide_fetch::fetch(&url));
        });
        self.state.needs_redraw = true;
        id
    }

    /// Poll the in-flight URL fetches and deliver the finished ones.
    pub(super) fn check_view_fetch(&mut self) {
        if self.state.view_fetches.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        self.state
            .view_fetches
            .retain(|fetch| match fetch.receiver.try_recv() {
                Ok(result) => {
                    finished.push((fetch.id, result));
                    false
                }
                Err(TryRecvError::Empty) => true,
                Err(TryRecvError::Disconnected) => {
                    finished.push((fetch.id, Err("the request was interrupted".to_string())));
                    false
                }
            });
        for (id, result) in finished {
            self.deliver_view_fetch(id, result);
            self.state.needs_redraw = true;
        }
    }

    /// Hand fetch `id`'s result to the viewer waiting for it, if still open.
    fn deliver_view_fetch(&mut self, id: u64, result: Result<termide_fetch::Fetched, String>) {
        let Some(slot) = self
            .layout_manager
            .iter_all_panels_mut()
            .find(|panel| waits_for(panel.as_any(), id))
        else {
            return;
        };
        match deliver(slot, result) {
            FollowUp::Done => {}
            FollowUp::ErrorModal(message) => self.show_error_modal(message),
            FollowUp::OpenNew(kind, url) => {
                let title = fetch_title(&url);
                match kind {
                    ViewKind::Html(src) => {
                        self.add_panel(Box::new(HtmlPanel::from_source(title, src, Some(url))));
                    }
                    ViewKind::Markdown(src) => {
                        self.add_panel(Box::new(MarkdownPanel::from_source(title, src, Some(url))));
                    }
                    ViewKind::Image(bytes, ext) => self.open_fetched_image(&bytes, &ext, &url),
                }
            }
            FollowUp::OpenImage(bytes, ext, url) => self.open_fetched_image(&bytes, &ext, &url),
            FollowUp::SaveLayout => self.auto_save_layout(),
        }
    }

    /// Cache fetched image bytes to a temp file and open them in the image
    /// preview (which handles graphics-protocol display or an external fallback).
    fn open_fetched_image(&mut self, bytes: &[u8], ext: &str, url: &str) {
        match cache_image(bytes, ext, url) {
            Ok(path) => {
                self.close_help_panels();
                let _ = self.event_preview_media(path);
            }
            Err(e) => self.show_error_modal(format!("Failed to cache image: {e}")),
        }
    }
}

/// Whether `panel` is a viewer waiting for fetch `id`.
fn waits_for(panel: &dyn std::any::Any, id: u64) -> bool {
    let waiting = if let Some(html) = panel.downcast_ref::<HtmlPanel>() {
        html.loading_id()
    } else if let Some(md) = panel.downcast_ref::<MarkdownPanel>() {
        md.loading_id()
    } else {
        None
    };
    waiting == Some(id)
}

/// Put a fetch result into the viewer `slot` waiting for it: into its page
/// when the kinds match, in place of a placeholder that has none, else into
/// what the app opens next.
fn deliver(
    slot: &mut Box<dyn termide_core::Panel>,
    result: Result<termide_fetch::Fetched, String>,
) -> FollowUp {
    let fetched = match result {
        Ok(fetched) => fetched,
        Err(e) => return fail(slot, format!("Fetch failed: {e}")),
    };
    let url = fetched.final_url.clone();
    let title = fetch_title(&url);
    let Some(kind) = classify(&fetched) else {
        return fail(
            slot,
            format!("Unsupported content type: {}", fetched.content_type),
        );
    };
    let any = slot.as_any_mut();
    if let Some(html) = any.downcast_mut::<HtmlPanel>() {
        let placeholder = html.is_placeholder();
        match kind {
            ViewKind::Html(src) => {
                html.apply_fetched(title, src, url);
                FollowUp::Done
            }
            ViewKind::Markdown(src) if placeholder => {
                *slot = Box::new(MarkdownPanel::from_source(title, src, Some(url)));
                FollowUp::Done
            }
            ViewKind::Image(bytes, ext) if placeholder => {
                image_into_placeholder(slot, &bytes, &ext, url)
            }
            ViewKind::Image(bytes, ext) => {
                html.stop_loading();
                FollowUp::OpenImage(bytes, ext, url)
            }
            kind => {
                html.stop_loading();
                FollowUp::OpenNew(kind, url)
            }
        }
    } else if let Some(md) = any.downcast_mut::<MarkdownPanel>() {
        match kind {
            ViewKind::Markdown(src) => {
                md.apply_fetched(title, src, url);
                FollowUp::Done
            }
            ViewKind::Image(bytes, ext) => {
                md.stop_loading();
                FollowUp::OpenImage(bytes, ext, url)
            }
            kind => {
                md.stop_loading();
                FollowUp::OpenNew(kind, url)
            }
        }
    } else {
        FollowUp::Done
    }
}

/// A failed fetch: a placeholder shows `message` in place of its page, a
/// viewer that has a page keeps it and the app shows the message.
fn fail(slot: &mut Box<dyn termide_core::Panel>, message: String) -> FollowUp {
    let any = slot.as_any_mut();
    if let Some(html) = any.downcast_mut::<HtmlPanel>() {
        let placeholder = html.is_placeholder();
        html.fail_loading(message.clone());
        if placeholder {
            return FollowUp::Done;
        }
    } else if let Some(md) = any.downcast_mut::<MarkdownPanel>() {
        md.stop_loading();
    }
    FollowUp::ErrorModal(message)
}

/// An image fetched into a placeholder: the image preview takes its place
/// where the terminal can draw graphics; elsewhere the image opens in the
/// system viewer and the placeholder keeps a clickable pictogram of it.
fn image_into_placeholder(
    slot: &mut Box<dyn termide_core::Panel>,
    bytes: &[u8],
    ext: &str,
    url: String,
) -> FollowUp {
    use termide_panel_image::ImagePanel;

    let title = fetch_title(&url);
    let path = match cache_image(bytes, ext, &url) {
        Ok(path) => path,
        Err(e) => return fail(slot, format!("Failed to cache image: {e}")),
    };
    if ImagePanel::graphics_available() {
        if let Ok(panel) = ImagePanel::new(path.clone()) {
            *slot = Box::new(panel);
            return FollowUp::SaveLayout;
        }
    }
    let page = format!(
        "<p><img src=\"{}\" alt=\"{}\"></p>",
        escape_html(&url).replace('"', "&quot;"),
        escape_html(&title).replace('"', "&quot;"),
    );
    if let Some(html) = slot.downcast_mut::<HtmlPanel>() {
        html.apply_fetched(title, page, url);
    }
    match open::that(&path) {
        Ok(()) => FollowUp::Done,
        Err(e) => FollowUp::ErrorModal(format!("Failed to open {}: {e}", path.display())),
    }
}

/// Write fetched image bytes to a temp file named after the URL.
fn cache_image(bytes: &[u8], ext: &str, url: &str) -> std::io::Result<std::path::PathBuf> {
    let title = fetch_title(url);
    let raw_stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
    let mut stem = sanitize_filename(raw_stem);
    if stem.is_empty() {
        stem = "image".to_string();
    }
    let path = std::env::temp_dir().join(format!("termide-web-{stem}.{ext}"));
    std::fs::write(&path, bytes)?;
    Ok(path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use termide_core::Panel;

    fn fetched(url: &str, content_type: &str, body: &str) -> termide_fetch::Fetched {
        termide_fetch::Fetched {
            final_url: url.to_string(),
            content_type: content_type.to_string(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn placeholder(id: u64) -> Box<dyn Panel> {
        Box::new(HtmlPanel::loading(id, "https://ex.com/a".to_string()))
    }

    #[test]
    fn a_result_finds_the_viewer_waiting_under_its_id() {
        let slot = placeholder(7);
        assert!(waits_for(slot.as_any(), 7));
        assert!(!waits_for(slot.as_any(), 8));
    }

    #[test]
    fn an_html_page_fills_the_placeholder() {
        let mut slot = placeholder(1);
        let follow = deliver(
            &mut slot,
            Ok(fetched("https://ex.com/a", "text/html", "<p>hi</p>")),
        );
        assert!(matches!(follow, FollowUp::Done));
        let html = slot.downcast_ref::<HtmlPanel>().unwrap();
        assert_eq!(html.loading_id(), None);
        assert!(!html.is_placeholder());
        assert_eq!(slot.title(), "https://ex.com/a");
    }

    #[test]
    fn markdown_takes_the_placeholder_place() {
        let mut slot = placeholder(1);
        let follow = deliver(
            &mut slot,
            Ok(fetched("https://ex.com/r.md", "text/markdown", "# r")),
        );
        assert!(matches!(follow, FollowUp::Done));
        assert!(slot.downcast_ref::<MarkdownPanel>().is_some());
    }

    #[test]
    fn a_failure_shows_in_the_placeholder_but_keeps_a_page() {
        let mut slot = placeholder(1);
        assert!(matches!(
            deliver(&mut slot, Err("timed out".to_string())),
            FollowUp::Done
        ));
        assert_eq!(slot.downcast_ref::<HtmlPanel>().unwrap().loading_id(), None);

        let mut page = HtmlPanel::from_source(
            "a".into(),
            "<p>old</p>".into(),
            Some("https://ex.com/a".into()),
        );
        page.start_loading(2, "https://ex.com/b".into());
        let mut slot: Box<dyn Panel> = Box::new(page);
        assert!(matches!(
            deliver(&mut slot, Err("timed out".to_string())),
            FollowUp::ErrorModal(m) if m == "Fetch failed: timed out"
        ));
        let html = slot.downcast_ref::<HtmlPanel>().unwrap();
        assert_eq!(html.loading_id(), None);
        assert_eq!(slot.title(), "https://ex.com/a");
    }

    #[test]
    fn another_kind_from_a_page_opens_beside_it() {
        let mut page = HtmlPanel::from_source(
            "a".into(),
            "<p>old</p>".into(),
            Some("https://ex.com/a".into()),
        );
        page.start_loading(3, "https://ex.com/r.md".into());
        let mut slot: Box<dyn Panel> = Box::new(page);
        let follow = deliver(
            &mut slot,
            Ok(fetched("https://ex.com/r.md", "text/markdown", "# r")),
        );
        assert!(matches!(
            follow,
            FollowUp::OpenNew(ViewKind::Markdown(_), _)
        ));
        assert!(slot.downcast_ref::<HtmlPanel>().is_some());
    }
}
