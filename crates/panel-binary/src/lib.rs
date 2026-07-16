//! Binary file viewer/editor panel (hex/ASCII).
//!
//! Renders a binary file as a classic hex dump — `offset │ hex bytes │ ASCII
//! gutter` — in pure text pseudographics. The number of bytes per row adapts to
//! the panel width in 16-byte sections, so a wide panel shows 32/48/… bytes per
//! row. The file is read in windows on demand so large files are not loaded
//! fully into memory.
//!
//! A byte cursor is shown in **both** the hex and ASCII zones at once (the
//! active zone is highlighted more strongly); `Tab` switches the active zone.
//! Shift+movement extends a selection and `Ctrl+C` copies it — as a hex string
//! when the cursor is in the hex zone, as text when it is in the ASCII zone.
//!
//! `Ctrl+L` (or the status-bar chip) toggles hex ↔ text. A real text file swaps
//! in place for a read-only editor (and the editor's `Ctrl+L` swaps back to
//! hex); a binary file — which the editor can't open as text — toggles an
//! in-panel lossy text view instead. `Ctrl+F` searches an ASCII substring or a
//! hex byte sequence, highlighting matches in both zones.
//!
//! Opened with `F4`, the panel is editable: typing overwrites the byte under
//! the cursor (two hex nibbles in the hex zone, a character in the ASCII zone)
//! without changing the file length. `Ctrl+S` saves after a confirmation,
//! backing the original up to `<file>.bak` first.

use std::any::Any;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use termide_core::{
    CommandResult, Config, HotkeyTable, KeyChord, Panel, PanelCommand, PanelEvent, RenderContext,
    SegmentKind, SessionPanel, StatusSegment, Theme, ThemeColors, WidthPreference,
};
use termide_modal::FindBar;
use termide_ui::ScrollBar;

mod edit;
mod io;
mod navigation;
mod render;
mod search;

/// Which column zone the cursor edits/navigates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Hex,
    Ascii,
}

/// In-panel rendering mode. `Text` is a lossy plain-text view used for binary
/// files (a real text file swaps to the editor instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Hex,
    Text,
}

/// Binary (hex/ASCII) viewer and overwrite editor.
pub struct BinaryPanel {
    /// Path to the file.
    file_path: PathBuf,
    /// Display title (filename).
    title: String,
    /// Open handle for windowed reads; `None` if the file could not be opened.
    file: Option<File>,
    /// File length in bytes.
    len: u64,
    /// Error message if the file could not be opened.
    error: Option<String>,
    /// Byte offset of the first visible row (kept aligned to the row size).
    top_byte: u64,
    /// Cursor byte index (`0..len`).
    cursor: u64,
    /// Selection anchor; `Some` while a selection is active.
    anchor: Option<u64>,
    /// Active column zone (hex or ASCII).
    zone: Zone,
    /// In-panel rendering mode (hex dump or lossy text, for binary files).
    mode: ViewMode,
    /// Whether the file is open for editing (overwrite-in-place).
    editable: bool,
    /// Pending overwrites by absolute offset (applied on render, written on save).
    edits: std::collections::BTreeMap<u64, u8>,
    /// High nibble typed in the hex zone, awaiting the low nibble.
    pending_nibble: Option<u8>,
    /// Byte where the current mouse drag started (anchor for drag-selection).
    drag_from: Option<u64>,
    /// Inline find bar (ASCII / hex byte search), when open.
    find_bar: Option<FindBar>,
    /// Match start offsets from the last search.
    matches: Vec<u64>,
    /// Byte length of each search match (the needle length).
    match_len: usize,
    /// Index of the current match within `matches`.
    match_idx: usize,
    /// Last render area (absolute) for click mapping + paging.
    last_area: Rect,
    /// Cached theme colors.
    theme: ThemeColors,
    /// Full theme, cached for rendering the find bar.
    theme_full: Option<Theme>,
    /// Configurable hotkeys (toggle hex/text).
    hotkeys: HotkeyTable,
    /// Pointer of the last `Arc<Config>` used to build hotkeys.
    last_config_ptr: usize,
    /// Whether the panel is focused (set each render); the byte cursor is only
    /// drawn when focused, so an unfocused viewer shows no cursor.
    focused: bool,
}

