//! The debug panel: the state of a DAP session — the call stack of the
//! stopped thread, the variables of the selected frame, the program's
//! output — and the keys that drive it.
//!
//! The session lives in a slot shared with the app, which starts it and
//! polls its adapter; the panel reads it to draw and calls it on keys.

use std::any::Any;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use termide_core::{
    CommandResult, InputAction, KeyChord, Panel, PanelCommand, PanelEvent, RenderContext,
};
use termide_debug::dap::{OutputKind, Session, SharedSession, Status, Variable};
use termide_theme::Theme;
use unicode_width::UnicodeWidthChar;

/// The `InputAction::Custom` of the evaluate prompt.
const EVALUATE: &str = "debug_evaluate";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Stack,
    Variables,
    Output,
}

/// A variable as a row of the flattened tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VarRow {
    path: Vec<usize>,
    depth: usize,
    name: String,
    value: String,
    type_name: Option<String>,
    has_children: bool,
    expanded: bool,
}

/// Rows of each section on screen, from the last render, for the mouse.
#[derive(Debug, Clone, Copy, Default)]
struct Areas {
    stack: (u16, u16),
    variables: (u16, u16),
    output: (u16, u16),
}

pub struct DebugPanel {
    session: SharedSession,
    theme: Theme,
    focus: Section,
    stack_selected: usize,
    var_selected: usize,
    var_scroll: usize,
    stack_scroll: usize,
    /// Output lines scrolled back from the end; 0 follows new output.
    output_back: usize,
    areas: Areas,
}

impl DebugPanel {
    pub fn new(session: SharedSession) -> Self {
        Self {
            session,
            theme: Theme::default(),
            focus: Section::Variables,
            stack_selected: 0,
            var_selected: 0,
            var_scroll: 0,
            stack_scroll: 0,
            output_back: 0,
            areas: Areas::default(),
        }
    }

    fn with_session<R>(&self, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        let mut slot = self.session.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_mut().map(f)
    }

    fn var_rows(&self) -> Vec<VarRow> {
        self.with_session(|s| flatten(&s.scopes))
            .unwrap_or_default()
    }

    fn frame_count(&self) -> usize {
        self.with_session(|s| s.frames.len()).unwrap_or_default()
    }

