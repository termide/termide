//! Modal form for command launch parameters.
//!
//! Renders a dynamic form based on CommandParam definitions from commands.toml.
//! Each parameter type gets an appropriate widget: text input, number input,
//! checkbox (bool), or select dropdown.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use std::collections::HashMap;

use crate::base::{button_style, render_input_field, render_modal_block};
use crate::input_keys::{handle_input_key, InputKeyResult};
use termide_config::commands::{CommandParam, CommandParamType};
use termide_config::constants::{
    MODAL_BUTTON_SPACING, MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT, MODAL_MIN_WIDTH_WIDE,
    MODAL_PADDING_WITH_DOUBLE_BORDER,
};
use termide_theme::Theme;

use crate::{centered_rect_with_size, Modal, ModalResult, TextInputHandler};

/// Result of the command parameters modal.
#[derive(Debug, Clone)]
pub struct CommandParamsResult {
    /// Map from parameter name to user-provided value.
    pub values: HashMap<String, String>,
}

/// Runtime value for a single parameter field.
#[derive(Debug, Clone)]
enum ParamValue {
    Text { input: TextInputHandler },
    Number { input: TextInputHandler },
    Bool { checked: bool },
    Select { selected: usize },
}

/// Focus area in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    /// One of the parameter fields (index).
    Field(usize),
    /// Action buttons.
    Buttons,
}

/// Modal for entering command launch parameters.
#[derive(Debug)]
pub struct CommandParamsModal {
    command_name: String,
    params: Vec<CommandParam>,
    values: Vec<ParamValue>,
    focus: FocusArea,
    selected_button: usize, // 0 = Run, 1 = Cancel
    last_buttons_area: Option<Rect>,
    last_field_areas: Vec<Rect>,
}

impl CommandParamsModal {
    /// Create a new command params modal.
    pub fn new(command_name: String, params: Vec<CommandParam>) -> Self {
        let values = params
            .iter()
            .map(|p| match p.param_type {
                CommandParamType::Text => {
                    let mut input = TextInputHandler::new();
                    if let Some(ref d) = p.default {
                        input.set_text(d.clone());
                    }
                    ParamValue::Text { input }
                }
                CommandParamType::Number => {
                    let mut input = TextInputHandler::new();
                    if let Some(ref d) = p.default {
                        input.set_text(d.clone());
                    }
                    ParamValue::Number { input }
                }
                CommandParamType::Bool => {
                    let checked = p.default.as_deref() == Some("true");
                    ParamValue::Bool { checked }
                }
                CommandParamType::Select => {
                    let selected = p
                        .default
                        .as_ref()
                        .and_then(|d| p.options.iter().position(|o| o == d))
                        .unwrap_or(0);
                    ParamValue::Select { selected }
                }
            })
            .collect();

        let focus = if params.is_empty() {
            FocusArea::Buttons
        } else {
            FocusArea::Field(0)
        };

        Self {
            command_name,
            params,
            values,
            focus,
            selected_button: 0,
            last_buttons_area: None,
            last_field_areas: Vec::new(),
        }
    }

    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let title_width = self.command_name.len() as u16 + 20; // "Parameters: name" + padding
        let label_width = 20u16;
        let input_width = 30u16;
        let content_width = title_width.max(label_width + input_width).max(36);
        let total_width = content_width + MODAL_PADDING_WITH_DOUBLE_BORDER;

        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT) as u16;
        let width = total_width
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width);

        // Border(1) + fields (2 rows each: label + input) + empty(1) + buttons(1) + border(1)
        let height = (1 + self.params.len() as u16 * 2 + 1 + 1 + 1).min(screen_height);

        (width, height)
    }

    fn next_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Field(i) => {
                if i + 1 < self.params.len() {
                    FocusArea::Field(i + 1)
                } else {
                    FocusArea::Buttons
                }
            }
            FocusArea::Buttons => {
                if self.params.is_empty() {
                    FocusArea::Buttons
                } else {
                    FocusArea::Field(0)
                }
            }
        };
    }

    fn prev_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Field(i) => {
                if i > 0 {
                    FocusArea::Field(i - 1)
                } else {
                    FocusArea::Buttons
                }
            }
            FocusArea::Buttons => {
                if self.params.is_empty() {
                    FocusArea::Buttons
                } else {
                    FocusArea::Field(self.params.len() - 1)
                }
            }
        };
    }

    fn try_confirm(&self) -> Option<ModalResult<CommandParamsResult>> {
        let mut values = HashMap::new();
        for (param, value) in self.params.iter().zip(self.values.iter()) {
            let str_value = match value {
                ParamValue::Text { input } => input.text().to_string(),
                ParamValue::Number { input } => input.text().to_string(),
                ParamValue::Bool { checked } => checked.to_string(),
                ParamValue::Select { selected } => {
                    param.options.get(*selected).cloned().unwrap_or_default()
                }
            };
            values.insert(param.name.clone(), str_value);
        }
        Some(ModalResult::Confirmed(CommandParamsResult { values }))
    }
}

