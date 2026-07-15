//! Link resolution, activation, history navigation, and "go to path".

use std::path::PathBuf;

use termide_core::{InputAction, LinkOpen, PanelEvent};

use crate::text::is_image_path;
use crate::HtmlPanel;

impl HtmlPanel {
    /// Resolve a link `href` to an absolute target: against the document URL for
    /// a fetched page, or against the file's directory for a file-backed view.
    pub(crate) fn resolve(&self, href: &str) -> String {
        if let Some(base) = &self.source_url {
            if let Ok(b) = url::Url::parse(base) {
                if let Ok(joined) = b.join(href) {
                    return joined.to_string();
                }
            }
            return href.to_string();
        }
        // File-backed: resolve a relative path against the file's directory.
        if href.contains("://") || std::path::Path::new(href).is_absolute() {
            return href.to_string();
        }
        if let Some(dir) = self.file_path.parent() {
            return dir.join(href).to_string_lossy().into_owned();
        }
        href.to_string()
    }

    /// Follow a link. Non-web targets go to the external opener. Web links
    /// honor the `open_links` setting: `External` → browser; `Panel` →
    /// in-place navigation in a fetched view, or a new viewer otherwise.
    pub(crate) fn activate_link(&mut self, href: &str) -> Vec<PanelEvent> {
        if href.is_empty() {
            return vec![];
        }
        // Same-page anchor: scroll to it (don't hand "#" to the system opener).
        if let Some(frag) = href.strip_prefix('#') {
            if !frag.is_empty() {
                self.scroll_to_anchor(frag);
            }
            return vec![PanelEvent::NeedsRedraw];
        }
        let target = self.resolve(href);
        let is_web = target.starts_with("http://") || target.starts_with("https://");
        let is_image = is_image_path(&target);
        // Images and pages each follow their own open-where setting; `O` is the
        // per-action external override (handled by the caller).
        let mode = if is_image {
            self.open_images
        } else {
            self.open_links
        };
        if mode == LinkOpen::External {
            return vec![PanelEvent::OpenExternal(PathBuf::from(target))];
        }
        if is_web {
            // Fetch and render in the viewer; image responses are routed to the
            // image preview by the fetch handler.
            if self.source_url.is_some() {
                self.history.truncate(self.hist_idx + 1);
                self.history.push(target.clone());
                self.hist_idx = self.history.len() - 1;
                vec![PanelEvent::NavigateUrl(target)]
            } else {
                vec![PanelEvent::OpenUrl(target)]
            }
        } else if is_image {
            // Local image → built-in image preview.
            vec![PanelEvent::PreviewMedia(PathBuf::from(target))]
        } else {
            vec![PanelEvent::OpenExternal(PathBuf::from(target))]
        }
    }

    /// Step back in history, re-fetching the previous page.
    pub(crate) fn go_back(&mut self) -> Vec<PanelEvent> {
        if self.hist_idx > 0 {
            self.hist_idx -= 1;
            return vec![PanelEvent::NavigateUrl(self.history[self.hist_idx].clone())];
        }
        vec![]
    }

    /// Step forward in history, re-fetching the next page.
    pub(crate) fn go_forward(&mut self) -> Vec<PanelEvent> {
        if self.hist_idx + 1 < self.history.len() {
            self.hist_idx += 1;
            return vec![PanelEvent::NavigateUrl(self.history[self.hist_idx].clone())];
        }
        vec![]
    }

    /// Build the "go to path" input request, seeded with this file's directory
    /// so relative entries resolve naturally.
    pub(crate) fn goto_path_event(&self) -> PanelEvent {
        let base = self
            .file_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let mut initial = base.display().to_string();
        if !initial.is_empty() {
            initial.push('/');
        }
        PanelEvent::ShowInput {
            prompt: "Go to path".to_string(),
            initial_value: initial,
            on_submit: InputAction::ViewPath { base_dir: base },
        }
    }
}