    fn move_selection(&mut self, delta: isize) {
        let clamp = |value: usize, len: usize| {
            (value as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize
        };
        match self.focus {
            Section::Stack => self.stack_selected = clamp(self.stack_selected, self.frame_count()),
            Section::Variables => {
                self.var_selected = clamp(self.var_selected, self.var_rows().len())
            }
            Section::Output => {
                let len = self.with_session(|s| s.output.len()).unwrap_or_default();
                self.output_back =
                    (self.output_back as isize - delta).clamp(0, len as isize) as usize;
            }
        }
    }

    /// Enter on the selected row: show that frame, or open that variable.
    fn activate(&mut self) {
        match self.focus {
            Section::Stack => {
                let index = self.stack_selected;
                self.with_session(|s| s.select_frame(index));
                self.var_selected = 0;
                self.var_scroll = 0;
            }
            Section::Variables => {
                if let Some(row) = self.var_rows().get(self.var_selected) {
                    let path = row.path.clone();
                    self.with_session(|s| s.toggle_variable(&path));
                }
            }
            Section::Output => self.output_back = 0,
        }
    }

    /// `→` opens a closed variable, `←` closes an open one or goes to its
    /// parent.
    fn expand(&mut self, open: bool) {
        if self.focus != Section::Variables {
            return;
        }
        let rows = self.var_rows();
        let Some(row) = rows.get(self.var_selected) else {
            return;
        };
        if open && row.has_children && !row.expanded || !open && row.expanded {
            let path = row.path.clone();
            self.with_session(|s| s.toggle_variable(&path));
        } else if !open && row.path.len() > 1 {
            let parent = &row.path[..row.path.len() - 1];
            if let Some(index) = rows.iter().position(|r| r.path == parent) {
                self.var_selected = index;
            }
        }
    }

    fn cycle_focus(&mut self, back: bool) {
        self.focus = match (self.focus, back) {
            (Section::Stack, false) | (Section::Output, true) => Section::Variables,
            (Section::Variables, false) | (Section::Stack, true) => Section::Output,
            (Section::Output, false) | (Section::Variables, true) => Section::Stack,
        };
    }

    // === Drawing ===

    fn draw_status(&self, buf: &mut Buffer, area: Rect, session: Option<&Session>) {
        let t = termide_i18n::t();
        let th = &self.theme;
        let (text, color) = match session.map(|s| &s.status) {
            None => (t.debug_no_session().to_string(), th.disabled),
            Some(Status::Building) => (format!("⚙ {}", t.debug_status_building()), th.warning),
            Some(Status::Starting) => (format!("… {}", t.debug_status_starting()), th.warning),
            Some(Status::Running) => (format!("▶ {}", t.debug_status_running()), th.success),
            Some(Status::Stopped {
                reason,
                description,
            }) => {
                let why = description.as_deref().unwrap_or(reason);
                (format!("‖ {}", t.debug_status_stopped_fmt(why)), th.warning)
            }
            Some(Status::Exited { code }) => {
                let code = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                (
                    format!("■ {}", t.debug_status_exited_fmt(&code)),
                    th.disabled,
                )
            }
            Some(Status::Ended) => (format!("■ {}", t.debug_status_ended()), th.disabled),
            Some(Status::Failed(e)) => (format!("✗ {}", t.debug_status_failed_fmt(e)), th.error),
        };
        let label = session.map(|s| s.spec().label.clone()).unwrap_or_default();
        let mut x = area.x;
        if !label.is_empty() {
            x = put(
                buf,
                x,
                area.y,
                area.right(),
                &format!(" {label} "),
                Style::default().fg(th.fg).add_modifier(Modifier::BOLD),
            );
        }
        put(
            buf,
            x,
            area.y,
            area.right(),
            &format!(" {text}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        );
        if area.height > 1 {
            put(
                buf,
                area.x,
                area.y + 1,
                area.right(),
                &format!(" {}", t.debug_hints()),
                Style::default().fg(th.disabled),
            );
        }
    }

    fn draw_header(&self, buf: &mut Buffer, area: Rect, y: u16, title: &str, section: Section) {
        let th = &self.theme;
        let focused = self.focus == section;
        let style = if focused {
            Style::default()
                .bg(th.accented_bg)
                .fg(th.accented_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(th.accented_bg).fg(th.fg)
        };
        fill(buf, area.x, y, area.right(), style);
        put(buf, area.x, y, area.right(), &format!(" {title}"), style);
    }

    fn row_style(&self, section: Section, selected: bool) -> Style {
        let th = &self.theme;
        if selected && self.focus == section {
            Style::default().bg(th.selected_bg).fg(th.selected_fg)
        } else {
            Style::default().fg(th.fg)
        }
    }

    fn draw_stack(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        y0: u16,
        height: u16,
        session: Option<&Session>,
    ) {
        let th = self.theme;
        let frames = session.map(|s| s.frames.as_slice()).unwrap_or_default();
        let current = session.map(|s| s.selected_frame).unwrap_or_default();
        if frames.is_empty() {
            let t = termide_i18n::t();
            put(
                buf,
                area.x,
                y0,
                area.right(),
                &format!("  {}", t.debug_no_frames()),
                Style::default().fg(th.disabled),
            );
            return;
        }
        self.stack_selected = self.stack_selected.min(frames.len() - 1);
        self.stack_scroll = scroll_to(self.stack_scroll, self.stack_selected, height as usize);
        for (row, (i, frame)) in frames
            .iter()
            .enumerate()
            .skip(self.stack_scroll)
            .take(height as usize)
            .enumerate()
        {
            let y = y0 + row as u16;
            let style = self.row_style(Section::Stack, i == self.stack_selected);
            fill(buf, area.x, y, area.right(), style);
            let mark = if i == current { "→ " } else { "  " };
            let mut x = put(buf, area.x, y, area.right(), mark, style.fg(th.warning));
            x = put(buf, x, y, area.right(), &frame.name, style);
            if let Some(path) = &frame.path {
                let file = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let place = format!("  {}:{}", file, frame.line);
                put(
                    buf,
                    x,
                    y,
                    area.right(),
                    &place,
                    style.fg(
                        if i == self.stack_selected && self.focus == Section::Stack {
                            th.selected_fg
                        } else {
                            th.disabled
                        },
                    ),
                );
            }
        }
    }

    fn draw_variables(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        y0: u16,
        height: u16,
        rows: &[VarRow],
    ) {
        let th = self.theme;
        if rows.is_empty() {
            return;
        }
        self.var_selected = self.var_selected.min(rows.len() - 1);
        self.var_scroll = scroll_to(self.var_scroll, self.var_selected, height as usize);
        for (offset, (i, row)) in rows
            .iter()
            .enumerate()
            .skip(self.var_scroll)
            .take(height as usize)
            .enumerate()
        {
            let y = y0 + offset as u16;
            let selected = i == self.var_selected && self.focus == Section::Variables;
            let style = self.row_style(Section::Variables, i == self.var_selected);
            fill(buf, area.x, y, area.right(), style);
            let arrow = match (row.has_children, row.expanded) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                _ => "  ",
            };
            let indent = " ".repeat(1 + row.depth * 2);
            let mut x = put(buf, area.x, y, area.right(), &indent, style);
            x = put(buf, x, y, area.right(), arrow, style);
            let name_style = if row.depth == 0 {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            };
            x = put(buf, x, y, area.right(), &row.name, name_style);
            if row.depth > 0 {
                x = put(buf, x, y, area.right(), " = ", style);
                x = put(buf, x, y, area.right(), &one_line(&row.value), style);
                if let Some(type_name) = &row.type_name {
                    let dim = if selected {
                        style
                    } else {
                        style.fg(th.disabled)
                    };
                    put(buf, x, y, area.right(), &format!("  {type_name}"), dim);
                }
            }
        }
    }

    fn draw_output(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        y0: u16,
        height: u16,
        session: Option<&Session>,
    ) {
        let th = self.theme;
        let Some(session) = session else {
            return;
        };
        let len = session.output.len();
        self.output_back = self.output_back.min(len.saturating_sub(height as usize));
        let end = len - self.output_back;
        // Long lines wrap; the rows of the lines up to `end` that fit are
        // shown, the last at the bottom.
        let width = area.width.saturating_sub(1).max(1) as usize;
        let mut rows: Vec<(String, Style)> = Vec::new();
        for line in session.output.range(..end).rev() {
            if rows.len() >= height as usize {
                break;
            }
            let (prefix, style) = match line.kind {
                OutputKind::Stdout => ("", Style::default().fg(th.fg)),
                OutputKind::Stderr | OutputKind::Error => ("", Style::default().fg(th.error)),
                OutputKind::Console | OutputKind::Build => ("", Style::default().fg(th.disabled)),
                OutputKind::Input => ("› ", Style::default().fg(th.accented_fg)),
                OutputKind::Result => ("= ", Style::default().fg(th.success)),
            };
            let text = format!("{prefix}{}", line.text.replace('\t', "    "));
            let wrapped = wrap(&text, width);
            rows.extend(wrapped.into_iter().rev().map(|row| (row, style)));
        }
        rows.truncate(height as usize);
        for (i, (row, style)) in rows.into_iter().rev().enumerate() {
            let y = y0 + i as u16;
            let x = put(buf, area.x, y, area.right(), " ", style);
            put(buf, x, y, area.right(), &row, style);
        }
    }
}

/// `text` cut into rows at most `width` columns wide; an empty text is one
/// empty row.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width && used > 0 {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().expect("never empty").push(ch);
        used += w;
    }
    rows
}

impl Drop for DebugPanel {
    /// Closing the panel ends its session, and the program with it.
    fn drop(&mut self) {
        self.with_session(Session::stop);
    }
}

impl Panel for DebugPanel {
    fn name(&self) -> &'static str {
        "debug"
    }