impl Modal for CommandParamsModal {
    type Result = CommandParamsResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let t = termide_i18n::t();
        let title = format!("{} {}", t.command_params_title(), self.command_name);
        let inner = render_modal_block(modal_area, buf, &title, theme);

        // Layout: fields + spacer + buttons
        let mut constraints: Vec<_> = self
            .params
            .iter()
            .flat_map(|_| [Constraint::Length(1), Constraint::Length(1)])
            .collect();
        constraints.push(Constraint::Length(1)); // spacer
        constraints.push(Constraint::Length(1)); // buttons

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        self.last_field_areas.clear();
        let mut chunk_idx = 0;

        for (i, (param, value)) in self.params.iter().zip(self.values.iter()).enumerate() {
            let is_focused = self.focus == FocusArea::Field(i);

            // Label row
            let label = format!("{}:", param.label);
            let label_style = if is_focused {
                Style::default().fg(theme.accented_fg)
            } else {
                Style::default().fg(theme.fg)
            };
            let label_para = Paragraph::new(label).style(label_style);
            label_para.render(chunks[chunk_idx], buf);
            chunk_idx += 1;

            // Input row
            let field_area = chunks[chunk_idx];
            self.last_field_areas.push(field_area);

            match value {
                ParamValue::Text { input } => {
                    let border_style = if is_focused {
                        Style::default().fg(theme.accented_fg)
                    } else {
                        Style::default().fg(theme.disabled)
                    };
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style);
                    let inner = block.inner(field_area);
                    block.render(field_area, buf);
                    render_input_field(
                        buf,
                        inner.x,
                        inner.y,
                        inner.width,
                        input.text(),
                        input.cursor_pos(),
                        input.selection_range(),
                        is_focused,
                        theme,
                    );
                }
                ParamValue::Number { input } => {
                    let border_style = if is_focused {
                        Style::default().fg(theme.accented_fg)
                    } else {
                        Style::default().fg(theme.disabled)
                    };
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style);
                    let inner = block.inner(field_area);
                    block.render(field_area, buf);
                    render_input_field(
                        buf,
                        inner.x,
                        inner.y,
                        inner.width,
                        input.text(),
                        input.cursor_pos(),
                        input.selection_range(),
                        is_focused,
                        theme,
                    );
                }
                ParamValue::Bool { checked } => {
                    let checkbox = termide_ui::checkbox(*checked);
                    let style = if is_focused {
                        Style::default()
                            .fg(theme.accented_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let text = format!(" {}  Enter/Space to toggle", checkbox);
                    let para = Paragraph::new(text).style(style);
                    para.render(field_area, buf);
                }
                ParamValue::Select { selected } => {
                    let current = param
                        .options
                        .get(*selected)
                        .map(|s| s.as_str())
                        .unwrap_or("-");
                    let style = if is_focused {
                        Style::default()
                            .fg(theme.accented_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let text = format!(" < {} >  \u{2190}/\u{2192} to change", current);
                    let para = Paragraph::new(text).style(style);
                    para.render(field_area, buf);
                }
            }
            chunk_idx += 1;
        }

        // Skip spacer
        chunk_idx += 1;

        // Buttons
        let run_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 0,
            theme,
        );
        let cancel_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 1,
            theme,
        );

