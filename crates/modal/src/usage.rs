//! The agents' usage window: what one session, the project or every project
//! spent, a scope and a period chosen with tabs.
//!
//! The window only shows what it is given. Its owner reads [`UsageModal::wants`]
//! after each key or click and hands it the lines of the view now chosen;
//! a session line that carries a log's path opens that session when picked.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use termide_config::constants::{
    MODAL_MAX_WIDTH_PERCENTAGE_WIDE, SPINNER_FRAMES, SPINNER_FRAMES_COUNT,
};
use termide_core::ThemeColors;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::scrollbar::ScrollBar;

use crate::base::render_modal_block;
use crate::info::InfoModal;
use crate::{centered_rect_with_size, is_click_outside, Modal, ModalResult};

/// Whose spending the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageScopeTab {
    /// The session of the agent panel it was opened from.
    Session,
    /// The project's sessions.
    Project,
    /// Every project's.
    All,
}

/// What the window shows: a scope, and for the project and every project a
/// period, by its index among the periods offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageView {
    pub scope: UsageScopeTab,
    pub period: usize,
}

/// A line of the window: a key, a value of one or more lines, and for a
/// session that can be opened, its log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLine {
    pub key: String,
    pub value: String,
    pub link: Option<PathBuf>,
}

impl UsageLine {
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            link: None,
        }
    }

    #[must_use]
    pub fn with_link(mut self, link: Option<PathBuf>) -> Self {
        self.link = link;
        self
    }
}

/// What the body holds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// The logs are being counted: so many read of so many.
    Counting {
        done: usize,
        total: usize,
    },
    Lines(Vec<UsageLine>),
}

/// Where a click lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Scope(UsageScopeTab),
    Period(usize),
}

/// Rows of the frame beside the body: the border, the tabs and a blank line
/// above, a blank line, the hint and the border below.
const CHROME: u16 = 6;

/// The usage window.
#[derive(Debug)]
pub struct UsageModal {
    title: String,
    scopes: Vec<(UsageScopeTab, String)>,
    periods: Vec<String>,
    view: UsageView,
    /// The session tab's lines, when there is a session.
    session: Option<Vec<UsageLine>>,
    /// The view the body was given for, and the body.
    shown: Option<UsageView>,
    body: Body,
    /// The picked session line, by its index among the linked lines.
    selected: Option<usize>,
    scroll: usize,
    spinner: usize,
    /// The widest and tallest the window has been, so it does not shrink
    /// as the tabs change.
    floor: (u16, u16),
    last_area: Option<Rect>,
    targets: Vec<(Rect, Target)>,
    /// The body's screen rows from the last frame: each the linked line on
    /// it, if any.
    rows: Vec<(u16, Option<usize>)>,
    total_lines: usize,
    visible: usize,
}

