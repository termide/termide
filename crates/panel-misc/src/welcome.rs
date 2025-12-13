use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};
use std::any::Any;

use termide_config::Config;
use termide_core::{Panel, PanelEvent, RenderContext};
use termide_i18n;
use termide_panel_editor::Editor;
use termide_theme::Theme;

// Embed help files at compile time
const HELP_EN: &str = include_str!("../../../help/en.txt");
const HELP_RU: &str = include_str!("../../../help/ru.txt");

/// Get help text based on current locale from i18n system
fn get_help_text() -> &'static str {
    // Use the language from the already initialized i18n system
    match termide_i18n::current_language() {
        "ru" => HELP_RU,
        _ => HELP_EN,
    }
}

/// Welcome panel (shown when all panels are closed)
/// Uses Editor in read-only mode to display scrollable help
pub struct WelcomePanel {
    editor: Editor,
}

impl WelcomePanel {
    pub fn new() -> Self {
        let help_text = get_help_text();
        let title = termide_i18n::t().panel_welcome().to_string();
        let editor = Editor::from_text(help_text, title);

        Self { editor }
    }
}

impl Panel for WelcomePanel {
    fn name(&self) -> &'static str {
        "welcome"
    }

    fn title(&self) -> String {
        termide_i18n::t().panel_welcome().to_string()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Config) {
        self.editor.prepare_render(theme, config);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        // Delegate rendering to the embedded editor
        self.editor.render(area, buf, ctx);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent> {
        // Delegate key handling to the embedded editor
        self.editor.handle_key(key)
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        // Delegate mouse handling to the embedded editor
        self.editor.handle_mouse(mouse, panel_area)
    }

    fn captures_escape(&self) -> bool {
        // Welcome panel doesn't need to capture Escape
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn is_welcome_panel(&self) -> bool {
        true
    }
}

impl Default for WelcomePanel {
    fn default() -> Self {
        Self::new()
    }
}
