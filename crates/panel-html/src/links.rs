//! Link resolution, activation, history navigation, and "go to path".

use std::path::PathBuf;

use termide_core::{LinkOpen, LinkTarget, PanelEvent};

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

    /// Follow a link. A same-page `#anchor` scrolls; inside a fetched page a
    /// web link opened in the panel replaces the page in place, with history.
    /// Anything else goes to the app as [`PanelEvent::OpenLink`], which opens
    /// a link the same way from every panel (`open_links` and `open_images`
    /// decide between a panel and the system opener); a scheme it does not
    /// know (`mailto:`) goes to the system opener. `O` is the per-action
    /// external override (handled by the caller).
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
        if termide_core::links::is_foreign_scheme(href) {
            return vec![PanelEvent::OpenExternal(PathBuf::from(href))];
        }
        let target = self.resolve(href);
        let is_web = target.starts_with("http://") || target.starts_with("https://");
        if is_web && self.source_url.is_some() && self.open_links == LinkOpen::Panel {
            self.history.truncate(self.hist_idx + 1);
            self.history.push(target.clone());
            self.hist_idx = self.history.len() - 1;
            return vec![PanelEvent::NavigateUrl(target)];
        }
        let base = self.file_path.parent().unwrap_or(std::path::Path::new("/"));
        match LinkTarget::from_href(&target, base) {
            Some(link) => vec![PanelEvent::OpenLink(link)],
            None => vec![PanelEvent::OpenExternal(PathBuf::from(target))],
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
}
