//! The help window: every key termide binds, opened on the section of the
//! panel it was called from, narrowed by what is typed.
//!
//! It confirms with the section at the top of its view, for its owner to
//! open the help panel there and keep it beside the work.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use termide_config::constants::MODAL_MAX_WIDTH_PERCENTAGE_WIDE;
use termide_config::{Config, KeyBinding};
use termide_core::ThemeColors;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::scrollbar::ScrollBar;

use crate::base::render_modal_block;
use crate::help_content::{HelpGenerator, HelpLines, HelpSection};
use crate::{centered_rect_with_size, is_click_outside, Modal, ModalResult};

/// The widest the window grows: the help's lines are short, and wider only
/// spreads them apart.
const MAX_WIDTH: u16 = 120;

/// Rows of the frame beside the body: the border, the filter and a blank
/// line above, a blank line, the hint and the border below.
const CHROME: u16 = 6;

/// The help window.
#[derive(Debug)]
pub struct HelpModal {
    sections: Vec<HelpSection>,
    filter: String,
    /// The section to open at, placed by the first frame.
    start: Option<usize>,
    scroll: usize,
    /// The binding that opens help, which closes it again.
    toggle: Option<KeyBinding>,
    /// The body as the last frame laid it out.
    shown: Option<HelpLines>,
    visible: usize,
    last_area: Option<Rect>,
}

impl HelpModal {
    /// Help for `config`'s keys, opened on the section of the panel named
    /// `panel` when it has one.
    #[must_use]
    pub fn new(config: &Config, panel: Option<&str>) -> Self {
        let sections = HelpGenerator::generate(config);
        let start = panel.and_then(|name| HelpGenerator::section_for_panel(&sections, name));
        Self {
            sections,
            filter: String::new(),
            start,
            scroll: 0,
            toggle: config.general.keybindings.open_help.clone(),
            shown: None,
            visible: 0,
            last_area: None,
        }
    }

    /// The section at the top of the view.
    fn current_section(&self) -> usize {
        self.shown
            .as_ref()
            .and_then(|shown| shown.section_at(self.scroll))
            .or(self.start)
            .unwrap_or(0)
    }

    fn max_scroll(&self) -> usize {
        self.shown
            .as_ref()
            .map_or(0, |shown| shown.lines.len().saturating_sub(self.visible))
    }

    fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
    }

    /// Bring the next section's header (or the previous one's) to the top.
    fn step_section(&mut self, forward: bool) {
        let Some(shown) = &self.shown else {
            return;
        };
        let target = if forward {
            // Past the header of the section in view, which may sit below
            // the top (the first one does, under the version line).
            let current = shown
                .section_at(self.scroll)
                .and_then(|s| shown.line_of(s))
                .unwrap_or(0)
                .max(self.scroll);
            shown
                .anchors
                .iter()
                .map(|(_, line)| *line)
                .find(|line| *line > current)
        } else {
            shown
                .anchors
                .iter()
                .map(|(_, line)| *line)
                .rfind(|line| *line < self.scroll)
        };
        if let Some(line) = target {
            self.scroll = line.min(self.max_scroll());
        }
    }

    fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.scroll = 0;
        self.start = None;
    }

    fn render_filter(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let prompt = "› ";
        buf.set_string(
            area.x + 1,
            area.y,
            prompt,
            Style::default().fg(theme.accented_fg),
        );
        let x = area.x + 1 + prompt.chars().count() as u16;
        let room = (area.x + area.width).saturating_sub(x + 1) as usize;
        let cursor = Style::default().add_modifier(Modifier::REVERSED);
        if self.filter.is_empty() {
            buf.set_string(x, area.y, " ", cursor);
            buf.set_stringn(
                x + 1,
                area.y,
                i18n::t().help_filter_placeholder(),
                room.saturating_sub(1),
                Style::default().fg(theme.disabled),
            );
            return;
        }
        // The end of a filter too long for the row.
        let skip = self.filter.chars().count().saturating_sub(room);
        let text: String = self.filter.chars().skip(skip).collect();
        let (end, _) = buf.set_stringn(x, area.y, &text, room, Style::default().fg(theme.fg));
        if end < area.x + area.width {
            buf.set_string(end, area.y, " ", cursor);
        }
    }
}