    fn title(&self) -> String {
        termide_i18n::t().debug_panel_title().to_string()
    }

    fn prepare_render(&mut self, theme: &Theme, _config: &std::sync::Arc<termide_config::Config>) {
        self.theme = *theme;
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _ctx: &RenderContext) {
        let th = self.theme;
        let blank = Style::default().bg(th.bg).fg(th.fg);
        for y in area.top()..area.bottom() {
            fill(buf, area.x, y, area.right(), blank);
        }
        if area.height == 0 || area.width == 0 {
            return;
        }
        let slot = std::sync::Arc::clone(&self.session);
        let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        let session = guard.as_ref();
        let rows = session.map(|s| flatten(&s.scopes)).unwrap_or_default();

        let status_rows = area.height.min(2);
        self.draw_status(buf, area, session);
        let body_top = area.y + status_rows;
        let body = area.height - status_rows;
        if body < 3 {
            return;
        }
        // Three headers; the rest split: a quarter (as needed) for the stack,
        // then the variables, then the output.
        let content = body - 3;
        let frames = session.map(|s| s.frames.len()).unwrap_or_default().max(1) as u16;
        let stack_h = frames.min((content / 4).max(1)).min(content);
        let vars_h = (content - stack_h) / 2;
        let out_h = content - stack_h - vars_h;

        let t = termide_i18n::t();
        let mut y = body_top;
        self.draw_header(buf, area, y, t.debug_section_stack(), Section::Stack);
        y += 1;
        self.areas.stack = (y, stack_h);
        self.draw_stack(buf, area, y, stack_h, session);
        y += stack_h;
        self.draw_header(
            buf,
            area,
            y,
            t.debug_section_variables(),
            Section::Variables,
        );
        y += 1;
        self.areas.variables = (y, vars_h);
        self.draw_variables(buf, area, y, vars_h, &rows);
        y += vars_h;
        self.draw_header(buf, area, y, t.debug_section_output(), Section::Output);
        y += 1;
        self.areas.output = (y, out_h);
        self.draw_output(buf, area, y, out_h, session);
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        let key = chord.canonical;
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        match key.code {
            KeyCode::Char('c') | KeyCode::F(5) if plain => {
                self.with_session(Session::continue_);
            }
            KeyCode::Char('n') if plain => {
                self.with_session(Session::step_over);
            }
            KeyCode::Char('s') if plain => {
                self.with_session(Session::step_in);
            }
            KeyCode::Char('o') if plain => {
                self.with_session(Session::step_out);
            }
            KeyCode::Char('p') if plain => {
                self.with_session(Session::pause);
            }
            KeyCode::Char('x') if plain => {
                self.with_session(Session::stop);
            }
            KeyCode::Char('r') if plain => return vec![PanelEvent::RestartDebug],
            KeyCode::Char('e') if plain => {
                return vec![PanelEvent::ShowInput {
                    prompt: termide_i18n::t().debug_evaluate_prompt().to_string(),
                    initial_value: String::new(),
                    on_submit: InputAction::Custom(EVALUATE.into()),
                }];
            }
            KeyCode::Tab => self.cycle_focus(false),
            KeyCode::BackTab => self.cycle_focus(true),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.move_selection(isize::MIN / 2),
            KeyCode::End => self.move_selection(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(),
            KeyCode::Right => self.expand(true),
            KeyCode::Left => self.expand(false),
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        let in_rows = |(y0, h): (u16, u16)| {
            (event.row >= y0 && event.row < y0 + h).then(|| (event.row - y0) as usize)
        };
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(row) = in_rows(self.areas.stack) {
                    self.focus = Section::Stack;
                    let index = self.stack_scroll + row;
                    if index < self.frame_count() {
                        self.stack_selected = index;
                        self.activate();
                    }
                } else if let Some(row) = in_rows(self.areas.variables) {
                    self.focus = Section::Variables;
                    let index = self.var_scroll + row;
                    if index < self.var_rows().len() {
                        let again = self.var_selected == index;
                        self.var_selected = index;
                        if again {
                            self.activate();
                        }
                    }
                } else if in_rows(self.areas.output).is_some() {
                    self.focus = Section::Output;
                } else {
                    return vec![];
                }
                vec![PanelEvent::NeedsRedraw]
            }
            MouseEventKind::ScrollUp => self.handle_scroll(-1, _panel_area),
            MouseEventKind::ScrollDown => self.handle_scroll(1, _panel_area),
            _ => vec![],
        }
    }

