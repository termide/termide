use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use super::editor::Editor;
use super::Panel;
use crate::state::AppState;

// Embed help files at compile time
const HELP_EN: &str = include_str!("../../help/en.txt");
const HELP_RU: &str = include_str!("../../help/ru.txt");

/// Get help text based on current locale from i18n system
fn get_help_text() -> &'static str {
    // Use the language from the already initialized i18n system
    match crate::i18n::current_language() {
        "ru" => HELP_RU,
        _ => HELP_EN,
    }
}

/// Welcome panel (shown when all panels are closed)
/// Uses Editor in read-only mode to display scrollable help
pub struct Welcome {
    editor: Editor,
}

impl Welcome {
    pub fn new() -> Self {
        let help_text = get_help_text();
        let title = crate::i18n::t().panel_welcome().to_string();
        let editor = Editor::from_text(help_text, title);

        Self { editor }
    }
}

impl Panel for Welcome {
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        panel_index: usize,
        state: &AppState,
    ) {
        // Delegate rendering to the embedded editor
        self.editor
            .render(area, buf, is_focused, panel_index, state);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Delegate key handling to the embedded editor
        self.editor.handle_key(key)
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Result<()> {
        // Delegate mouse handling to the embedded editor
        self.editor.handle_mouse(mouse, panel_area)
    }

    fn title(&self) -> String {
        crate::i18n::t().panel_welcome().to_string()
    }

    fn captures_escape(&self) -> bool {
        // Welcome panel doesn't need to capture Escape
        false
    }

    fn is_welcome_panel(&self) -> bool {
        true
    }
}

impl Default for Welcome {
    fn default() -> Self {
        Self::new()
    }
}