impl Modal for HelpModal {
    /// The section to open the help panel at.
    type Result = usize;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_width = (f32::from(area.width) * MODAL_MAX_WIDTH_PERCENTAGE_WIDE) as u16;
        let width = max_width.clamp(40, MAX_WIDTH).min(area.width);
        let height = area
            .height
            .saturating_sub(2)
            .max(CHROME + 1)
            .min(area.height);
        let modal = centered_rect_with_size(width, height, area);
        self.last_area = Some(modal);
        let inner = render_modal_block(modal, buf, i18n::t().panel_help(), theme);
        if inner.height < CHROME - 1 {
            return;
        }
        self.render_filter(Rect::new(inner.x, inner.y, inner.width, 1), buf, theme);
        let body = Rect::new(
            inner.x + 1,
            inner.y + 2,
            inner.width.saturating_sub(2),
            inner.height.saturating_sub(4),
        );
        let shown = HelpGenerator::format(&self.sections, body.width as usize, theme, &self.filter);
        self.visible = body.height as usize;
        if let Some(section) = self.start.take() {
            self.scroll = shown.line_of(section).unwrap_or(0);
        }
        let total = shown.lines.len();
        self.shown = Some(shown);
        self.scroll = self.scroll.min(self.max_scroll());
        let found = self.shown.as_ref().is_some_and(|s| !s.anchors.is_empty());
        let shown = self.shown.as_ref().map_or(&[][..], |s| &s.lines[..]);
        if !found {
            buf.set_stringn(
                body.x + 1,
                body.y,
                i18n::t().editor_search_no_matches(),
                body.width as usize,
                Style::default().fg(theme.disabled),
            );
        } else {
            let lines: Vec<Line> = shown
                .iter()
                .skip(self.scroll)
                .take(self.visible)
                .cloned()
                .collect();
            Paragraph::new(lines).render(body, buf);
        }
        if ScrollBar::needs_scrollbar(self.visible, total) {
            ScrollBar::render(
                buf,
                modal.x + modal.width - 1,
                body.y,
                body.height,
                self.scroll,
                self.visible,
                total,
                &ThemeColors::from(theme),
                true,
            );
        }
        buf.set_stringn(
            inner.x + 1,
            inner.y + inner.height - 1,
            i18n::t().help_hint(),
            inner.width.saturating_sub(2) as usize,
            Style::default().fg(theme.disabled),
        );
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Result<Option<ModalResult<usize>>> {
        if self
            .toggle
            .as_ref()
            .is_some_and(|binding| binding.matches_canonical(&chord.canonical))
        {
            return Ok(Some(ModalResult::Cancelled));
        }
        let key = chord.canonical;
        let page = self.visible.max(2) as isize - 1;
        match key.code {
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Enter => return Ok(Some(ModalResult::Confirmed(self.current_section()))),
            KeyCode::BackTab => self.step_section(false),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.step_section(false);
            }
            KeyCode::Tab => self.step_section(true),
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = self.max_scroll(),
            KeyCode::Backspace => {
                let mut filter = self.filter.clone();
                filter.pop();
                self.set_filter(filter);
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                self.set_filter(String::new());
            }
            _ => {
                // Typed text as typed, not as its Latin key.
                let raw = chord.raw;
                if let KeyCode::Char(c) = raw.code {
                    if !raw
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    {
                        let mut filter = self.filter.clone();
                        filter.push(c);
                        self.set_filter(filter);
                    }
                }
            }
        }
        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<usize>>> {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollDown => self.scroll_by(3),
            _ if is_click_outside(&mouse, self.last_area) => {
                return Ok(Some(ModalResult::Cancelled));
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        let line = text.lines().next().unwrap_or_default();
        let filter = format!("{}{line}", self.filter);
        self.set_filter(filter);
        true
    }
}

#[cfg(test)]
mod tests;