impl UsageModal {
    /// A window titled `title`, offering `periods` (the first shown is
    /// `period`), the session tab with `session`'s lines when given, and
    /// opened on `scope`.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        periods: Vec<String>,
        period: usize,
        session: Option<Vec<UsageLine>>,
        scope: UsageScopeTab,
    ) -> Self {
        let t = i18n::t();
        let mut scopes = Vec::new();
        if session.is_some() {
            scopes.push((UsageScopeTab::Session, t.usage_scope_session().to_string()));
        }
        scopes.push((UsageScopeTab::Project, t.usage_scope_project().to_string()));
        scopes.push((UsageScopeTab::All, t.usage_scope_all().to_string()));
        let scope = if session.is_none() && scope == UsageScopeTab::Session {
            UsageScopeTab::Project
        } else {
            scope
        };
        Self {
            title: title.into(),
            scopes,
            periods,
            view: UsageView { scope, period },
            session,
            shown: None,
            body: Body::Counting { done: 0, total: 0 },
            selected: None,
            scroll: 0,
            spinner: 0,
            floor: (0, 0),
            last_area: None,
            targets: Vec::new(),
            rows: Vec::new(),
            total_lines: 0,
            visible: 0,
        }
        .reset_for_view()
    }

    /// The view chosen now.
    #[must_use]
    pub fn view(&self) -> UsageView {
        self.view
    }

    /// The view whose lines the window waits for: the one chosen, when it
    /// is not the session's and its lines were not given yet.
    #[must_use]
    pub fn wants(&self) -> Option<UsageView> {
        (self.view.scope != UsageScopeTab::Session && self.shown != Some(self.view))
            .then_some(self.view)
    }

    /// The logs are being counted, `done` of `total`, and nothing can be
    /// shown yet.
    pub fn set_counting(&mut self, done: usize, total: usize) {
        if self.view.scope != UsageScopeTab::Session {
            self.body = Body::Counting { done, total };
            self.shown = None;
        }
    }

    /// Whether it shows the counting of the logs.
    #[must_use]
    pub fn is_counting(&self) -> bool {
        self.view.scope != UsageScopeTab::Session && matches!(self.body, Body::Counting { .. })
    }

    /// The lines of `view`; ignored when another is chosen by now. Called
    /// again for the view shown, as the logs change, it keeps the place.
    pub fn set_lines(&mut self, view: UsageView, lines: Vec<UsageLine>) {
        if view != self.view {
            return;
        }
        let again = self.shown == Some(view);
        self.shown = Some(view);
        self.body = Body::Lines(lines);
        if !again {
            self.scroll = 0;
            self.selected = None;
        }
        let links = self.links().len();
        self.selected = self.selected.filter(|at| *at < links);
    }

    /// Advance the counting spinner.
    pub fn advance_spinner(&mut self) {
        self.spinner = (self.spinner + 1) % SPINNER_FRAMES_COUNT;
    }

    fn reset_for_view(mut self) -> Self {
        if self.view.scope == UsageScopeTab::Session {
            self.shown = Some(self.view);
        }
        self
    }

    /// The lines of the view shown.
    fn lines(&self) -> &[UsageLine] {
        match (self.view.scope, &self.session, &self.body) {
            (UsageScopeTab::Session, Some(lines), _) => lines,
            (UsageScopeTab::Session, None, _) => &[],
            (_, _, Body::Lines(lines)) => lines,
            (_, _, Body::Counting { .. }) => &[],
        }
    }

    /// The logs of the linked lines, in order.
    fn links(&self) -> Vec<&PathBuf> {
        self.lines()
            .iter()
            .filter_map(|l| l.link.as_ref())
            .collect()
    }

    fn choose(&mut self, view: UsageView) {
        if view == self.view {
            return;
        }
        self.view = view;
        self.scroll = 0;
        self.selected = None;
        if view.scope == UsageScopeTab::Session {
            self.shown = Some(view);
        } else if self.shown != Some(view) {
            self.body = Body::Counting { done: 0, total: 0 };
        }
    }

    fn step_scope(&mut self, forward: bool) {
        let at = self
            .scopes
            .iter()
            .position(|(scope, _)| *scope == self.view.scope)
            .unwrap_or(0);
        let n = self.scopes.len();
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        let scope = self.scopes[next].0;
        self.choose(UsageView { scope, ..self.view });
    }

    fn step_period(&mut self, forward: bool) {
        if self.view.scope == UsageScopeTab::Session || self.periods.is_empty() {
            return;
        }
        let n = self.periods.len();
        let period = if forward {
            (self.view.period + 1).min(n - 1)
        } else {
            self.view.period.saturating_sub(1)
        };
        self.choose(UsageView {
            period,
            ..self.view
        });
    }

    fn max_scroll(&self) -> usize {
        self.total_lines.saturating_sub(self.visible)
    }

    fn step_selection(&mut self, down: bool) {
        let links = self.links().len();
        if links == 0 {
            self.scroll = if down {
                (self.scroll + 1).min(self.max_scroll())
            } else {
                self.scroll.saturating_sub(1)
            };
            return;
        }
        self.selected = Some(match (self.selected, down) {
            (None, true) => 0,
            (None, false) => links - 1,
            (Some(at), true) => (at + 1).min(links - 1),
            (Some(at), false) => at.saturating_sub(1),
        });
    }

    /// The body as screen lines, each with the linked line it shows.
    fn body_lines(&self, width: usize, theme: &Theme) -> Vec<(Line<'static>, Option<usize>)> {
        let t = i18n::t();
        if let (true, Body::Counting { done, total }) =
            (self.view.scope != UsageScopeTab::Session, &self.body)
        {
            let text = format!(
                "{} {}",
                SPINNER_FRAMES[self.spinner],
                t.usage_counting_fmt(*done, *total)
            );
            return vec![(
                Line::styled(format!("  {text}"), Style::default().fg(theme.fg)),
                None,
            )];
        }
        let lines = self.lines();
        let key_width = lines.iter().map(|l| l.key.width()).max().unwrap_or(0);
        let value_width = width.saturating_sub(key_width + 4).max(10);
        let key_style = Style::default()
            .fg(theme.accented_fg)
            .add_modifier(Modifier::BOLD);
        let mut out = Vec::new();
        let mut link = 0;
        for line in lines {
            let linked = line.link.is_some().then(|| {
                link += 1;
                link - 1
            });
            let picked = linked.is_some() && linked == self.selected;
            let value_style = if picked {
                Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.fg)
            };
            let padding = " ".repeat(key_width - line.key.width());
            for (index, text) in InfoModal::wrap_text(&line.value, value_width)
                .into_iter()
                .enumerate()
            {
                let key = if index == 0 {
                    format!("  {}{padding}  ", line.key)
                } else {
                    " ".repeat(key_width + 4)
                };
                out.push((
                    Line::from(vec![
                        Span::styled(key, key_style),
                        Span::styled(text, value_style),
                    ]),
                    linked,
                ));
            }
        }
        out
    }

    fn content_width(&self) -> usize {
        let lines = self.lines();
        let key_width = lines.iter().map(|l| l.key.width()).max().unwrap_or(0);
        let value_width = lines
            .iter()
            .flat_map(|l| l.value.split('\n'))
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        let tabs: usize = self
            .scopes
            .iter()
            .map(|(_, s)| s.width() + 3)
            .sum::<usize>()
            + self.periods.iter().map(|p| p.width() + 3).sum::<usize>()
            + 4;
        (key_width + 4 + value_width)
            .max(tabs)
            .max(i18n::t().usage_hint().width() + 2)
    }

    /// The tab row: the scopes on the left, the periods on the right when
    /// they apply, those that do not fit left out.
    fn render_tabs(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.targets.clear();
        let active = Style::default()
            .fg(theme.selected_fg)
            .bg(theme.selected_bg)
            .add_modifier(Modifier::BOLD);
        let idle = Style::default().fg(theme.fg);
        let right = area.x + area.width;
        let mut x = area.x + 1;
        for (scope, label) in &self.scopes {
            let text = format!(" {label} ");
            let width = text.width() as u16;
            if x + width > right {
                break;
            }
            let style = if *scope == self.view.scope {
                active
            } else {
                idle
            };
            buf.set_string(x, area.y, &text, style);
            self.targets
                .push((Rect::new(x, area.y, width, 1), Target::Scope(*scope)));
            x += width + 1;
        }
        if self.view.scope == UsageScopeTab::Session {
            return;
        }
        let widths: Vec<u16> = self.periods.iter().map(|p| p.width() as u16 + 2).collect();
        let total: u16 = widths.iter().sum::<u16>() + widths.len() as u16;
        // Beside the scopes, right-aligned; else on the row below them;
        // else the chosen period alone there.
        let (mut px, y) = if right.saturating_sub(total) > x {
            (right.saturating_sub(total), area.y)
        } else if area.x + 1 + total <= right {
            (area.x + 1, area.y + 1)
        } else {
            let label = format!("‹ {} ›", self.periods[self.view.period]);
            buf.set_stringn(
                area.x + 1,
                area.y + 1,
                &label,
                area.width.saturating_sub(2) as usize,
                active,
            );
            return;
        };
        for (index, label) in self.periods.iter().enumerate() {
            let style = if index == self.view.period {
                active
            } else {
                idle
            };
            buf.set_string(px, y, format!(" {label} "), style);
            self.targets
                .push((Rect::new(px, y, widths[index], 1), Target::Period(index)));
            px += widths[index] + 1;
        }
    }
}