        let buttons = Line::from(vec![
            Span::styled(format!("[ {} ]", t.command_params_run()), run_style),
            Span::raw("    "),
            Span::styled(format!("[ {} ]", t.command_params_cancel()), cancel_style),
        ]);

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[chunk_idx], buf);
        self.last_buttons_area = Some(chunks[chunk_idx]);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        // Tab / Shift+Tab navigation
        if key.code == KeyCode::Tab {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.prev_focus();
            } else {
                self.next_focus();
            }
            return Ok(None);
        }

        match self.focus {
            FocusArea::Field(idx) => {
                let param = &self.params[idx];
                match &mut self.values[idx] {
                    ParamValue::Text { input } | ParamValue::Number { input } => {
                        match handle_input_key(input, key) {
                            InputKeyResult::Handled | InputKeyResult::TextModified => {
                                return Ok(None);
                            }
                            InputKeyResult::NotHandled => {}
                        }
                        match key.code {
                            KeyCode::Down | KeyCode::Enter => self.next_focus(),
                            KeyCode::Up => self.prev_focus(),
                            _ => {}
                        }
                    }
                    ParamValue::Bool { checked } => match key.code {
                        KeyCode::Char(' ') | KeyCode::Enter => {
                            *checked = !*checked;
                        }
                        KeyCode::Down => self.next_focus(),
                        KeyCode::Up => self.prev_focus(),
                        _ => {}
                    },
                    ParamValue::Select { selected } => match key.code {
                        KeyCode::Left => {
                            if *selected > 0 {
                                *selected -= 1;
                            }
                        }
                        KeyCode::Right => {
                            if *selected + 1 < param.options.len() {
                                *selected += 1;
                            }
                        }
                        KeyCode::Down | KeyCode::Enter => self.next_focus(),
                        KeyCode::Up => self.prev_focus(),
                        _ => {}
                    },
                }
                Ok(None)
            }
            FocusArea::Buttons => {
                match key.code {
                    KeyCode::Left => {
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                    }
                    KeyCode::Right => {
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                    }
                    KeyCode::Up | KeyCode::BackTab => self.prev_focus(),
                    KeyCode::Enter => {
                        if self.selected_button == 0 {
                            return Ok(self.try_confirm());
                        } else {
                            return Ok(Some(ModalResult::Cancelled));
                        }
                    }
                    _ => {}
                }
                Ok(None)
            }
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crossterm::event::{MouseButton, MouseEventKind};

        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        // Check field clicks
        for (i, area) in self.last_field_areas.iter().enumerate() {
            if mouse.row >= area.y
                && mouse.row < area.y + area.height
                && mouse.column >= area.x
                && mouse.column < area.x + area.width
            {
                self.focus = FocusArea::Field(i);
                if let ParamValue::Bool { checked } = &mut self.values[i] {
                    *checked = !*checked;
                }
                return Ok(None);
            }
        }

        // Check buttons
        if let Some(buttons_area) = self.last_buttons_area {
            if mouse.row >= buttons_area.y
                && mouse.row < buttons_area.y + buttons_area.height
                && mouse.column >= buttons_area.x
                && mouse.column < buttons_area.x + buttons_area.width
            {
                self.focus = FocusArea::Buttons;
                let run_text = format!("[ {} ]", termide_i18n::t().command_params_run());
                let cancel_text = format!("[ {} ]", termide_i18n::t().command_params_cancel());
                let total = run_text.len() + MODAL_BUTTON_SPACING as usize + cancel_text.len();
                let start = buttons_area.x + (buttons_area.width.saturating_sub(total as u16)) / 2;
                let run_end = start + run_text.len() as u16;
                let cancel_start = run_end + MODAL_BUTTON_SPACING;

                if mouse.column >= start && mouse.column < run_end {
                    return Ok(self.try_confirm());
                } else if mouse.column >= cancel_start
                    && mouse.column < cancel_start + cancel_text.len() as u16
                {
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
        }

        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        match self.focus {
            FocusArea::Field(idx) => match &mut self.values[idx] {
                ParamValue::Text { input } | ParamValue::Number { input } => {
                    input.paste(text);
                    true
                }
                ParamValue::Bool { .. } | ParamValue::Select { .. } => false,
            },
            FocusArea::Buttons => false,
        }
    }
}