impl BinaryPanel {
    /// Open a binary file in the hex viewer.
    pub fn new(path: PathBuf) -> Result<Self> {
        let mut panel = Self {
            file_path: path.clone(),
            title: String::new(),
            file: None,
            len: 0,
            error: None,
            top_byte: 0,
            cursor: 0,
            anchor: None,
            zone: Zone::Hex,
            mode: ViewMode::Hex,
            editable: false,
            edits: std::collections::BTreeMap::new(),
            pending_nibble: None,
            drag_from: None,
            find_bar: None,
            matches: Vec::new(),
            match_len: 0,
            match_idx: 0,
            last_area: Rect::default(),
            theme: ThemeColors::default(),
            theme_full: None,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            focused: false,
        };
        panel.set_file(path);
        Ok(panel)
    }

    /// Open a binary file for editing (overwrite-in-place).
    pub fn new_editable(path: PathBuf) -> Result<Self> {
        let mut panel = Self::new(path)?;
        panel.editable = true;
        Ok(panel)
    }

    /// Whether the buffer has unsaved overwrites.
    pub fn is_modified(&self) -> bool {
        !self.edits.is_empty()
    }

    /// Whether the panel is open for editing (vs. read-only viewing). Used to
    /// carry the edit/view mode across hex↔text swaps.
    pub fn is_editable(&self) -> bool {
        self.editable
    }

    /// Point the panel at a file (also used to reuse an existing viewer).
    pub fn set_file(&mut self, path: PathBuf) {
        self.title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("binary")
            .to_string();
        match File::open(&path) {
            Ok(f) => {
                self.len = f.metadata().map(|m| m.len()).unwrap_or(0);
                self.file = Some(f);
                self.error = None;
            }
            Err(e) => {
                self.file = None;
                self.len = 0;
                self.error = Some(format!("Cannot open file: {e}"));
            }
        }
        self.file_path = path;
        self.top_byte = 0;
        self.cursor = 0;
        self.anchor = None;
        self.edits.clear();
        self.pending_nibble = None;
    }

    /// Toggle hex ↔ text. A real text file swaps in place for the editor; a
    /// binary file (which the editor can't open) toggles an in-panel lossy
    /// text view instead.
    fn toggle_view(&mut self) -> Vec<PanelEvent> {
        if !termide_core::util::is_binary_file(&self.file_path) {
            // Swapping to the text editor replaces this panel, so block it while
            // there are unsaved hex edits (mirrors the editor's hex toggle).
            if self.editable && self.is_modified() {
                return vec![PanelEvent::ShowMessage(
                    "Save the file before switching to text view".to_string(),
                )];
            }
            return vec![PanelEvent::SwapActiveToText(self.file_path.clone())];
        }
        // In-panel hex ↔ text view of a binary file: same buffer, edits are
        // applied in both modes, so nothing is lost.
        self.mode = match self.mode {
            ViewMode::Hex => ViewMode::Text,
            ViewMode::Text => ViewMode::Hex,
        };
        self.clamp_top();
        vec![PanelEvent::NeedsRedraw]
    }

    /// Write pending edits to disk after backing the original up to `<file>.bak`.
    pub fn save(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        if self.edits.is_empty() {
            return Ok(());
        }
        let mut bak = self.file_path.clone().into_os_string();
        bak.push(".bak");
        std::fs::copy(&self.file_path, PathBuf::from(bak))?;

        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.file_path)?;
        for (&off, &b) in &self.edits {
            f.seek(SeekFrom::Start(off))?;
            f.write_all(&[b])?;
        }
        f.flush()?;