impl Modal for UsageModal {
    /// The session log picked to open.
    type Result = PathBuf;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_width = (f32::from(area.width) * MODAL_MAX_WIDTH_PERCENTAGE_WIDE) as u16;
        let wanted = (self.content_width() as u16).saturating_add(4);
        let width = wanted.max(self.floor.0).min(max_width).min(area.width);
        let inner_width = width.saturating_sub(4) as usize;
        let lines = self.body_lines(inner_width, theme);
        let max_height = area.height.saturating_sub(2);
        let wanted = (lines.len() as u16).saturating_add(CHROME);
        let height = wanted.max(self.floor.1).min(max_height).max(CHROME + 1);
        self.floor = (width, height);
        let modal = centered_rect_with_size(width, height, area);
        self.last_area = Some(modal);
        let inner = render_modal_block(modal, buf, &self.title, theme);
        if inner.height < 4 {
            return;
        }
        self.render_tabs(Rect::new(inner.x, inner.y, inner.width, 1), buf, theme);
        let body = Rect::new(
            inner.x + 1,
            inner.y + 2,
            inner.width.saturating_sub(1),
            inner.height.saturating_sub(4),
        );
        self.visible = body.height as usize;
        self.total_lines = lines.len();
        // Keep the picked session in sight.
        if let Some(picked) = self.selected {
            if let Some(at) = lines.iter().position(|(_, link)| *link == Some(picked)) {
                if at < self.scroll {
                    self.scroll = at;
                } else if at >= self.scroll + self.visible {
                    self.scroll = at + 1 - self.visible;
                }
            }
        }
        self.scroll = self.scroll.min(self.max_scroll());
        self.rows.clear();
        let shown: Vec<(Line<'static>, Option<usize>)> = lines
            .into_iter()
            .skip(self.scroll)
            .take(self.visible)
            .collect();
        for (offset, (line, link)) in shown.into_iter().enumerate() {
            let y = body.y + offset as u16;
            Paragraph::new(line).render(Rect::new(body.x, y, body.width, 1), buf);
            self.rows.push((y, link));
        }
        if ScrollBar::needs_scrollbar(self.visible, self.total_lines) {
            ScrollBar::render(
                buf,
                modal.x + modal.width - 1,
                body.y,
                body.height,
                self.scroll,
                self.visible,
                self.total_lines,
                &ThemeColors::from(theme),
                true,
            );
        }
        let hint = i18n::t().usage_hint();
        buf.set_stringn(
            inner.x + 1,
            inner.y + inner.height - 1,
            hint,
            inner.width.saturating_sub(2) as usize,
            Style::default().fg(theme.disabled),
        );
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<PathBuf>>> {
        let key = chord.canonical;
        let page = self.visible.max(1);
        match key.code {
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Enter => {
                let picked = self
                    .selected
                    .and_then(|at| self.links().get(at).map(|path| (*path).clone()));
                return Ok(Some(match picked {
                    Some(path) => ModalResult::Confirmed(path),
                    None => ModalResult::Cancelled,
                }));
            }
            KeyCode::BackTab => self.step_scope(false),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.step_scope(false),
            KeyCode::Tab => self.step_scope(true),
            KeyCode::Left | KeyCode::Char('[') => self.step_period(false),
            KeyCode::Right | KeyCode::Char(']') => self.step_period(true),
            KeyCode::Up | KeyCode::Char('k') => self.step_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.step_selection(true),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::PageDown => self.scroll = (self.scroll + page).min(self.max_scroll()),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = self.max_scroll(),
            _ => {}
        }
        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<PathBuf>>> {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                self.scroll = (self.scroll + 3).min(self.max_scroll());
                return Ok(None);
            }
            _ => {}
        }
        if is_click_outside(&mouse, self.last_area) {
            return Ok(Some(ModalResult::Cancelled));
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }
        let hit = self.targets.iter().find(|(rect, _)| {
            mouse.row == rect.y && mouse.column >= rect.x && mouse.column < rect.x + rect.width
        });
        if let Some((_, target)) = hit {
            match *target {
                Target::Scope(scope) => self.choose(UsageView { scope, ..self.view }),
                Target::Period(period) => self.choose(UsageView {
                    period,
                    ..self.view
                }),
            }
            return Ok(None);
        }
        let link = self
            .rows
            .iter()
            .find(|(y, _)| *y == mouse.row)
            .and_then(|(_, link)| *link);
        if let Some(at) = link {
            if let Some(path) = self.links().get(at).map(|path| (*path).clone()) {
                return Ok(Some(ModalResult::Confirmed(path)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
