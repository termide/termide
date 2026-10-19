//! Breakpoints of the editor's file.
//!
//! The buffer holds them as line marks, so they move with the text as it is
//! edited; the process-wide store in `termide_debug` holds them for every
//! file, for a debug launch and across restarts. An editor loads its file's
//! breakpoints from the store when it opens or reloads the file and writes
//! them back after every input that changed them.

use std::collections::BTreeSet;

use super::Editor;

impl Editor {
    /// Lines (0-based) with a breakpoint.
    pub fn breakpoint_lines(&self) -> &BTreeSet<usize> {
        self.buffer.line_marks()
    }

    /// Set a breakpoint on the cursor line, or remove the one there. An
    /// editor without a file (help, a new buffer) takes none.
    pub fn toggle_breakpoint(&mut self) -> bool {
        self.toggle_breakpoint_at(self.cursor.line)
    }

    /// Set a breakpoint on `line` (0-based), or remove the one there.
    /// Returns whether the line has one now.
    pub fn toggle_breakpoint_at(&mut self, line: usize) -> bool {
        if self.buffer.file_path().is_none() {
            return false;
        }
        let set = self.buffer.toggle_line_mark(line);
        self.sync_breakpoints();
        set
    }

    /// Put the cursor on `line` (0-based), scrolling only as far as needed
    /// to show it: the debugger stepping through the file moves the view
    /// with it but does not jump it about.
    pub fn reveal_line(&mut self, line: usize) {
        let line = line.min(self.buffer.line_count().saturating_sub(1));
        self.cursor = termide_buffer::Cursor::at(line, 0);
        self.selection = None;
        self.scroll_follows_cursor = true;
    }

    /// Take this file's breakpoints from the store.
    pub fn reload_breakpoints(&mut self) {
        let Some(path) = self.buffer.file_path() else {
            return;
        };
        let lines = termide_debug::breakpoints::store().lines(path);
        self.buffer.set_line_marks(lines);
        self.breakpoints_synced = self.buffer.line_marks_version();
    }

    /// Write this file's breakpoints to the store when an edit or a toggle
    /// changed them since the last write.
    pub fn sync_breakpoints(&mut self) {
        let version = self.buffer.line_marks_version();
        if version == self.breakpoints_synced {
            return;
        }
        self.breakpoints_synced = version;
        if let Some(path) = self.buffer.file_path() {
            termide_debug::breakpoints::store()
                .set_lines(path, self.buffer.line_marks().iter().copied());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_core::{KeyChord, Panel, PanelEvent};

    use super::super::build_editor_hotkey_table;
    use crate::{Editor, EditorConfig};

    fn editor(text: &str) -> (Editor, tempfile::NamedTempFile) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{text}").unwrap();
        let mut e =
            Editor::open_file_with_config(file.path().to_path_buf(), EditorConfig::default())
                .unwrap();
        let mut config = termide_config::Config::default();
        config.normalize();
        e.hotkeys = build_editor_hotkey_table(&config);
        (e, file)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
        KeyChord::identity(KeyEvent::new(code, modifiers))
    }

    fn stored(path: &std::path::Path) -> Vec<usize> {
        termide_debug::breakpoints::store()
            .lines(path)
            .into_iter()
            .collect()
    }

    #[test]
    fn ctrl_f8_toggles_a_breakpoint_that_follows_edits_into_the_store() {
        let (mut e, file) = editor("a\nb\nc\n");
        e.cursor.line = 1;
        e.handle_key(key(KeyCode::F(8), KeyModifiers::CONTROL));
        assert_eq!(stored(file.path()), [1]);

        // A line typed above moves it down, in the store too.
        e.cursor = termide_buffer::Cursor::at(0, 0);
        e.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(stored(file.path()), [2]);

        e.cursor.line = 2;
        e.handle_key(key(KeyCode::F(8), KeyModifiers::CONTROL));
        assert!(stored(file.path()).is_empty());
    }

    #[test]
    fn a_reopened_file_gets_its_breakpoints_back() {
        let (mut e, file) = editor("a\nb\nc\n");
        e.toggle_breakpoint_at(2);
        let again =
            Editor::open_file_with_config(file.path().to_path_buf(), EditorConfig::default())
                .unwrap();
        assert!(again.breakpoint_lines().contains(&2));
    }

    #[test]
    fn f5_asks_to_debug_this_file() {
        let (mut e, file) = editor("print(1)\n");
        let events = e.handle_key(key(KeyCode::F(5), KeyModifiers::NONE));
        assert!(matches!(
            events.as_slice(),
            [PanelEvent::StartDebug { file: Some(f) }] if f == file.path()
        ));
    }

    #[test]
    fn the_panel_menu_starts_debugging_and_removes_the_breakpoints() {
        let (mut e, file) = editor("print(1)\n");
        let actions: Vec<String> = Panel::context_menu_items(&e)
            .into_iter()
            .map(|(_, action)| action)
            .collect();
        assert!(
            actions.iter().any(|a| a == crate::START_DEBUG_ACTION),
            "{actions:?}"
        );
        assert!(
            actions.iter().any(|a| a == crate::CLEAR_BREAKPOINTS_ACTION),
            "{actions:?}"
        );

        let events = Panel::handle_status_action(&mut e, crate::START_DEBUG_ACTION);
        assert!(matches!(
            events.as_slice(),
            [PanelEvent::StartDebug { file: Some(f) }] if f == file.path()
        ));
        assert!(
            actions.iter().any(|a| a == crate::CONFIGURE_DEBUG_ACTION),
            "{actions:?}"
        );
        let events = Panel::handle_status_action(&mut e, crate::CONFIGURE_DEBUG_ACTION);
        assert!(matches!(
            events.as_slice(),
            [PanelEvent::ConfigureDebug { file: Some(f) }] if f == file.path()
        ));
        let events = Panel::handle_status_action(&mut e, crate::CLEAR_BREAKPOINTS_ACTION);
        assert!(matches!(events.as_slice(), [PanelEvent::ClearBreakpoints]));
    }

    #[test]
    fn a_click_on_the_line_numbers_toggles_a_breakpoint() {
        let (mut e, file) = editor("a\nb\nc\n");
        let colors = termide_core::ThemeColors::default();
        let ctx = termide_core::RenderContext {
            theme: &colors,
            config: &termide_core::PanelConfig {
                tab_size: 4,
                word_wrap: false,
                show_line_numbers: true,
                show_hidden_files: false,
            },
            is_focused: true,
            panel_index: 0,
            terminal_width: 40,
            terminal_height: 10,
            border_right_x: None,
            border_bottom_y: None,
        };
        let content = Rect::new(1, 1, 38, 8);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        Panel::render(&mut e, content, &mut buf, &ctx);

        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        Panel::handle_mouse(&mut e, click, Rect::new(0, 0, 40, 10));
        assert_eq!(stored(file.path()), [1]);
        assert_eq!(e.cursor.line, 0, "the click does not move the cursor");

        // The marker is drawn in the gutter of that line.
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        Panel::render(&mut e, content, &mut buf, &ctx);
        let row: String = (1..5).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row.contains('●'), "{row:?}");
    }
}