        self.edits.clear();
        self.pending_nibble = None;
        // Refresh the read handle so reads reflect the saved bytes.
        self.file = File::open(&self.file_path).ok();
        Ok(())
    }
}

impl Panel for BinaryPanel {
    fn name(&self) -> &'static str {
        "binary"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        if self.editable && self.is_modified() {
            format!("{}*", self.title)
        } else {
            self.title.clone()
        }
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        self.theme = ThemeColors::from(theme);
        self.theme_full = Some(*theme);
        let ptr = Arc::as_ptr(config) as usize;
        if self.last_config_ptr != ptr {
            self.last_config_ptr = ptr;
            let mut t = HotkeyTable::new();
            t.insert("toggle_hex", &config.viewer.keybindings.toggle_hex);
            t.insert("toggle_view", &config.viewer.keybindings.toggle_view);
            self.hotkeys = t;
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.focused = ctx.is_focused;

        // Find bar docked at the TOP with a separator below, matching the
        // editor / file manager.
        let mut hex_area = area;
        if let (Some(bar), Some(theme)) = (self.find_bar.as_mut(), self.theme_full.as_ref()) {
            let bar_h = bar.height().min(area.height);
            let bar_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: bar_h,
            };
            bar.render(bar_area, buf, theme, true);

            let mut used = bar_h;
            let sep_y = area.y + bar_h;
            if sep_y < area.y + area.height {
                let style = Style::default().fg(self.theme.disabled);
                for dx in 0..area.width {
                    buf[(area.x + dx, sep_y)].set_symbol("─").set_style(style);
                }
                used += 1;
            }
            hex_area = Rect {
                x: area.x,
                y: area.y + used,
                width: area.width,
                height: area.height.saturating_sub(used),
            };
        }
        self.last_area = hex_area;

        if let Some(ref err) = self.error {
            buf.set_string(
                hex_area.x,
                hex_area.y,
                err,
                Style::default().fg(self.theme.error),
            );
        } else if self.len == 0 {
            buf.set_string(
                hex_area.x,
                hex_area.y,
                "(empty file)",
                Style::default().fg(self.theme.disabled),
            );
        } else if hex_area.height > 0 {
            self.clamp_top();

            let bpr = self.cols();
            let visible_rows = hex_area.height as u64;
            let start = self.top_byte;
            let want = (visible_rows * bpr).min(self.len - start) as usize;
            let window = self.read_window(start, want);

            for row in 0..visible_rows {
                let row_start = (row * bpr) as usize;
                if row_start >= window.len() {
                    break;
                }
                let row_end = (row_start + bpr as usize).min(window.len());
                let bytes = &window[row_start..row_end];
                let off = start + row * bpr;
                let line = match self.mode {
                    ViewMode::Hex => self.hex_row(off, bytes, bpr),
                    ViewMode::Text => self.text_row(off, bytes),
                };
                buf.set_line(hex_area.x, hex_area.y + row as u16, &line, hex_area.width);
            }

            // Scroll progress on the right border, like the other panels.
            let total_rows = self.len.div_ceil(bpr) as usize;
            let viewport = hex_area.height as usize;
            if let Some(border_x) = ctx.border_right_x {
                if ScrollBar::needs_scrollbar(viewport, total_rows) {
                    ScrollBar::render(
                        buf,
                        border_x,
                        hex_area.y,
                        hex_area.height,
                        (self.top_byte / bpr) as usize,
                        viewport,
                        total_rows,
                        &self.theme,
                        ctx.is_focused,
                    );
                }
            }
        }
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        if self.error.is_some() {
            return vec![];
        }
        // Same uniform `Label: value` layout and styling as the text editor:
        // dimmed labels, plain info values, bold clickable values.
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

