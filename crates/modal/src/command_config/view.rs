//! Modal trait implementation for the command-config modal: rendering,
//! key handling, mouse hit-testing, and paste routing.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
};
use termide_config::commands::{CommandCwd, CommandMode};
use termide_config::constants::MODAL_BUTTON_SPACING;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::SuggestionAction;

use crate::base::{button_style, drag_field, field_box, press_field_box, render_modal_block};
use crate::input_keys::{handle_input_key, InputKeyResult};
use crate::{centered_rect_with_size, Modal, ModalResult};

use super::{CommandConfigModal, CommandConfigResult, FocusArea};

impl Modal for CommandConfigModal {
    type Result = CommandConfigResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (modal_w, modal_h) = self.calculate_modal_size(area.width, area.height);
        let modal_area = centered_rect_with_size(modal_w, modal_h, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Build layout constraints
        let suggestions: Vec<String> = self.group_suggestion.suggestions().to_vec();
        let dropdown_height =
            if self.is_create() && self.group_suggestion.is_expanded() && !suggestions.is_empty() {
                suggestions.len().min(5) as u16 + 1
            } else {
                0
            };

        let mut constraints: Vec<Constraint> = Vec::new();

        if self.is_create() {
            constraints.push(Constraint::Length(3)); // group
            if dropdown_height > 0 {
                constraints.push(Constraint::Length(dropdown_height));
            }
            constraints.push(Constraint::Length(3)); // command
            constraints.push(Constraint::Length(3)); // display name
            constraints.push(Constraint::Length(2)); // mode selector
            constraints.push(Constraint::Length(3)); // hotkey
            if self.hotkey_error || self.hotkey_conflict.is_some() {
                constraints.push(Constraint::Length(1)); // error hint
            }
            constraints.push(Constraint::Length(1)); // cwd checkbox
            constraints.push(Constraint::Length(1)); // checkbox
            constraints.push(Constraint::Length(1)); // spacer
            constraints.push(Constraint::Length(1)); // buttons
        } else {
            constraints.push(Constraint::Length(3)); // group
            if dropdown_height > 0 {
                constraints.push(Constraint::Length(dropdown_height));
            }
            constraints.push(Constraint::Length(3)); // display name
            constraints.push(Constraint::Length(3)); // command
            constraints.push(Constraint::Length(2)); // mode selector
            constraints.push(Constraint::Length(3)); // hotkey
            if self.hotkey_error || self.hotkey_conflict.is_some() {
                constraints.push(Constraint::Length(1)); // error hint
            }
            constraints.push(Constraint::Length(1)); // cwd checkbox
            constraints.push(Constraint::Length(1)); // checkbox
            constraints.push(Constraint::Length(1)); // spacer
            constraints.push(Constraint::Length(1)); // buttons
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let mut chunk_idx = 0;
        let t = i18n::t();

        if self.is_create() {
            // 1. Group field
            self.last_group_text_area = Some(self.render_group_field(
                buf,
                chunks[chunk_idx],
                t.command_config_label_group(),
                theme,
            ));
            let group_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);
            self.last_group_field_area = Some(group_chunks[1]);
            chunk_idx += 1;

            // Group dropdown
            if dropdown_height > 0 {
                let dd_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(15), Constraint::Min(1)])
                    .split(chunks[chunk_idx]);

                self.last_group_dropdown_area = Some(dd_chunks[1]);
                let selected_idx = self.group_suggestion.selected_index();
                let items: Vec<ListItem> = suggestions
                    .iter()
                    .enumerate()
                    .map(|(idx, group)| {
                        let (prefix, style) = if idx == selected_idx {
                            (
                                "\u{25B6} ",
                                Style::default()
                                    .fg(theme.selected_fg)
                                    .bg(theme.selected_bg)
                                    .add_modifier(Modifier::BOLD),
                            )
                        } else {
                            ("  ", Style::default().fg(theme.fg))
                        };
                        ListItem::new(Line::from(Span::styled(
                            format!("{}{}", prefix, group),
                            style,
                        )))
                    })
                    .collect();

                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::LEFT | Borders::BOTTOM | Borders::RIGHT)
                            .border_style(Style::default().fg(theme.accented_fg)),
                    )
                    .style(Style::default().bg(theme.bg))
                    .render(dd_chunks[1], buf);
                chunk_idx += 1;
            } else {
                self.last_group_dropdown_area = None;
            }

            // 2. Display name field (пункт меню)
            self.last_display_name_text_area = Some(Self::render_labeled_input_field(
                buf,
                chunks[chunk_idx],
                t.command_config_label_display_name(),
                &mut self.display_name_input,
                self.focus == FocusArea::DisplayName,
                theme,
            ));
            chunk_idx += 1;

            // 3. Command field
            self.last_command_text_area = Some(Self::render_labeled_input_field(
                buf,
                chunks[chunk_idx],
                t.command_config_label_command(),
                &mut self.command_input,
                self.focus == FocusArea::Command,
                theme,
            ));
            chunk_idx += 1;
        } else {
            // 1. Group field
            self.last_group_text_area = Some(self.render_group_field(
                buf,
                chunks[chunk_idx],
                t.command_config_label_group(),
                theme,
            ));
            let group_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);
            self.last_group_field_area = Some(group_chunks[1]);
            chunk_idx += 1;

            if dropdown_height > 0 {
                let dd_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(15), Constraint::Min(1)])
                    .split(chunks[chunk_idx]);

                self.last_group_dropdown_area = Some(dd_chunks[1]);
                let selected_idx = self.group_suggestion.selected_index();
                let items: Vec<ListItem> = suggestions
                    .iter()
                    .enumerate()
                    .map(|(idx, group)| {
                        let (prefix, style) = if idx == selected_idx {
                            (
                                "\u{25B6} ",
                                Style::default()
                                    .fg(theme.selected_fg)
                                    .bg(theme.selected_bg)
                                    .add_modifier(Modifier::BOLD),
                            )
                        } else {
                            ("  ", Style::default().fg(theme.fg))
                        };
                        ListItem::new(Line::from(Span::styled(
                            format!("{}{}", prefix, group),
                            style,
                        )))
                    })
                    .collect();

                List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::LEFT | Borders::BOTTOM | Borders::RIGHT)
                            .border_style(Style::default().fg(theme.accented_fg)),
                    )
                    .style(Style::default().bg(theme.bg))
                    .render(dd_chunks[1], buf);
                chunk_idx += 1;
            } else {
                self.last_group_dropdown_area = None;
            }

            // 2. Display name field
            self.last_display_name_text_area = Some(Self::render_labeled_input_field(
                buf,
                chunks[chunk_idx],
                t.command_config_label_display_name(),
                &mut self.display_name_input,
                self.focus == FocusArea::DisplayName,
                theme,
            ));
            chunk_idx += 1;

            // 3. Command field
            self.last_command_text_area = Some(Self::render_labeled_input_field(
                buf,
                chunks[chunk_idx],
                t.command_config_label_command(),
                &mut self.command_input,
                self.focus == FocusArea::Command,
                theme,
            ));
            chunk_idx += 1;
        }

        // Mode selector (2 rows)
        {
            let mode_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);

            let is_focused = self.focus == FocusArea::Mode;

            // Mode label
            Paragraph::new(t.command_config_label_mode().to_string())
                .style(Style::default().fg(theme.fg))
                .alignment(Alignment::Right)
                .render(
                    Rect {
                        x: mode_chunks[0].x,
                        y: mode_chunks[0].y,
                        width: mode_chunks[0].width,
                        height: 1,
                    },
                    buf,
                );

            // Mode buttons
            let mode_area = mode_chunks[1];
            let modes = [
                CommandMode::Terminal,
                CommandMode::Background,
                CommandMode::Report,
            ];
            let mut x_offset = mode_area.x + 1;
            for m in &modes {
                let is_selected = self.command_mode == *m;
                let label = Self::mode_label(*m);
                let style = if is_selected && is_focused {
                    Style::default()
                        .fg(theme.bg)
                        .bg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD)
                } else if is_focused {
                    Style::default().fg(theme.fg)
                } else {
                    Style::default().fg(theme.disabled)
                };

                let display = format!(" {} ", label);
                if x_offset + display.len() as u16 <= mode_area.x + mode_area.width {
                    buf.set_string(x_offset, mode_area.y, &display, style);
                    x_offset += display.len() as u16;
                }
            }

            // Hint on second row
            if is_focused && mode_area.height > 1 {
                let hint = "\u{2190}/\u{2192} switch, 1/2/3 select";
                buf.set_string(
                    mode_area.x + 1,
                    mode_area.y + 1,
                    hint,
                    Style::default().fg(theme.disabled),
                );
            }

            self.last_mode_area = Some(mode_area);
            chunk_idx += 1;
        }

        // 4. Hotkey field
        self.last_hotkey_text_area = Some(Self::render_labeled_input_field(
            buf,
            chunks[chunk_idx],
            t.command_config_label_hotkey(),
            &mut self.hotkey_input,
            self.focus == FocusArea::Hotkey,
            theme,
        ));
        chunk_idx += 1;

        // Hotkey error hint
        if self.hotkey_error || self.hotkey_conflict.is_some() {
            let err_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);
            let message = self
                .hotkey_conflict
                .as_deref()
                .unwrap_or_else(|| t.command_config_hotkey_invalid());
            Paragraph::new(Span::styled(message, Style::default().fg(theme.error)))
                .render(err_chunks[1], buf);
            chunk_idx += 1;
        }

        // Hotkey hint (when focused, no error)
        if self.focus == FocusArea::Hotkey && !self.hotkey_error && self.hotkey_conflict.is_none() {
            // Could render a hint below the input
        }

        // Where the command runs: checked is the project root, unchecked the
        // folder of the focused panel.
        {
            let cwd_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(15), Constraint::Min(1)])
                .split(chunks[chunk_idx]);
            let is_focused = self.focus == FocusArea::CwdCheckbox;
            let cwd_text = format!(
                " [{}] {}",
                termide_ui::checkbox_mark(self.cwd == CommandCwd::Project),
                t.command_config_cwd_checkbox()
            );
            Paragraph::new(cwd_text)
                .style(Style::default().fg(if is_focused {
                    theme.accented_fg
                } else {
                    theme.fg
                }))
                .render(cwd_chunks[1], buf);
            self.last_cwd_area = Some(cwd_chunks[1]);
            chunk_idx += 1;
        }

        // Project checkbox
        let cb_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(15), Constraint::Min(1)])
            .split(chunks[chunk_idx]);

        let checkbox_char = termide_ui::checkbox_mark(self.is_project);
        let checkbox_style = if self.focus == FocusArea::ProjectCheckbox {
            Style::default().fg(theme.accented_fg)
        } else {
            Style::default().fg(theme.fg)
        };
        let checkbox_text = format!(
            " [{}] {}",
            checkbox_char,
            t.command_config_project_checkbox()
        );
        Paragraph::new(checkbox_text)
            .style(checkbox_style)
            .render(cb_chunks[1], buf);
        self.last_checkbox_area = Some(cb_chunks[1]);
        chunk_idx += 1;

        // Spacer
        chunk_idx += 1;

        // Buttons
        let buttons_area = chunks[chunk_idx];
        self.last_buttons_area = Some(buttons_area);
        let btn_count = self.button_count();

        let spans: Vec<Span> = (0..btn_count)
            .flat_map(|i| {
                let label = self.button_label(i);
                let is_sel = self.focus == FocusArea::Buttons && self.selected_button == i;
                let style = button_style(is_sel, theme);
                let display = format!("[ {} ]", label);
                let mut v = vec![Span::styled(display, style)];
                if i < btn_count - 1 {
                    v.push(Span::raw("    "));
                }
                v
            })
            .collect();

        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Center)
            .render(buttons_area, buf);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        // Escape
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            if self.is_create() && self.group_suggestion.is_expanded() {
                self.group_suggestion.collapse();
                return Ok(None);
            }
            return Ok(Some(ModalResult::Cancelled));
        }

        // Tab / Shift+Tab
        if key.code == KeyCode::Tab {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.prev_focus();
            } else if self.is_create()
                && self.focus == FocusArea::Group
                && !self.group_suggestion.suggestions().is_empty()
            {
                if self.group_suggestion.is_expanded() {
                    self.group_suggestion.collapse();
                } else {
                    self.group_suggestion.expand();
                }
            } else {
                self.next_focus();
            }
            return Ok(None);
        }
        if key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::BackTab {
            self.prev_focus();
            return Ok(None);
        }

        match self.focus {
            FocusArea::Group => match self.group_suggestion.handle_key(key) {
                SuggestionAction::Handled => {}
                SuggestionAction::Confirmed => {
                    self.group_suggestion.collapse();
                    self.next_focus();
                }
                SuggestionAction::Cancelled => {
                    self.group_suggestion.collapse();
                }
                SuggestionAction::TextModified => {}
                SuggestionAction::NotHandled => {
                    match handle_input_key(self.group_suggestion.input_mut(), key) {
                        InputKeyResult::Handled | InputKeyResult::TextModified => {}
                        InputKeyResult::NotHandled => match key.code {
                            KeyCode::Down => self.next_focus(),
                            KeyCode::Up => self.prev_focus(),
                            KeyCode::Enter => self.next_focus(),
                            _ => {}
                        },
                    }
                }
            },
            FocusArea::Command => match handle_input_key(&mut self.command_input, key) {
                InputKeyResult::Handled | InputKeyResult::TextModified => {}
                InputKeyResult::NotHandled => match key.code {
                    KeyCode::Down => self.next_focus(),
                    KeyCode::Up => self.prev_focus(),
                    KeyCode::Enter => self.next_focus(),
                    _ => {}
                },
            },
            FocusArea::DisplayName => match handle_input_key(&mut self.display_name_input, key) {
                InputKeyResult::Handled | InputKeyResult::TextModified => {}
                InputKeyResult::NotHandled => match key.code {
                    KeyCode::Down => self.next_focus(),
                    KeyCode::Up => self.prev_focus(),
                    KeyCode::Enter => self.next_focus(),
                    _ => {}
                },
            },
            FocusArea::Mode => match key.code {
                KeyCode::Right => {
                    self.command_mode = match self.command_mode {
                        CommandMode::Terminal => CommandMode::Background,
                        CommandMode::Background => CommandMode::Report,
                        CommandMode::Report => CommandMode::Terminal,
                    };
                }
                KeyCode::Left => {
                    self.command_mode = match self.command_mode {
                        CommandMode::Terminal => CommandMode::Report,
                        CommandMode::Background => CommandMode::Terminal,
                        CommandMode::Report => CommandMode::Background,
                    };
                }
                KeyCode::Char('1') => self.command_mode = CommandMode::Terminal,
                KeyCode::Char('2') => self.command_mode = CommandMode::Background,
                KeyCode::Char('3') => self.command_mode = CommandMode::Report,
                KeyCode::Down | KeyCode::Enter => self.next_focus(),
                KeyCode::Up => self.prev_focus(),
                _ => {}
            },
            FocusArea::CwdCheckbox => match key.code {
                KeyCode::Char(' ') => {
                    self.cwd = match self.cwd {
                        CommandCwd::Panel => CommandCwd::Project,
                        CommandCwd::Project => CommandCwd::Panel,
                    };
                }
                KeyCode::Down | KeyCode::Enter => self.next_focus(),
                KeyCode::Up => self.prev_focus(),
                _ => {}
            },
            FocusArea::Hotkey => match handle_input_key(&mut self.hotkey_input, key) {
                InputKeyResult::Handled | InputKeyResult::TextModified => self.validate_hotkey(),
                InputKeyResult::NotHandled => match key.code {
                    KeyCode::Down => self.next_focus(),
                    KeyCode::Up => self.prev_focus(),
                    KeyCode::Enter => {
                        self.validate_hotkey();
                        if !self.hotkey_error && self.hotkey_conflict.is_none() {
                            self.next_focus();
                        }
                    }
                    _ => {}
                },
            },
            FocusArea::ProjectCheckbox => match key.code {
                KeyCode::Char(' ') => self.is_project = !self.is_project,
                KeyCode::Down | KeyCode::Enter => self.next_focus(),
                KeyCode::Up => self.prev_focus(),
                _ => {}
            },
            FocusArea::Buttons => match key.code {
                KeyCode::Left => {
                    if self.selected_button > 0 {
                        self.selected_button -= 1;
                    } else {
                        self.selected_button = self.button_count() - 1;
                    }
                }
                KeyCode::Right => {
                    self.selected_button += 1;
                    if self.selected_button >= self.button_count() {
                        self.selected_button = 0;
                    }
                }
                KeyCode::Up | KeyCode::BackTab => {
                    self.selected_button = 0;
                    self.prev_focus();
                }
                KeyCode::Enter => {
                    if self.selected_button == self.button_count() - 1 {
                        return Ok(Some(ModalResult::Cancelled));
                    }
                    if let Some(result) = self.try_confirm() {
                        return Ok(Some(result));
                    }
                }
                _ => {}
            },
        }

        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let col = mouse.column;
        let row = mouse.row;

        use crossterm::event::{MouseButton, MouseEventKind};
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(field) = self.dragging {
                    let (input, area) = self.text_field_mut(field);
                    drag_field(input, area, &mouse);
                }
                return Ok(None);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = None;
                return Ok(None);
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return Ok(None),
        }

        // A press on a text field's box, its border included, focuses it and
        // places the cursor. The group's right end, where its arrow is,
        // opens the list instead.
        let has_groups = !self.group_suggestion.suggestions().is_empty();
        for field in [
            FocusArea::Group,
            FocusArea::Command,
            FocusArea::DisplayName,
            FocusArea::Hotkey,
        ] {
            let (input, area) = self.text_field_mut(field);
            let frame = field_box(area, field != FocusArea::Group || !has_groups);
            if press_field_box(input, area, frame, &mouse) {
                if field != FocusArea::Group {
                    self.group_suggestion.collapse();
                }
                self.focus = field;
                self.dragging = Some(field);
                return Ok(None);
            }
        }

        if let Some(area) = self.last_group_field_area {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                self.focus = FocusArea::Group;
                if !self.group_suggestion.suggestions().is_empty() {
                    if self.group_suggestion.is_expanded() {
                        self.group_suggestion.collapse();
                    } else {
                        self.group_suggestion.expand();
                    }
                }
                return Ok(None);
            }
        }
        if let Some(area) = self.last_group_dropdown_area {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                let idx = (row - area.y) as usize;
                if idx < self.group_suggestion.suggestions().len() {
                    self.group_suggestion.select_and_confirm(idx);
                }
                return Ok(None);
            }
        }

        if let Some(area) = self.last_cwd_area {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                self.focus = FocusArea::CwdCheckbox;
                self.cwd = match self.cwd {
                    CommandCwd::Panel => CommandCwd::Project,
                    CommandCwd::Project => CommandCwd::Panel,
                };
                return Ok(None);
            }
        }

        if let Some(area) = self.last_checkbox_area {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                self.focus = FocusArea::ProjectCheckbox;
                self.is_project = !self.is_project;
                return Ok(None);
            }
        }

        // Mode selector
        if let Some(area) = self.last_mode_area {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                self.focus = FocusArea::Mode;
                self.group_suggestion.collapse();
                let modes = [
                    CommandMode::Terminal,
                    CommandMode::Background,
                    CommandMode::Report,
                ];
                let mut x_offset = area.x + 1;
                for m in &modes {
                    let label = Self::mode_label(*m);
                    let w = label.len() as u16 + 2;
                    if col >= x_offset && col < x_offset + w {
                        self.command_mode = *m;
                        break;
                    }
                    x_offset += w + 1;
                }
                return Ok(None);
            }
        }

        // Buttons
        if let Some(area) = self.last_buttons_area {
            if col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                self.focus = FocusArea::Buttons;
                let btn_count = self.button_count();

                // Calculate button positions
                let mut button_starts: Vec<(u16, u16)> = Vec::new();
                let mut bx = area.x;
                let total_width: u16 = (0..btn_count)
                    .map(|i| {
                        let w = self.button_label(i).len() as u16 + 4; // "[ label ]"
                        button_starts.push((bx, w));
                        bx += w + MODAL_BUTTON_SPACING;
                        w + if i < btn_count - 1 {
                            MODAL_BUTTON_SPACING
                        } else {
                            0
                        }
                    })
                    .sum();

                // Recalculate with centering
                let start_x = area.x + (area.width.saturating_sub(total_width)) / 2;
                button_starts.clear();
                let mut cx = start_x;
                for i in 0..btn_count {
                    let label = self.button_label(i);
                    let w = label.len() as u16 + 4;
                    button_starts.push((cx, w));
                    cx += w + MODAL_BUTTON_SPACING;
                }

                for (i, (sx, w)) in button_starts.iter().enumerate() {
                    if col >= *sx && col < sx + w {
                        self.selected_button = i;
                        if i == self.button_count() - 1 {
                            return Ok(Some(ModalResult::Cancelled));
                        }
                        if let Some(result) = self.try_confirm() {
                            return Ok(Some(result));
                        }
                        break;
                    }
                }
                return Ok(None);
            }
        }

        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        match self.focus {
            FocusArea::Group => {
                self.group_suggestion.input_mut().paste(text);
                true
            }
            FocusArea::Command => {
                self.command_input.paste(text);
                true
            }
            FocusArea::DisplayName => {
                self.display_name_input.paste(text);
                true
            }
            FocusArea::Hotkey => {
                self.hotkey_input.paste(text);
                self.validate_hotkey();
                true
            }
            _ => false,
        }
    }
}
