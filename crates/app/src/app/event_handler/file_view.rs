//! File and viewer open/swap handlers for panel events.

#![allow(deprecated)]

use anyhow::Result;
use std::path::PathBuf;

use crate::app::App;
use crate::state::PendingAction;
use crate::PanelExt;
use termide_i18n as i18n;
use termide_panel_editor::Editor;

impl App {
    /// Handle OpenFile event - open file in editor (reuse existing tab if already open)
    pub(super) fn event_open_file(&mut self, file_path: PathBuf) -> Result<()> {
        self.close_help_panels();

        // Check if the file is already open — focus it instead of creating a duplicate
        if self.focus_editor_by_path(&file_path) {
            return Ok(());
        }

        let _ = self.open_editor_for_file(file_path);
        Ok(())
    }

    /// Handle ViewFile event - open file in read-only editor mode
    pub(in crate::app) fn event_view_file(&mut self, file_path: PathBuf) -> Result<()> {
        self.close_help_panels();
        let _ = self.open_editor_for_file_readonly(file_path);
        Ok(())
    }

    /// Handle OpenFileAt event - open file in editor at specific location (for go-to-definition)
    pub(super) fn event_open_file_at(
        &mut self,
        file_path: PathBuf,
        line: usize,
        column: usize,
    ) -> Result<()> {
        self.close_help_panels();
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        let t = i18n::t();

        // First check if the file is already open in an editor
        let mut found_existing = false;
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                if editor.file_path() == Some(&file_path) {
                    // File is already open - just move cursor to position
                    editor.goto_position(line, column);
                    found_existing = true;
                    break;
                }
            }
        }
        if found_existing {
            self.state
                .set_info(format!("{}:{}:{}", filename, line + 1, column));
            self.notify_outline_file_opened();
            return Ok(());
        }

        // File not open - open it and move to position
        match Editor::open_file_with_config(file_path.clone(), self.state.editor_config()) {
            Ok(mut editor_panel) => {
                // Move cursor to the requested position
                editor_panel.goto_position(line, column);

                // Initialize LSP for the editor
                if let Some(ref mut lsp_manager) = self.state.lsp_manager {
                    editor_panel.init_lsp(lsp_manager);
                }

                self.add_panel(Box::new(editor_panel));
                self.notify_outline_file_opened();
                self.auto_save_session();
                self.state.set_info(t.editor_file_opened(filename));
            }
            Err(e) => {
                let error_msg = t.status_error_open_file(filename, &e.to_string());
                log::error!("Error opening '{}': {}", filename, e);
                self.show_error_modal(error_msg);
            }
        }
        Ok(())
    }

    /// Handle ExecuteFile event - run executable in a new terminal
    pub(super) fn event_execute_file(&mut self, file_path: PathBuf) -> Result<()> {
        self.close_help_panels();

        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");

        // Working directory = directory containing the file
        let working_dir = file_path.parent().map(|p| p.to_path_buf());

        // Command to execute
        let command = file_path.to_string_lossy().into_owned();

        match self.create_terminal_panel(working_dir) {
            Ok(mut terminal) => {
                // Send command to execute the file
                let _ = terminal.send_command(&command);
                self.add_panel(Box::new(terminal));
                self.auto_save_session();
            }
            Err(e) => {
                log::error!("Failed to create terminal for '{}': {}", filename, e);
            }
        }
        Ok(())
    }

    /// Handle RunCommand event - run command in a new terminal
    pub(super) fn event_run_command(
        &mut self,
        command: String,
        cwd: Option<PathBuf>,
    ) -> Result<()> {
        self.close_help_panels();

        match self.create_terminal_panel(cwd) {
            Ok(mut terminal) => {
                let _ = terminal.send_command(&command);
                self.add_panel(Box::new(terminal));
                self.auto_save_session();
            }
            Err(e) => {
                log::error!("Failed to create terminal for command '{}': {}", command, e);
            }
        }
        Ok(())
    }

    /// Handle PreviewMedia event - preview image/video using native graphics or system viewer
    /// Swap the active panel in place for a hex viewer of the same file.
    pub(super) fn event_swap_active_to_hex(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_binary::BinaryPanel;
        // Carry the edit/view mode across the swap: leaving an editable text
        // editor lands in an editable hex editor (and vice versa), so the user
        // keeps editing instead of dropping silently into view-only.
        let editable = self
            .layout_manager
            .active_panel()
            .and_then(|p| p.as_editor())
            .map(|e| !e.get_editor_info().read_only)
            .unwrap_or(false);
        let opened = if editable {
            BinaryPanel::new_editable(file_path)
        } else {
            BinaryPanel::new(file_path)
        };
        match opened {
            Ok(panel) => {
                self.layout_manager.replace_active_panel(Box::new(panel));
                self.state.needs_redraw = true;
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open binary file: {e}")),
        }
        Ok(())
    }

    /// Swap the active panel in place for an editor of the same file, carrying
    /// the hex editor's edit/view mode over.
    pub(super) fn event_swap_active_to_text(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_binary::BinaryPanel;
        use termide_panel_editor::{Editor, EditorConfig};
        // Open editable when coming from an editable hex editor, or from the
        // markdown/mermaid preview (switching to source is always for editing).
        let from_preview = matches!(
            self.layout_manager.active_panel().map(|p| p.name()),
            Some("markdown") | Some("mermaid") | Some("html")
        );
        let editable = from_preview
            || self
                .layout_manager
                .active_panel()
                .and_then(|p| p.as_any().downcast_ref::<BinaryPanel>())
                .map(|b| b.is_editable())
                .unwrap_or(false);
        let config = if editable {
            EditorConfig::default()
        } else {
            EditorConfig::view_only()
        };
        match Editor::open_file_with_config(file_path, config) {
            Ok(mut editor) => {
                if let Some(ref mut lsp) = self.state.lsp_manager {
                    editor.init_lsp(lsp);
                }
                self.layout_manager.replace_active_panel(Box::new(editor));
                self.notify_outline_file_opened();
                self.state.needs_redraw = true;
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open file: {e}")),
        }
        Ok(())
    }

    /// Open a markdown file in the rendered preview panel (read-only).
    pub(in crate::app) fn event_view_markdown(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_markdown::MarkdownPanel;

        // Each open creates its own focused viewer. Reusing a viewer in place
        // (replacing the open file) is intentionally image-only — that exists
        // so an album can be flipped through from the file manager.
        self.close_help_panels();
        match MarkdownPanel::new(file_path) {
            Ok(panel) => {
                self.add_panel(Box::new(panel));
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open markdown file: {e}")),
        }
        Ok(())
    }

    /// Swap the active panel in place for the rendered markdown preview.
    pub(super) fn event_swap_active_to_markdown(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_markdown::MarkdownPanel;
        match MarkdownPanel::new(file_path) {
            Ok(panel) => {
                self.layout_manager.replace_active_panel(Box::new(panel));
                self.state.needs_redraw = true;
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open markdown file: {e}")),
        }
        Ok(())
    }

    /// Open a `.mmd` file in the Mermaid diagram viewer (read-only).
    pub(in crate::app) fn event_view_mermaid(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_mermaid::MermaidPanel;

        // Each open creates its own focused viewer; reuse-in-place is
        // image-only (see event_view_markdown).
        self.close_help_panels();
        match MermaidPanel::new(file_path) {
            Ok(panel) => {
                self.add_panel(Box::new(panel));
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open diagram file: {e}")),
        }
        Ok(())
    }

    /// Swap the active panel in place for the Mermaid diagram view.
    pub(super) fn event_swap_active_to_mermaid(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_mermaid::MermaidPanel;
        match MermaidPanel::new(file_path) {
            Ok(panel) => {
                self.layout_manager.replace_active_panel(Box::new(panel));
                self.state.needs_redraw = true;
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open diagram file: {e}")),
        }
        Ok(())
    }

    /// Open a "Save As" dialog to export in-memory `content` to a chosen path.
    /// Seeds the dialog with `default_name` under the current directory.
    pub(super) fn event_save_content_as(
        &mut self,
        content: String,
        default_name: String,
    ) -> Result<()> {
        use termide_modal::{ActiveModal, SaveAsModal};
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let default_value = directory.join(&default_name).display().to_string();
        let modal = SaveAsModal::new(i18n::t().modal_save_as_title(), default_value);
        let action = PendingAction::SaveContentAs { directory, content };
        self.handle_modal_request(action, ActiveModal::SaveAs(Box::new(modal)))?;
        Ok(())
    }

    /// Open an `.html` file in the rendered HTML viewer (read-only).
    pub(in crate::app) fn event_view_html(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_html::HtmlPanel;

        // Each open creates its own focused viewer; reuse-in-place is
        // image-only (see event_view_markdown).
        self.close_help_panels();
        match HtmlPanel::new(file_path) {
            Ok(panel) => {
                self.add_panel(Box::new(panel));
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open HTML file: {e}")),
        }
        Ok(())
    }

    /// Swap the active panel in place for the rendered HTML preview.
    pub(super) fn event_swap_active_to_html(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_html::HtmlPanel;
        match HtmlPanel::new(file_path) {
            Ok(panel) => {
                self.layout_manager.replace_active_panel(Box::new(panel));
                self.state.needs_redraw = true;
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open HTML file: {e}")),
        }
        Ok(())
    }

    pub(super) fn event_view_binary(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_binary::BinaryPanel;

        // Reuse an existing binary viewer if one is open, focusing it like
        // opening a text viewer focuses its panel.
        if let Some(panel) = self.layout_manager.focus_and_expand_panel_by_name("binary") {
            if let Some(bin) = panel.as_any_mut().downcast_mut::<BinaryPanel>() {
                bin.set_file(file_path);
                self.state.needs_redraw = true;
                return Ok(());
            }
        }

        self.close_help_panels();
        match BinaryPanel::new(file_path) {
            Ok(panel) => {
                self.add_panel(Box::new(panel));
                self.auto_save_session();
            }
            Err(e) => {
                self.show_error_modal(format!("Failed to open binary file: {e}"));
            }
        }
        Ok(())
    }

    /// Open a binary file in the hex editor (editable). Always a fresh panel so
    /// the editable state is unambiguous.
    pub(super) fn event_edit_binary(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_binary::BinaryPanel;
        self.close_help_panels();
        match BinaryPanel::new_editable(file_path) {
            Ok(panel) => {
                self.add_panel(Box::new(panel));
                self.auto_save_session();
            }
            Err(e) => self.show_error_modal(format!("Failed to open binary file: {e}")),
        }
        Ok(())
    }

    pub(in crate::app) fn event_preview_media(&mut self, file_path: PathBuf) -> Result<()> {
        use termide_panel_image::ImagePanel;

        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        // Check if file is an image by extension
        let is_image = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                matches!(
                    ext.to_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif"
                )
            })
            .unwrap_or(false);

        // Try native graphics rendering for images if protocol is available
        if is_image && ImagePanel::graphics_available() {
            // Try to reuse existing ImagePanel
            if let Some(panel) = self.layout_manager.find_and_expand_panel_by_name("image") {
                if let Some(image_panel) = panel.as_any_mut().downcast_mut::<ImagePanel>() {
                    image_panel.set_image(file_path);
                    self.state.needs_redraw = true;
                    return Ok(());
                }
            }

            // No existing panel - create new one without changing focus
            self.close_help_panels();
            match ImagePanel::new(file_path.clone()) {
                Ok(panel) => {
                    self.add_panel_without_focus(Box::new(panel));
                    self.auto_save_session();
                    return Ok(());
                }
                Err(e) => {
                    let _ = e; // Native graphics unavailable, fall through to xdg-open
                }
            }
        }

        // Fallback to system default viewer (xdg-open)
        if let Err(e) = open::that(&file_path) {
            log::error!("Failed to open '{}': {}", filename, e);
            self.show_error_modal(format!("Failed to open {}: {}", filename, e));
        }
        Ok(())
    }

    /// Handle OpenExternal event - open file with system default application
    pub(super) fn event_open_external(&mut self, file_path: PathBuf) -> Result<()> {
        let t = termide_i18n::t();
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        // Show status message
        self.state.set_info(t.status_opening_external(&filename));

        if let Err(e) = open::that(&file_path) {
            log::error!("Failed to open '{}': {}", filename, e);
            self.show_error_modal(format!("Failed to open {}: {}", filename, e));
        }
        Ok(())
    }

    /// Handle OpenRemoteFile event - open remote file via VFS
    pub(super) fn event_open_remote_file(&mut self, url: String) -> Result<()> {
        self.close_help_panels();

        // Parse URL to VfsPath
        let vfs_path = match termide_vfs::parse_vfs_url(&url) {
            Ok(path) => path,
            Err(e) => {
                let error_msg = format!("Invalid remote URL: {}", e);
                log::error!("{}", error_msg);
                self.show_error_modal(error_msg);
                return Ok(());
            }
        };

        let filename = vfs_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("remote")
            .to_string();

        // Get VfsManager from active FileManager panel
        let vfs_manager = if let Some(panel) = self.layout_manager.active_panel() {
            if let Some(fm) = panel
                .as_any()
                .downcast_ref::<termide_panel_file_manager::FileManager>()
            {
                fm.vfs_state().manager_arc()
            } else {
                let error_msg = "No file manager panel available for remote file access";
                log::error!("{}", error_msg);
                self.show_error_modal(error_msg.to_string());
                return Ok(());
            }
        } else {
            let error_msg = "No active panel";
            log::error!("{}", error_msg);
            self.show_error_modal(error_msg.to_string());
            return Ok(());
        };

        // Create temp directory for remote files
        let temp_dir = std::env::temp_dir().join("termide-remote-edit");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            let error_msg = format!("Failed to create temp directory: {}", e);
            log::error!("{}", error_msg);
            self.show_error_modal(error_msg);
            return Ok(());
        }

        // Generate unique temp file name
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let temp_path = temp_dir.join(format!("{}_{}", timestamp, filename));

        // Create download request via OperationManager
        let request =
            termide_file_ops::OperationRequest::download(vfs_path.clone(), temp_path.clone());

        // Start download via OperationManager (no modal)
        match self.state.start_operation_now(request, vfs_manager.clone()) {
            Ok(operation_id) => {
                // Track the operation in the operations panel
                self.state.track_operation(
                    operation_id,
                    crate::state::OperationType::CopyDownload,
                    vfs_path.to_url_string(),
                    temp_path.display().to_string(),
                    1,
                    0,
                );

                // Store pending editor download metadata for post-processing
                self.state.pending_editor_download = Some(crate::state::PendingEditorDownload {
                    operation_id,
                    remote_path: vfs_path,
                    temp_path,
                    config: self.state.editor_config(),
                    vfs_manager,
                });

                // Open operations panel to show progress
                self.open_operations_panel()?;
            }
            Err(e) => {
                let error_msg = format!("Failed to start download: {}", e);
                log::error!("{}", error_msg);
                self.show_error_modal(error_msg);
            }
        }

        Ok(())
    }

    /// Handle SaveFile event - save file at given path
    pub(super) fn event_save_file(&mut self, path: PathBuf) -> Result<()> {
        // Store info needed for LSP notification (before mutable borrow)
        let mut lsp_info: Option<(String, std::path::PathBuf)> = None;

        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                match editor.save_file_as(path.clone()) {
                    Ok(()) => {
                        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                        self.state.set_info(format!("Saved: {}", filename));

                        // Collect LSP info for didSave notification
                        if let Some(lang) = editor.lsp_language() {
                            lsp_info = Some((lang.to_string(), path.clone()));
                        }
                    }
                    Err(e) => {
                        log::error!("Save failed: {}", e);
                        self.show_error_modal(format!("Save failed: {}", e));
                    }
                }
            }
        }

        // Send LSP didSave notification (triggers full analysis for semantic errors)
        if let Some((lang, file_path)) = lsp_info {
            if let Some(ref lsp_manager) = self.state.lsp_manager {
                lsp_manager.did_save(&lang, &file_path, None);
            }
        }

        Ok(())
    }
}
