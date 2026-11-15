//! Panel navigation, focus, paste, and cursor-position event handlers.

use anyhow::Result;

use crate::app::App;
use crate::PanelExt;
use termide_core::PanelCommand;

impl App {
    /// Handle bracketed paste event - paste text directly to active panel
    pub fn handle_paste_event(&mut self, text: String) -> Result<()> {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            panel.handle_command(PanelCommand::PasteText { text });
        }
        Ok(())
    }

    /// Move the editor cursor to a 0-based line/column (status-bar Pos modal).
    pub(in crate::app) fn event_goto_position(&mut self, line: usize, column: usize) {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.goto_position(line, column);
            }
        }
    }

    /// Follow a link clicked in any panel, inside termide: an `http(s)`
    /// address fetched into a viewer (an `ftp://` one in the browser, as the
    /// viewer cannot fetch it); a local path as Open… opens one — an image in
    /// the image preview. [`Self::event_open_link_external`] is the outside twin.
    pub(super) fn event_open_link(&mut self, link: termide_core::LinkTarget) -> Result<()> {
        use termide_core::LinkTarget;
        match link {
            LinkTarget::Url(url) => {
                if url.starts_with("http://") || url.starts_with("https://") {
                    self.start_url_fetch(url);
                } else {
                    self.open_external_detached(&url);
                }
            }
            LinkTarget::Path(path) => {
                if !path.exists() {
                    self.show_error_modal(
                        termide_i18n::t().nav_no_such_path_fmt(&path.display().to_string()),
                    );
                    return Ok(());
                }
                self.open_local_path(path)?;
            }
        }
        Ok(())
    }

    /// Open a link outside termide: a web address in the browser, a local
    /// path in its system application.
    pub(super) fn event_open_link_external(
        &mut self,
        link: termide_core::LinkTarget,
    ) -> Result<()> {
        use termide_core::LinkTarget;
        match link {
            LinkTarget::Url(url) => self.open_external_detached(&url),
            LinkTarget::Path(path) => {
                if !path.exists() {
                    self.show_error_modal(
                        termide_i18n::t().nav_no_such_path_fmt(&path.display().to_string()),
                    );
                    return Ok(());
                }
                self.open_external_detached(&path.to_string_lossy());
            }
        }
        Ok(())
    }
}