    fn handle_scroll(&mut self, delta: i32, _area: Rect) -> Vec<PanelEvent> {
        self.move_selection(delta as isize * 3);
        vec![PanelEvent::NeedsRedraw]
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::InputSubmitted { action, text } if action == EVALUATE => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    self.with_session(|s| s.evaluate(&text));
                    self.output_back = 0;
                }
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The variables tree as rows, the open nodes' children under them.
fn flatten(scopes: &[Variable]) -> Vec<VarRow> {
    fn walk(nodes: &[Variable], path: &mut Vec<usize>, depth: usize, out: &mut Vec<VarRow>) {
        for (i, node) in nodes.iter().enumerate() {
            path.push(i);
            out.push(VarRow {
                path: path.clone(),
                depth,
                name: node.name.clone(),
                value: node.value.clone(),
                type_name: node.type_name.clone(),
                has_children: node.has_children(),
                expanded: node.expanded,
            });
            if node.expanded {
                if let Some(children) = &node.children {
                    walk(children, path, depth + 1, out);
                }
            }
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(scopes, &mut Vec::new(), 0, &mut out);
    out
}

/// The scroll offset that keeps `selected` within `height` rows.
fn scroll_to(scroll: usize, selected: usize, height: usize) -> usize {
    if height == 0 {
        return selected;
    }
    if selected < scroll {
        selected
    } else if selected >= scroll + height {
        selected + 1 - height
    } else {
        scroll
    }
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

fn fill(buf: &mut Buffer, x0: u16, y: u16, right: u16, style: Style) {
    for x in x0..right {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(' ');
            cell.set_style(style);
        }
    }
}

/// Draw `text` from `x`, clipped at `right`; returns the column after it.
fn put(buf: &mut Buffer, mut x: u16, y: u16, right: u16, text: &str, style: Style) -> u16 {
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0) as u16;
        if width == 0 {
            continue;
        }
        if x + width > right {
            break;
        }
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
        for extra in 1..width {
            if let Some(cell) = buf.cell_mut((x + extra, y)) {
                cell.set_symbol("");
            }
        }
        x += width;
    }
    x
}

#[cfg(test)]
mod tests;
