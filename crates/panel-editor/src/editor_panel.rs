//! Panel trait implementation for Editor.
//!
//! This module is the integration point between the editor and the app's
//! panel framework: lifecycle hooks (prepare_render, render, tick),
//! input dispatch (handle_key, handle_mouse, handle_scroll), and the
//! cross-panel command bus (handle_command).

use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::{buffer::Buffer, layout::Rect};

use termide_config::Config;
use termide_core::{
    CommandResult, Panel, PanelCommand, PanelEvent, RenderContext, SegmentKind, SessionPanel,
    StatusSegment, WidthPreference,
};
use termide_i18n::t;
use termide_modal::{ActiveModal, InputModal};
use termide_state::PendingAction;
use termide_theme::Theme;

use crate::keyboard;

use super::{build_editor_hotkey_table, Editor};

impl Editor {
    /// Apply a config-defined keyword highlighter when this file's extension
    /// has no tree-sitter grammar (e.g. an in-development language). Tree-sitter
    /// grammars always win; a no-match clears any previous custom syntax.
    fn sync_custom_highlight(&mut self, config: &Config) {
        let title_path = std::path::Path::new(&self.file_state.title);
        let custom = if termide_highlight::detect_language(title_path).is_some() {
            None
        } else if let Some(ext) = title_path.extension().and_then(|e| e.to_str()) {
            config
                .highlight
                .custom_languages
                .iter()
                .find(|l| l.extensions.iter().any(|e| e == ext))
                .map(|l| {
                    termide_highlight::KeywordSyntax::new(
                        l.name.clone(),
                        l.line_comment.clone(),
                        l.block_comment.clone(),
                        l.keywords.clone(),
                        l.types.clone(),
                    )
                })
        } else {
            None
        };
        self.render_cache.highlight.set_custom_syntax(custom);
    }

    /// The swap event when this editor holds a previewable source file
    /// (Markdown or Mermaid). Used to make the view/edit toggle (Ctrl+E / Edit
    /// chip) swap to the rendered view instead of flipping read-only.
    fn preview_swap_event(&self) -> Option<PanelEvent> {
        let path = self.file_path()?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" => Some(PanelEvent::SwapActiveToMarkdown(path.to_path_buf())),
            "mmd" | "mermaid" => Some(PanelEvent::SwapActiveToMermaid(path.to_path_buf())),
            "html" | "htm" => Some(PanelEvent::SwapActiveToHtml(path.to_path_buf())),
            _ => None,
        }
    }
}

impl Panel for Editor {
    fn name(&self) -> &'static str {
        "editor"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        use termide_config::constants::spinner_frame;

        let modified = if self.buffer.is_modified() { "*" } else { "" };

        let external_change = if self.file_state.external_change_detected {
            " [changed on disk]"
        } else {
            ""
        };

