//! Help panel showing keybindings reference.
//!
//! Renders help content directly without using Editor, providing:
//! - No line numbers (cleaner display)
//! - Full-width pseudo-graphic tables
//! - Simple scroll navigation

use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Widget},
};
use std::any::Any;

use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up, Config};
use termide_core::{
    CommandResult, Panel, PanelCommand, PanelEvent, RenderContext, WidthPreference,
};
use termide_i18n;
use termide_theme::Theme;
use termide_ui::ScrollBar;

use crate::help_generator::{HelpGenerator, HelpSection};

/// Help panel showing keybindings reference.
pub struct HelpPanel {
    /// Help sections data
    sections: Vec<HelpSection>,
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
    /// Current scroll offset (first visible line)
    scroll_offset: usize,
    /// Last rendered width (for cache invalidation)
    last_width: u16,
    /// Cached formatted lines
    cached_lines: Vec<Line<'static>>,
    /// Cached theme
    cached_theme: Theme,
    /// Cached vim_mode setting for keyboard handling
    vim_mode: bool,
}

impl HelpPanel {
    /// Create a new help panel with dynamic content from config.
    pub fn new(config: &Config) -> Self {
        let sections = HelpGenerator::generate(config);

        Self {
            sections,
            scrollbars: termide_core::ScrollBars::default(),
            scroll_offset: 0,
            last_width: 0,
            cached_lines: Vec::new(),
            cached_theme: Theme::default(),
            vim_mode: config.general.vim_mode,
        }
    }

    /// Regenerate cached lines if width changed.
    fn regenerate_if_needed(&mut self, width: u16) {
        if width != self.last_width {
            self.cached_lines =
                HelpGenerator::format_lines(&self.sections, width as usize, &self.cached_theme);
            self.last_width = width;
            // Clamp scroll offset after regeneration
            self.clamp_scroll(0);
        }
    }

    /// Clamp scroll offset to valid range.
    fn clamp_scroll(&mut self, visible_height: usize) {
        let max_scroll = self
            .cached_lines
            .len()
            .saturating_sub(visible_height.max(1));
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    /// Scroll up by given amount.
    fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll down by given amount.
    fn scroll_down(&mut self, amount: usize, visible_height: usize) {
        self.scroll_offset += amount;
        self.clamp_scroll(visible_height);
    }
}

impl Panel for HelpPanel {
    fn name(&self) -> &'static str {
        "help"
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.scroll_offset = offset.min(self.cached_lines.len().saturating_sub(1));
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        termide_i18n::t().panel_help().to_string()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        if self.cached_theme != *theme {
            self.cached_theme = *theme;
            self.last_width = 0; // Force regeneration
        }
        self.vim_mode = config.general.vim_mode;
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        // Regenerate lines if width changed
        self.regenerate_if_needed(area.width.saturating_sub(1)); // -1 for scrollbar

        let visible_height = area.height as usize;
        self.clamp_scroll(visible_height);

        // Get visible lines
        let visible_lines: Vec<Line> = self
            .cached_lines
            .iter()
            .skip(self.scroll_offset)
            .take(visible_height)
            .cloned()
            .collect();

        // Render content area (leaving space for scrollbar)
        let content_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(1),
            height: area.height,
        };

        let paragraph = Paragraph::new(visible_lines);
        paragraph.render(content_area, buf);

        // Render scrollbar on the right edge
        self.scrollbars.vertical = None;
        if ScrollBar::needs_scrollbar(visible_height, self.cached_lines.len()) {
            self.scrollbars.vertical = ScrollBar::render_tracked(
                buf,
                area.x + area.width - 1,
                area.y,
                area.height,
                self.scroll_offset,
                visible_height,
                self.cached_lines.len(),
                ctx.theme,
                ctx.is_focused,
            );
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        // Use a reasonable default for visible height
        let visible_height = 20;

        // Vim-aware navigation (j/k/g/G when vim_mode is enabled)
        if is_move_up(&key, self.vim_mode) {
            self.scroll_up(1);
            return vec![];
        }
        if is_move_down(&key, self.vim_mode) {
            self.scroll_down(1, visible_height);
            return vec![];
        }
        if is_go_home(&key, self.vim_mode) {
            self.scroll_offset = 0;
            return vec![];
        }
        if is_go_end(&key, self.vim_mode) {
            self.scroll_offset = self.cached_lines.len().saturating_sub(visible_height);
            return vec![];
        }

        match key.code {
            KeyCode::PageUp => {
                self.scroll_up(visible_height.saturating_sub(2));
            }
            KeyCode::PageDown => {
                self.scroll_down(visible_height.saturating_sub(2), visible_height);
            }
            _ => {}
        }
        vec![]
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Vec<PanelEvent> {
        let visible_height = area.height as usize;

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3, visible_height);
            }
            _ => {}
        }
        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, area: Rect) -> Vec<PanelEvent> {
        let lines = delta.unsigned_abs() as usize * 3; // 3 lines per scroll unit
        let visible_height = area.height as usize;

        if delta < 0 {
            self.scroll_up(lines);
        } else {
            self.scroll_down(lines, visible_height);
        }
        vec![]
    }

    fn captures_escape(&self) -> bool {
        // Help panel doesn't need to capture Escape
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn is_help_panel(&self) -> bool {
        true
    }
}