        let mut segs = vec![StatusSegment::new(" ", SegmentKind::Label)];
        // View: Hex/Text — clicking toggles the representation.
        let view = match self.mode {
            ViewMode::Hex => "Hex ",
            ViewMode::Text => "Text",
        };
        segs.extend(clickable("View", view.to_string(), "toggle"));
        segs.push(sep());
        // Edit: Yes/No — clicking flips editability; `*` marks unsaved edits.
        let edit = if self.editable {
            if self.is_modified() {
                "Yes*"
            } else {
                "Yes "
            }
        } else {
            "No  "
        };
        segs.extend(clickable("Edit", edit.to_string(), "toggle_edit"));
        segs.push(sep());
        segs.extend(info_field(
            "Off",
            format!("{:#X} / {:#X}", self.cursor, self.len),
        ));
        if let Some(a) = self.anchor {
            let n = a.max(self.cursor) - a.min(self.cursor) + 1;
            segs.push(sep());
            segs.extend(info_field("Sel", n.to_string()));
        }
        segs
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "toggle" => self.toggle_view(),
            "toggle_edit" => {
                self.editable = !self.editable;
                vec![PanelEvent::NeedsRedraw]
            }
            _ => vec![],
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            // Report like the editor so the shared close-with-unsaved dialog
            // (Save / Don't save / Cancel) is reused for the hex editor too.
            PanelCommand::GetModificationStatus => CommandResult::ModificationStatus {
                is_modified: self.editable && self.is_modified(),
                has_external_change: false,
            },
            // Copy is routed from the global keybinding to the focused panel.
            // Copy the selection if one exists; otherwise let the key fall
            // through. Cut/Paste are inapplicable to this read-only viewer.
            PanelCommand::Copy => {
                if self.anchor.is_some() {
                    self.copy_selection();
                    CommandResult::Handled(true)
                } else {
                    CommandResult::Handled(false)
                }
            }
            PanelCommand::Cut => CommandResult::Handled(false),
            PanelCommand::Paste => CommandResult::Handled(false),
            _ => CommandResult::None,
        }
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;

        // While the find bar is open it owns input (Esc / Ctrl+F close it).
        if self.find_bar.is_some() {
            if key.code == KeyCode::Char('f') && key.modifiers == KeyModifiers::CONTROL {
                self.close_find();
                return vec![PanelEvent::NeedsRedraw];
            }
            let action = self.find_bar.as_mut().unwrap().handle_key(key);
            return match action {
                Some(a) => self.handle_find_action(a),
                None => vec![PanelEvent::NeedsRedraw],
            };
        }
        if key.code == KeyCode::Char('f') && key.modifiers == KeyModifiers::CONTROL {
            self.open_find();
            return vec![PanelEvent::NeedsRedraw];
        }

        if self.hotkeys.matches("toggle_hex", &key) {
            return self.toggle_view();
        }
        // Edit/view toggle (Ctrl+E), mirroring the text editor's status chip.
        if self.hotkeys.matches("toggle_view", &key) {
            self.editable = !self.editable;
            return vec![PanelEvent::NeedsRedraw];
        }
        // Ctrl+R: re-read the file from disk (pick up external changes), keeping
        // the cursor. Skipped while there are unsaved edits so they aren't lost.
        if key.code == KeyCode::Char('r') && key.modifiers == KeyModifiers::CONTROL {
            if !self.is_modified() {
                let cursor = self.cursor;
                self.set_file(self.file_path.clone());
                self.cursor = cursor.min(self.len.saturating_sub(1));
                self.ensure_cursor_visible();
            }
            return vec![PanelEvent::NeedsRedraw];
        }

        // Edit mode: Ctrl+S asks to save; typed hex digits / chars overwrite
        // (handled before navigation so letters aren't treated as motions).
        if self.editable {
            if key.code == KeyCode::Char('s') && key.modifiers == KeyModifiers::CONTROL {
                if self.is_modified() {
                    let name = self.title.clone();
                    return vec![PanelEvent::ShowConfirm {
                        message: format!("Save {name}? A .bak backup will be created."),
                        on_confirm: termide_core::ConfirmAction::SaveBinary,
                    }];
                }
                return vec![];
            }
            if self.try_edit(key) {
                return vec![PanelEvent::NeedsRedraw];
            }
        }

        let cols = self.cols() as i64;
        let page = ((self.last_area.height as i64 - 1).max(1)) * cols;
        let extend = key.modifiers.contains(KeyModifiers::SHIFT);
        let ro = !self.editable; // vim-letter motions only when not editing
        match key.code {
            KeyCode::Tab => {
                self.zone = match self.zone {
                    Zone::Hex => Zone::Ascii,
                    Zone::Ascii => Zone::Hex,
                };
            }
            KeyCode::Left => self.move_cursor(-1, extend),
            KeyCode::Right => self.move_cursor(1, extend),
            KeyCode::Up => self.move_cursor(-cols, extend),
            KeyCode::Down => self.move_cursor(cols, extend),
            KeyCode::PageUp => self.move_cursor(-page, extend),
            KeyCode::PageDown => self.move_cursor(page, extend),
            KeyCode::Home => self.set_cursor(self.cursor - self.cursor % cols as u64, extend),
            KeyCode::End => {
                let row_end = self.cursor - self.cursor % cols as u64 + cols as u64 - 1;
                self.set_cursor(row_end, extend)
            }
            KeyCode::Char('h') if ro => self.move_cursor(-1, extend),
            KeyCode::Char('l') if ro => self.move_cursor(1, extend),
            KeyCode::Char('k') if ro => self.move_cursor(-cols, extend),
            KeyCode::Char('j') if ro => self.move_cursor(cols, extend),
            KeyCode::Char('g') if ro => self.set_cursor(0, extend),
            KeyCode::Char('G') if ro => self.set_cursor(self.len.saturating_sub(1), extend),
            KeyCode::Char('q') if ro => return vec![PanelEvent::ClosePanel],
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        self.scroll_rows(delta as i64);
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll_rows(-1),
            MouseEventKind::ScrollDown => self.scroll_rows(1),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((byte, zone)) = self.byte_at_event(&event) {
                    self.zone = zone;
                    self.set_cursor(byte, false); // clears any selection
                    self.drag_from = Some(byte);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(from) = self.drag_from {
                    if let Some((byte, zone)) = self.byte_at_event(&event) {
                        self.zone = zone;
                        self.anchor = Some(from);
                        self.cursor = byte;
                        self.ensure_cursor_visible();
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag_from = None,
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn captures_escape(&self) -> bool {
        self.find_bar.is_some()
    }

    fn needs_close_confirmation(&self) -> Option<String> {
        if self.editable && self.is_modified() {
            Some("File has unsaved changes. Close anyway?".to_string())
        } else {
            None
        }
    }

    fn to_session(&self, _session_dir: &Path) -> Option<SessionPanel> {
        Some(SessionPanel::Binary {
            path: self.file_path.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.file_path.parent().map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_with(len: u64, w: u16, h: u16) -> BinaryPanel {
        let mut p = BinaryPanel::new(PathBuf::from("/dev/null")).unwrap();
        p.len = len;
        p.last_area = Rect::new(0, 0, w, h);
        p
    }

    #[test]
    fn toggle_view_swaps_text_file_to_editor() {
        // /dev/null reads as empty → treated as text → swaps to the editor.
        let mut p = panel_with(10, 80, 10);
        assert!(matches!(
            p.toggle_view().as_slice(),
            [PanelEvent::SwapActiveToText(_)]
        ));
    }

    #[test]
    fn toggle_edit_action_flips_editability() {
        let mut p = panel_with(10, 80, 10);
        assert!(!p.is_editable());
        p.handle_status_action("toggle_edit");
        assert!(p.is_editable(), "clicking Edit enables editing");
        p.handle_status_action("toggle_edit");
        assert!(!p.is_editable(), "clicking Edit again returns to view-only");
    }
}
