//! Link resolution, activation, in-panel history, and anchor scrolling.

use std::path::PathBuf;

use termide_core::{LinkTarget, PanelEvent};

use crate::MarkdownPanel;

impl MarkdownPanel {
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

    /// Follow a link inside termide (`Enter`, a click). A same-page `#anchor`
    /// scrolls; inside a fetched page a web link replaces the page in place,
    /// with history. Anything else goes to the app as
    /// [`PanelEvent::OpenLink`], which opens a link the same way from every
    /// panel; a scheme it does not know (`mailto:`) goes to the system
    /// opener. [`Self::activate_link_external`] is the outside twin.
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
        if is_web && self.source_url.is_some() {
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

    /// Open a link outside termide (`O`/`Alt+Enter`, `Alt+Click`): a web
    /// address in the browser, a local file in its system application. A
    /// same-page `#anchor` of a file-backed view has nowhere outside to go,
    /// so it scrolls as a followed one does.
    pub(crate) fn activate_link_external(&mut self, href: &str) -> Vec<PanelEvent> {
        if href.is_empty() {
            return vec![];
        }
        if href.starts_with('#') && self.source_url.is_none() {
            return self.activate_link(href);
        }
        if termide_core::links::is_foreign_scheme(href) {
            return vec![PanelEvent::OpenExternal(PathBuf::from(href))];
        }
        let target = self.resolve(href);
        let base = self.file_path.parent().unwrap_or(std::path::Path::new("/"));
        match LinkTarget::from_href(&target, base) {
            Some(link) => vec![PanelEvent::OpenLinkExternal(link)],
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

    /// Scroll so the named anchor's line is at the top of the view. No-op if
    /// the anchor is unknown.
    pub(crate) fn scroll_to_anchor(&mut self, frag: &str) {
        if let Some(&(_, line)) = self.doc.anchors.iter().find(|(id, _)| id == frag) {
            self.top = line.min(self.max_top());
            self.cursor = (line, 0);
            self.anchor = None;
        }
    }
}