        let search_info = if let Some(ref search) = self.search.state {
            if search.is_active() {
                let current = search.current_match.map(|i| i + 1).unwrap_or(0);
                let total = search.match_count();
                if total > 0 {
                    format!(" [{}]", t().editor_search_match_info(current, total))
                } else {
                    format!(" [{}]", t().editor_search_no_matches())
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Upload spinner (takes priority over LSP spinner)
        let upload_indicator = if self.file_state.uploading {
            format!("{} ", spinner_frame())
        } else {
            String::new()
        };

        // LSP loading spinner (only if not uploading)
        let lsp_indicator = if !self.file_state.uploading && self.lsp.server_loading {
            format!("{} ", spinner_frame())
        } else {
            String::new()
        };

        // LSP status text (shown after filename)
        let lsp_status = self
            .lsp
            .server_status_text
            .as_ref()
            .map(|s| format!(" ({})", s))
            .unwrap_or_default();

        format!(
            "{}{}{}{}{}{}{}",
            upload_indicator,
            lsp_indicator,
            self.file_state.title,
            modified,
            lsp_status,
            external_change,
            search_info
        )
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        self.render_cache.theme = *theme;
        self.render_cache.config = Arc::clone(config);

        // Sync EditorConfig with global Config.editor settings
        // This ensures runtime config changes are applied to the editor
        self.config.word_wrap = config.editor.word_wrap;
        // Per-editor tab_size override wins over the global config, so a user
        // picking a different size via the status bar survives the per-frame
        // resync.
        self.config.tab_size = self.tab_size_override.unwrap_or(config.editor.tab_size);
        self.config.auto_indent = config.editor.auto_indent;
        self.config.auto_close_brackets = config.editor.auto_close_brackets;
        self.git.blame_enabled = config.editor.show_blame;

        // Sync highlight cache with theme's light/dark mode and default foreground color
        self.render_cache
            .highlight
            .set_light_theme(theme.is_light_theme());
        self.render_cache.highlight.set_default_fg(theme.fg);

        let config_ptr = Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            self.hotkeys = build_editor_hotkey_table(config);
            self.sync_custom_highlight(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        // Use cached theme and config (updated by app layer before rendering)
        let theme = self.render_cache.theme;
        let config = self.render_cache.config.clone();

        // Dock the inline find/replace bar at the TOP (consistent with the file
        // manager), with a pseudographic separator, shrinking the content area
        // so the viewport/scroll/mouse math see the reduced height.
        let mut content_area = area;
        if let Some(mut bar) = self.find_bar.take() {
            let bar_h = bar.height().min(area.height);
            let bar_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: bar_h,
            };
            let active = !self.find_bar_focus_buffer;
            bar.render(bar_area, buf, &theme, active);
            self.find_bar = Some(bar);

            // Separator row below the bar.
            let sep_y = area.y + bar_h;
            let mut used = bar_h;
            if sep_y < area.y + area.height {
                let style = ratatui::style::Style::default().fg(theme.disabled);
                for dx in 0..area.width {
                    buf[(area.x + dx, sep_y)].set_symbol("─").set_style(style);
                }
                used += 1;
            }

            content_area = Rect {
                x: area.x,
                y: area.y + used,
                width: area.width,
                height: area.height.saturating_sub(used),
            };
        }

        self.render_content(
            content_area,
            buf,
            &theme,
            &config,
            ctx.is_focused,
            ctx.border_right_x,
        );

        // Language picker overlay (drawn last, over the content).
        if self.syntax_picker.is_some() {
            let colors = termide_core::ThemeColors::from(&theme);
            if let Some(picker) = self.syntax_picker.as_mut() {
                picker.render(area, buf, &colors);
            }
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;

        // The syntax-language picker owns input while open.
        if self.syntax_picker.is_some() {
            use crate::syntax_picker::PickerAction;
            match self.syntax_picker.as_mut().unwrap().handle_key(key) {
                PickerAction::Select(lang) => self.apply_syntax(&lang),
                PickerAction::Cancel => self.syntax_picker = None,
                PickerAction::None => {}
            }
            return vec![PanelEvent::NeedsRedraw];
        }

        // Any keyboard input should make viewport follow cursor again
        self.scroll_follows_cursor = true;

        // Close hover popups on any key press
        // For Escape, just close the popup and don't process further if one was open
        let had_popup = self.lsp.hover_popup.is_some()
            || self.lsp.completion_popup.is_some()
            || self.lsp.code_action_popup.is_some();
        self.close_hover_popup();

        if had_popup && key.code == crossterm::event::KeyCode::Esc {
            // Close completion / code-action popups if open
            if self.lsp.completion_popup.is_some() {
                self.cancel_completion();
            }
            if self.lsp.code_action_popup.is_some() {
                self.cancel_code_action();
            }
            // Just close the popup, don't trigger other Escape actions
            return vec![];
        }

        // Note: Key translation should be done at app level before calling handle_key
        // If you need translation, call translate_hotkey from termide-core or keyboard module

        // The viewer's hex/text toggle (configurable, default Ctrl+L) swaps this
        // editor in place for the hex viewer of the same file — the inverse of
        // the binary viewer's toggle. Blocked when the buffer has unsaved edits
        // so the swap can't discard them.
        if self.hotkeys.matches("viewer_toggle_hex", &key) {
            if let Some(path) = self.file_path().map(|p| p.to_path_buf()) {
                if self.buffer_is_modified() {
                    return vec![PanelEvent::ShowMessage(
                        "Save the file before switching to hex view".to_string(),
                    )];
                }
                return vec![PanelEvent::SwapActiveToHex(path)];
            }
            return vec![];
        }

        // Toggle view (read-only) ↔ edit. For Markdown the same key swaps to the
        // rendered preview instead (source ↔ preview is the view/edit axis).
        if self.hotkeys.matches("viewer_toggle_view", &key) {
            if let Some(ev) = self.preview_swap_event() {
                if self.buffer_is_modified() {
                    return vec![PanelEvent::ShowMessage(
                        "Save the file before switching to preview".to_string(),
                    )];
                }
                return vec![ev];
            }
            self.config.read_only = !self.config.read_only;
            return vec![PanelEvent::NeedsRedraw];
        }

        // The inline find/replace bar (before vim / command processing). Tab
        // toggles focus between the bar and the buffer "results" zone, like the
        // file manager. In the bar zone the bar owns keys; in the buffer zone
        // keys fall through to normal editor handling (cursor navigation, F3
        // stepping), with Esc closing the bar.
        if self.find_bar.is_some() {
            use crossterm::event::KeyCode;
            let plain = key.modifiers.is_empty();
            if plain && key.code == KeyCode::Tab {
                self.find_bar_focus_buffer = !self.find_bar_focus_buffer;
                return vec![PanelEvent::NeedsRedraw];
            }
            if self.find_bar_focus_buffer {
                if plain && key.code == KeyCode::Esc {
                    self.close_find_bar();
                    return vec![PanelEvent::NeedsRedraw];
                }
                // else fall through to normal buffer handling below
            } else {
                return self.handle_find_bar_key(key);
            }
        }

        // Collect events from internal state
        let mut events = Vec::new();

        // Handle Vim mode if enabled
        if let Some(ref mut vim_state) = self.vim {
            use crate::vim::{handle_vim_key, VimKeyResult};

            let result = handle_vim_key(vim_state, key);

            match result {
                VimKeyResult::Consumed => return events,
                VimKeyResult::PassThrough => {
                    // In insert mode, fall through to normal editor handling
                }
                VimKeyResult::Unhandled => {
                    // Key not recognized by vim in NORMAL/VISUAL mode - ignore it
                    return events;
                }
                _ => {
                    // Execute vim action
                    if let Some(panel_events) = self.execute_vim_result(result) {
                        events.extend(panel_events);
                    }
                    // Convert status_message to event
                    if let Some(message) = self.status_message.take() {
                        events.push(PanelEvent::SetStatusMessage {
                            message,
                            is_error: false,
                        });
                    }
                    return events;
                }
            }
        }

        let command = keyboard::EditorCommand::from_key_event(
            key,
            self.config.read_only,
            self.search.state.is_some(),
            self.selection.is_some(),
            self.lsp.completion_popup.is_some(),
            self.lsp.code_action_popup.is_some(),
            &self.hotkeys,
        );

        // Execute command and handle errors
        if let Err(e) = command.execute(self) {
            events.push(PanelEvent::SetStatusMessage {
                message: e.to_string(),
                is_error: true,
            });
        }

        // Convert status_message to event and take it (removes from legacy field)
        if let Some(message) = self.status_message.take() {
            events.push(PanelEvent::SetStatusMessage {
                message,
                is_error: false,
            });
        }

        events
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        // The language picker owns the mouse while open (scroll / click / click-away).
        if self.syntax_picker.is_some() {
            use crate::syntax_picker::PickerAction;
            match self.syntax_picker.as_mut().unwrap().handle_mouse(mouse) {
                PickerAction::Select(lang) => self.apply_syntax(&lang),
                PickerAction::Cancel => self.syntax_picker = None,
                PickerAction::None => {}
            }
            return vec![PanelEvent::NeedsRedraw];
        }
        self.handle_mouse_event(mouse, panel_area)
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        // The language picker owns the wheel while open (the host forwards
        // coalesced scroll via this path, not handle_mouse).
        if let Some(picker) = self.syntax_picker.as_mut() {
            picker.scroll(delta);
            return vec![PanelEvent::NeedsRedraw];
        }
        let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
        if delta < 0 {
            // Scroll up - check popups first
            if let Some(ref mut popup) = self.lsp.completion_popup {
                popup.scroll_up(lines);
                return vec![];
            }
            if let Some(ref mut popup) = self.lsp.hover_popup {
                popup.scroll_up(lines);
                return vec![];
            }
            // No popup - scroll editor by visual rows (accounts for word wrap)
            self.scroll_visual_rows_up(lines);
        } else {
            // Scroll down - check popups first
            if let Some(ref mut popup) = self.lsp.completion_popup {
                popup.scroll_down(lines);
                return vec![];
            }
            if let Some(ref mut popup) = self.lsp.hover_popup {
                popup.scroll_down(lines);
                return vec![];
            }
            // No popup - scroll editor by visual rows (accounts for word wrap)
            self.scroll_visual_rows_down(lines);
        }
        self.scroll_follows_cursor = false;
        vec![]
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        // Skip background work when panel is collapsed (stale)
        if self.is_stale {
            return vec![];
        }

        // Handle auto-scroll during selection drag
        if self.tick_auto_scroll() {
            return vec![PanelEvent::NeedsRedraw];
        }

        // Keep redrawing while spinner is animating (upload or LSP loading)
        if self.file_state.uploading || self.lsp.server_loading {
            return vec![PanelEvent::NeedsRedraw];
        }

        // Check if async blame data arrived
        if self.git.poll_blame() {
            return vec![PanelEvent::NeedsRedraw];
        }

        vec![]
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        let t = t();
        let info = self.get_editor_info();
        // Uniform `Label: value` fields in a stable order: mode toggles
        // (View/Edit) come first with a fixed width so toggling them never
        // shifts anything to their right; shared info follows; the
        // variable-width Pos sits last so cursor movement can't move a clicker.
        // Each field is a dimmed `Label:` plus its value; clickable values are
        // bold (Active) and carry the action on both segments so the whole
        // field is a click target, info values are plain (Value).
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        let clickable = |label: &str, value: String, action: &'static str| {
            [
                StatusSegment::clickable(format!("{label}: "), SegmentKind::Label, action),
                StatusSegment::clickable(value, SegmentKind::Active, action),
            ]
        };
        let info_field = |label: &str, value: String| {
            [
                StatusSegment::new(format!("{label}: "), SegmentKind::Label),
                StatusSegment::new(value, SegmentKind::Value),
            ]
        };

        let mut s = vec![StatusSegment::new(" ", SegmentKind::Label)];
        // View: Text — clicking swaps to the hex viewer.
        s.extend(clickable("View", "Text".to_string(), "to_hex"));
        s.push(sep());
        // Edit: Yes/No — read-only toggle; pad "No " so the width stays stable.
        let edit = if info.read_only { "No " } else { "Yes" };
        s.extend(clickable("Edit", edit.to_string(), "toggle_edit"));
        s.push(sep());
        let ftype = if info.syntax_highlighting {
            info.file_type.clone()
        } else {
            t.status_plain_text().to_string()
        };
        s.extend(clickable("Highlight", ftype, "pick_syntax"));
        s.push(sep());
        s.extend(clickable("Tab", info.tab_size.to_string(), "tab_size"));
        s.push(sep());
        s.extend(info_field("EOL", info.line_ending.clone()));
        s.push(sep());
        s.extend(info_field("Encoding", info.encoding.clone()));
        s.push(sep());
        s.extend(clickable(
            "Pos",
            format!("{}:{}", info.line, info.column),
            "goto_line",
        ));
        if let Some(m) = info.vim_mode {
            s.push(sep());
            s.push(StatusSegment::new(m.to_string(), SegmentKind::Warn));
        }
        s
    }

    fn context_menu_items(&self) -> Vec<(String, &'static str)> {
        vec![(t().menu_view_as_diagram().to_string(), "view_as_diagram")]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "goto_line" => {
                let modal = InputModal::with_default("Go to line", "", String::new());
                self.modal_request =
                    Some((PendingAction::GotoLine, ActiveModal::Input(Box::new(modal))));
                vec![]
            }
            "tab_size" => {
                let t = t();
                let modal = InputModal::with_default(
                    t.status_tab_modal_title(),
                    "",
                    self.config.tab_size.to_string(),
                );
                self.modal_request = Some((
                    PendingAction::ChangeEditorTabSize,
                    ActiveModal::Input(Box::new(modal)),
                ));
                vec![]
            }
            "pick_syntax" => {
                self.open_language_picker();
                vec![PanelEvent::NeedsRedraw]
            }
            "toggle_edit" => {
                if let Some(ev) = self.preview_swap_event() {
                    if self.buffer_is_modified() {
                        return vec![PanelEvent::ShowMessage(
                            "Save the file before switching to preview".to_string(),
                        )];
                    }
                    return vec![ev];
                }
                self.config.read_only = !self.config.read_only;
                vec![PanelEvent::NeedsRedraw]
            }
            "to_hex" => {
                if let Some(path) = self.file_path().map(|p| p.to_path_buf()) {
                    if self.buffer_is_modified() {
                        vec![PanelEvent::ShowMessage(
                            "Save the file before switching to hex view".to_string(),
                        )]
                    } else {
                        vec![PanelEvent::SwapActiveToHex(path)]
                    }
                } else {
                    vec![]
                }
            }
            "view_as_diagram" => {
                let source = self.buffer.text();
                let language = self
                    .render_cache
                    .highlight
                    .current_syntax()
                    .map(|s| s.to_string());
                let file_path = self.file_path().map(|p| p.to_path_buf());
                if let Some(mermaid) = crate::diagram::generate_class_diagram(
                    &source,
                    language.as_deref(),
                    file_path.as_deref(),
                ) {
                    let temp_path = std::env::temp_dir().join(format!(
                        "termide-diagram-{}-{}.mmd",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ));
                    if std::fs::write(&temp_path, &mermaid).is_ok() {
                        return vec![PanelEvent::SwapActiveToMermaid(temp_path)];
                    }
                }
                vec![PanelEvent::ShowMessage(
                    t().status_no_diagram_symbols().to_string(),
                )]
            }
            _ => vec![],
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::GetRepoRoot => {
                let repo_root = self.get_or_compute_repo_root().cloned();
                CommandResult::RepoRoot(repo_root)
            }
            PanelCommand::OnGitUpdate { repo_paths } => {
                if let Some(file_path) = self.file_path() {
                    // Check if any updated repo contains this file
                    if repo_paths.iter().any(|repo| file_path.starts_with(repo)) {
                        self.update_git_diff();
                        return CommandResult::NeedsRedraw(true);
                    }
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::CheckPendingGitDiff => {
                self.check_pending_git_diff_update();
                CommandResult::None
            }
            PanelCommand::CheckGitDiffReceiver => {
                let needs_redraw = self.check_git_diff_receiver();
                CommandResult::NeedsRedraw(needs_redraw)
            }
            PanelCommand::CheckExternalModification => {
                self.check_external_modification();
                CommandResult::None
            }
            PanelCommand::GetFsWatchInfo => {
                // For Editor, return file path info for watcher registration
                let file_path = self.file_path().map(|p| p.to_path_buf());
                if let Some(ref file_path) = file_path {
                    let repo_root = self.get_or_compute_repo_root().cloned();
                    let current_path = file_path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("/"));
                    CommandResult::FsWatchInfo {
                        watched_root: repo_root,
                        current_path,
                        is_git_repo: self
                            .git
                            .cached_repo_root
                            .as_ref()
                            .is_some_and(|r| r.is_some()),
                    }
                } else {
                    CommandResult::None
                }
            }
            PanelCommand::OnFsUpdate { changed_path } => {
                if let Some(file_path) = self.file_path() {
                    // Check for exact file match or directory containing the file
                    let file_changed =
                        changed_path == file_path || changed_path.parent() == file_path.parent();

                    if file_changed {
                        self.update_git_diff();
                        self.check_external_modification();
                        return CommandResult::NeedsRedraw(true);
                    }
                }
                CommandResult::NeedsRedraw(false)
            }
            PanelCommand::Reload => {
                if self.reload_from_disk().is_ok() {
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::NeedsRedraw(false)
                }
            }
            PanelCommand::GetModificationStatus => CommandResult::ModificationStatus {
                is_modified: self.buffer.is_modified(),
                has_external_change: self.file_state.external_change_detected,
            },
            PanelCommand::Save => match self.save() {
                Ok(None) => CommandResult::SaveResult {
                    success: true,
                    error: None,
                },
                Ok(Some(_remote_info)) => {
                    // PanelCommand::Save is only used for synchronous save probes
                    // (tests, internal callers). App-driven remote saves bypass this
                    // path and call `editor.save()` directly to retrieve the upload
                    // tuple and queue it via `queue_remote_editor_upload`.
                    // Report success so `SaveResult` remains a pure sync signal.
                    CommandResult::SaveResult {
                        success: true,
                        error: None,
                    }
                }
                Err(e) => CommandResult::SaveResult {
                    success: false,
                    error: Some(e.to_string()),
                },
            },
            PanelCommand::CloseWithoutSaving => {
                // Clear external change flag - the panel is being closed without saving
                self.file_state.external_change_detected = false;
                // Note: buffer.modified stays true but caller handles closing directly
                CommandResult::None
            }
            PanelCommand::MarkStale => {
                self.is_stale = true;
                CommandResult::None
            }
            PanelCommand::RefreshIfStale => {
                if self.is_stale {
                    self.is_stale = false;
                    self.check_external_modification();
                    self.update_git_diff();
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::None
                }
            }
            PanelCommand::Copy => {
                if self.selection.is_some() {
                    if let Err(e) = self.copy_to_clipboard() {
                        log::error!("Editor copy failed: {}", e);
                    }
                    CommandResult::Handled(true)
                } else {
                    CommandResult::Handled(false)
                }
            }
            PanelCommand::Cut => {
                if self.selection.is_some() {
                    if let Err(e) = self.cut_to_clipboard() {
                        log::error!("Editor cut failed: {}", e);
                    }
                    CommandResult::Handled(true)
                } else {
                    CommandResult::Handled(false)
                }
            }
            PanelCommand::Paste => {
                if let Err(e) = self.paste_from_clipboard() {
                    log::error!("Editor paste failed: {}", e);
                }
                CommandResult::Handled(true)
            }
            PanelCommand::PasteText { text } => {
                if let Err(e) = self.paste_text(&text) {
                    log::error!("Editor paste_text failed: {}", e);
                }
                CommandResult::None
            }
            // Commands not applicable to Editor
            PanelCommand::SetFsWatchRoot { .. }
            | PanelCommand::Resize { .. }
            | PanelCommand::SetHostFocus { .. }
            | PanelCommand::RefreshDirectory
            | PanelCommand::SetGitOperationInProgress { .. }
            | PanelCommand::UpdateRepoPaths { .. } => CommandResult::None,
        }
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        if self.buffer.is_modified() {
            Some("File has unsaved changes. Close anyway?".to_string())
        } else if self.file_state.external_change_detected {
            Some("File changed on disk. Close anyway?".to_string())
        } else {
            None
        }
    }

    fn captures_escape(&self) -> bool {
        // Capture Escape when the inline find bar is open, search is active,
        // popups are open, the language picker is open, or vim is in INSERT mode
        self.syntax_picker.is_some()
            || self.find_bar.is_some()
            || self.search.state.is_some()
            || self.lsp.completion_popup.is_some()
            || self.lsp.hover_popup.is_some()
            || self
                .vim
                .as_ref()
                .map(|v| v.mode.is_insert())
                .unwrap_or(false)
    }

    fn to_session(&self, session_dir: &std::path::Path) -> Option<SessionPanel> {
        if let Some(path) = self.file_path() {
            // Named file - save path
            Some(SessionPanel::Editor {
                path: Some(path.to_path_buf()),
                unsaved_buffer_file: None,
            })
        } else if self.buffer_is_modified() {
            // Unnamed buffer with unsaved content - save to session dir
            // ensure_unsaved_buffer_file() must be called before to_session()
            let filename = self.unsaved_buffer_file()?.to_string();

            let content = self.buffer.text();
            if content.trim().is_empty() {
                return None; // Don't save empty buffers
            }

            // Save content to session directory
            if let Err(e) = termide_session::save_unsaved_buffer(session_dir, &filename, &content) {
                log::warn!("Failed to save unsaved buffer: {}", e);
                return None;
            }

            Some(SessionPanel::Editor {
                path: None,
                unsaved_buffer_file: Some(filename),
            })
        } else {
            // Unnamed buffer without changes - don't save
            None
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.file_path()
            .and_then(|p| p.parent().map(|parent| parent.to_path_buf()))
    }
}
