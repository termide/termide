//! Settings modal rendering: sidebar, content fields, LSP edit form,
//! keybindings list, and the action-button bar.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use termide_i18n as i18n;
use termide_theme::Theme;
use unicode_width::UnicodeWidthStr;

use crate::base::button_style;

use super::fields::{fields_for_tab, get_field_value, ContentRow, FieldDescriptor, FieldType};
use super::kb::{get_kb_value, kb_binding_names};
use super::{
    button_labels, button_spans, FocusArea, KbMode, LspMode, SettingsModal, SettingsTab,
    SidebarRow, BUTTON_RESET, ENUM_PICKER_MAX_VISIBLE,
};

/// Width reserved for field labels, and therefore where values line up.
///
/// Measured from the labels actually in use rather than fixed at 32 columns:
/// a translated label — "Всегда отсоединяемая сессия (Unix)" is 34 — would
/// otherwise be truncated or run under its own value. Capped at half the
/// content width so a long label cannot squeeze the values out.
fn label_column_width(tab: SettingsTab, area_width: u16) -> usize {
    const MIN: usize = 24;
    /// Blank columns between the longest label and the values, so that a label
    /// filling the column does not end up touching its own value.
    const GAP: usize = 2;

    let widest = fields_for_tab(tab)
        .iter()
        .map(|d| d.label.width())
        .max()
        .unwrap_or(MIN);

    // Capped by what the values need, not by an arbitrary half: with a hard
    // half-width cap the column landed exactly on the longest label at the
    // sizes this modal actually opens at, leaving no gap at all.
    const MIN_VALUE_WIDTH: usize = 12;
    let cap = (area_width as usize)
        .saturating_sub(MIN_VALUE_WIDTH)
        .max(MIN);
    (widest + GAP).clamp(MIN, cap)
}

/// Truncate `s` to at most `max_chars` Unicode scalar values, safe for UTF-8 slicing.
/// `value` cut to `max` display columns with a trailing `…` when it does not
/// fit. Widths, not bytes: a byte cut lands inside a multi-byte character
/// (a Cyrillic label, say) and panics.
fn fit_width(value: String, max: usize) -> String {
    if max > 2 {
        termide_ui::path_utils::truncate_right(&value, max)
    } else {
        value
    }
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    if let Some((idx, _)) = s.char_indices().nth(max_chars) {
        &s[..idx]
    } else {
        s
    }
}

impl SettingsModal {
    // ---- Rendering helpers ----

    /// Render the left sidebar with section leaves and the expandable Keybindings group.
    pub(super) fn render_sidebar(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.width < 3 || area.height < 1 {
            return;
        }
        let width = area.width as usize;
        let visible = area.height as usize;
        self.clamp_sidebar_scroll(visible);

        let rows = self.visible_sidebar_rows();
        let focused = self.focus == FocusArea::Sidebar;
        // Only show the cursor highlight when the sidebar actually has focus.
        // When the user moves focus to Content/Buttons, the section title is
        // already shown in the content area header, so keeping a highlighted
        // row here would only confuse where the real focus lives.
        let active_cursor = if focused {
            Some(self.sidebar_cursor)
        } else {
            None
        };

        let kb_label = Self::kb_group_label();

        for row_i in 0..visible {
            let idx = self.sidebar_scroll + row_i;
            if idx >= rows.len() {
                break;
            }
            let y = area.y + row_i as u16;
            let is_selected = active_cursor == Some(idx);

            if is_selected {
                for x in area.x..area.x + area.width {
                    buf[(x, y)]
                        .set_style(Style::default().bg(theme.selected_bg).fg(theme.selected_fg));
                }
            }

            let (prefix, label): (String, String) = match rows[idx] {
                SidebarRow::Leaf(tab) => (" ".to_string(), tab.label()),
                SidebarRow::KbGroupHeader => (
                    if self.keybindings_expanded {
                        "▼ "
                    } else {
                        "▶ "
                    }
                    .to_string(),
                    kb_label.clone(),
                ),
                SidebarRow::KbChild(i) => ("   ".to_string(), Self::kb_section_label(i)),
            };

            let style = if is_selected {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else if matches!(rows[idx], SidebarRow::KbGroupHeader) {
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };

            let full = format!("{}{}", prefix, label);
            let max_w = width.saturating_sub(1);
            let display = if full.chars().count() > max_w && max_w > 1 {
                let mut s: String = full.chars().take(max_w.saturating_sub(1)).collect();
                s.push('…');
                s
            } else {
                full
            };
            buf.set_string(area.x + 1, y, &display, style);
        }

        self.last_sidebar_area = Some(area);
    }

    /// Render the bottom button bar using standard button style.
    pub(super) fn render_buttons(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.width < 4 {
            return;
        }
        let y = area.y;

        // Separator line
        for x in area.x..area.x + area.width {
            buf[(x, y)]
                .set_char('─')
                .set_style(Style::default().fg(theme.disabled));
        }

        // Buttons on the next row
        let by = y + 1;
        if by >= area.y + area.height {
            self.last_buttons_area = Some(area);
            return;
        }

        let labels = button_labels(self.project_override_active);
        let spans = button_spans(area.x, area.width, &labels);

        for (i, label) in labels.iter().enumerate() {
            let mut x = spans[i].0;
            let is_selected = self.focus == FocusArea::Buttons && self.selected_button == i;
            let style = if i == BUTTON_RESET && !self.reset_available && !is_selected {
                Style::default().fg(theme.disabled)
            } else {
                button_style(is_selected, theme)
            };
            let btn = format!("[ {} ]", label);
            for ch in btn.chars() {
                if x < (area.x as usize) + area.width as usize {
                    buf[(x as u16, by)].set_char(ch).set_style(style);
                    x += 1;
                }
            }
        }

        self.last_buttons_area = Some(Rect::new(area.x, by, area.width, 1));
    }

    /// Render a section title at the top of `area`. Returns the remaining area
    /// below the title (title row + blank row consumed).
    fn render_section_title(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str) -> Rect {
        if area.height < 3 {
            return area;
        }
        buf.set_string(
            area.x + 2,
            area.y,
            title,
            Style::default()
                .fg(theme.accented_fg)
                .add_modifier(Modifier::BOLD),
        );
        Rect::new(area.x, area.y + 2, area.width, area.height - 2)
    }

    /// Render content area with field rows.
    pub(super) fn render_content(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        // LSP server edit form — takes over the entire content area
        if self.active_tab == SettingsTab::Lsp && self.lsp_mode == LspMode::ServerEdit {
            let title = format!(
                "{} › {}",
                SettingsTab::Lsp.label(),
                i18n::t().settings_lsp_add_server()
            );
            let inner = Self::render_section_title(area, buf, theme, &title);
            self.render_lsp_edit_form(inner, buf, theme);
            self.last_content_area = Some(inner);
            return;
        }

        // Keybindings tab — dedicated renderer (renders its own title).
        if self.active_tab == SettingsTab::Keybindings {
            let section_name = Self::kb_section_label(self.kb_section);
            let title = format!("{} › {}", Self::kb_group_label(), section_name);
            let inner = Self::render_section_title(area, buf, theme, &title);
            self.render_keybindings(inner, buf, theme);
            self.last_content_area = Some(inner);
            return;
        }

        // Regular tab: title + grouped fields.
        let title = self.active_tab.label();
        let area = Self::render_section_title(area, buf, theme, &title);

        let rows = self.content_rows();
        if rows.is_empty() {
            self.last_content_area = Some(area);
            return;
        }

        let visible_rows = area.height as usize;
        self.clamp_scroll(visible_rows);

        let fields = fields_for_tab(self.active_tab);
        let label_width = label_column_width(self.active_tab, area.width);
        let value_x = area.x as usize + 2 + label_width;
        let max_value_width = (area.x as usize + area.width as usize).saturating_sub(value_x);

        for row_off in 0..visible_rows {
            let row_idx = self.content_scroll + row_off;
            if row_idx >= rows.len() {
                break;
            }
            let y = area.y + row_off as u16;
            let row = rows[row_idx];
            let is_focused = self.focus == FocusArea::Content
                && row_idx == self.field_cursor
                && row.is_selectable();

            if is_focused {
                for x in area.x..area.x + area.width {
                    buf[(x, y)]
                        .set_style(Style::default().bg(theme.selected_bg).fg(theme.selected_fg));
                }
            }

            match row {
                ContentRow::Header(label) => {
                    let text = format!("── {} ──", label);
                    buf.set_string(
                        area.x + 2,
                        y,
                        &text,
                        Style::default()
                            .fg(theme.disabled)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                ContentRow::Spacer => {
                    // Intentionally blank row between groups.
                }
                ContentRow::Field(field_idx) => {
                    let Some(desc) = fields.get(field_idx) else {
                        continue;
                    };
                    let label_style = if is_focused {
                        Style::default()
                            .fg(theme.selected_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };

                    let label_text = truncate_str(desc.label, label_width);
                    buf.set_string(area.x + 2, y, label_text, label_style);

                    let value = if self.editing && is_focused {
                        format!("{}_", self.edit_buffer)
                    } else {
                        self.format_field_value(desc, field_idx)
                    };

                    let value_style = if is_focused {
                        Style::default().fg(theme.selected_fg)
                    } else {
                        match desc.field_type {
                            FieldType::Bool | FieldType::Enum => {
                                Style::default().fg(theme.accented_fg)
                            }
                            _ => Style::default().fg(theme.fg),
                        }
                    };

                    let display_value = fit_width(value, max_value_width);
                    buf.set_string(value_x as u16, y, &display_value, value_style);
                }
                ContentRow::LspAddServer => {
                    let label = i18n::t().settings_lsp_add_server();
                    let style = if is_focused {
                        Style::default()
                            .fg(theme.selected_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.accented_fg)
                    };
                    buf.set_string(area.x + 2, y, label, style);
                }
                ContentRow::LspServer(server_idx) => {
                    if server_idx >= self.lsp_server_keys.len() {
                        continue;
                    }
                    let lang = &self.lsp_server_keys[server_idx];
                    let label_style = if is_focused {
                        Style::default()
                            .fg(theme.selected_fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let label = format!("• {}", lang);
                    buf.set_string(area.x + 2, y, &label, label_style);

                    if let Some(srv) = self.config.lsp.servers.get(lang) {
                        let cmd_info = format!("{} {}", srv.command, srv.args.join(" "));
                        let cmd_style = if is_focused {
                            Style::default().fg(theme.selected_fg)
                        } else {
                            Style::default().fg(theme.disabled)
                        };
                        let max_cmd = max_value_width.saturating_sub(12);
                        let display_cmd = fit_width(cmd_info, max_cmd);
                        buf.set_string(value_x as u16, y, &display_cmd, cmd_style);

                        let del_label = if is_focused { "[Del]" } else { "" };
                        let del_x = (area.x as usize + area.width as usize).saturating_sub(6);
                        buf.set_string(
                            del_x as u16,
                            y,
                            del_label,
                            Style::default().fg(theme.accented_fg),
                        );
                    }
                }
            }
        }

        self.last_content_area = Some(area);
    }

    /// Render the LSP server edit form.
    fn render_lsp_edit_form(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let labels = [
            "Language:",
            "Command:",
            "Args (comma-sep):",
            "Root markers (comma-sep):",
        ];
        let x = area.x as usize + 4;
        let val_x = x + 26;
        let max_val = (area.x as usize + area.width as usize)
            .saturating_sub(val_x)
            .saturating_sub(2);

        for (i, label) in labels.iter().enumerate() {
            let y = area.y + 1 + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let is_focused = self.lsp_edit_cursor == i;
            let label_style = if is_focused {
                Style::default()
                    .fg(theme.accented_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            buf.set_string(x as u16, y, label, label_style);

            let value = if is_focused {
                format!("{}_", self.lsp_edit_fields[i])
            } else {
                self.lsp_edit_fields[i].clone()
            };
            let display_val = fit_width(value, max_val);
            let val_style = if is_focused {
                Style::default().fg(theme.accented_fg)
            } else {
                Style::default().fg(theme.fg)
            };
            buf.set_string(val_x as u16, y, &display_val, val_style);
        }

        // Hint line
        let hint_y = area.y + 6;
        if hint_y < area.y + area.height {
            let hint = "Enter=save  Esc=cancel  Tab=next field";
            buf.set_string(x as u16, hint_y, hint, Style::default().fg(theme.disabled));
        }
    }

    /// Format a field value for display, with visual indicators.
    fn format_field_value(&self, desc: &FieldDescriptor, index: usize) -> String {
        let raw = get_field_value(&self.config, self.active_tab, index);
        match desc.field_type {
            FieldType::Bool => {
                if raw == "true" {
                    "[✓]".to_string()
                } else {
                    "[✗]".to_string()
                }
            }
            FieldType::Enum => {
                format!("< {} >", raw)
            }
            FieldType::OptionalText => raw,
            _ => raw,
        }
    }

    fn render_keybindings(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.height == 0 || area.width < 4 {
            return;
        }

        let list_y = area.y;
        let list_h = area.height.saturating_sub(1); // leave 1 row for hint

        let names = kb_binding_names(self.kb_section);
        let visible = list_h as usize;
        if self.kb_cursor < self.kb_scroll {
            self.kb_scroll = self.kb_cursor;
        }
        if visible > 0 && self.kb_cursor >= self.kb_scroll + visible {
            self.kb_scroll = self.kb_cursor - visible + 1;
        }

        let label_width = 28.min(area.width as usize / 2);

        for row in 0..visible {
            let idx = self.kb_scroll + row;
            if idx >= names.len() {
                break;
            }
            let y = list_y + row as u16;
            let is_focused_row = self.focus == FocusArea::Content && self.kb_cursor == idx;
            let is_capturing = self.kb_mode == KbMode::Capturing && self.kb_cursor == idx;

            if is_focused_row || is_capturing {
                for x in area.x..area.x + area.width {
                    buf[(x, y)]
                        .set_style(Style::default().bg(theme.selected_bg).fg(theme.selected_fg));
                }
            }

            let name = names[idx];
            let label_style = if is_focused_row || is_capturing {
                Style::default()
                    .fg(theme.selected_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let display_name = if name.len() > label_width {
                let truncated = truncate_str(name, label_width - 1);
                format!("{truncated}…")
            } else {
                name.to_string()
            };
            buf.set_string(area.x + 2, y, &display_name, label_style);

            let val_x = area.x + 2 + label_width as u16;
            if is_capturing {
                buf.set_string(
                    val_x,
                    y,
                    i18n::t().settings_kb_press_key(),
                    Style::default()
                        .fg(theme.accented_fg)
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                let val = get_kb_value(&self.config, self.kb_section, name);
                let val_style = if is_focused_row {
                    Style::default().fg(theme.selected_fg)
                } else {
                    Style::default().fg(theme.accented_fg)
                };
                let max_val = (area.x + area.width).saturating_sub(val_x) as usize;
                let display_val = fit_width(val, max_val);
                buf.set_string(val_x, y, &display_val, val_style);
            }
        }

        // Hint line at the bottom of the area. If there is a fresh
        // capture message (e.g. conflict warning), show it instead of
        // the static hint — it's more actionable.
        let hint_y = area.y + area.height - 1;
        let it = i18n::t();
        if let Some(msg) = &self.kb_capture_message {
            buf.set_string(area.x + 2, hint_y, msg, Style::default().fg(theme.warning));
        } else {
            let hint = match self.kb_mode {
                KbMode::Bindings => it.settings_kb_hint_bindings(),
                KbMode::Capturing => it.settings_kb_hint_capturing(),
            };
            buf.set_string(
                area.x + 2,
                hint_y,
                hint,
                Style::default().fg(theme.disabled),
            );
        }
    }
}

impl SettingsModal {
    /// Draw the open enum dropdown over the form.
    ///
    /// Anchored under the field it belongs to, and flipped above it when there
    /// is no room below, so the list never runs off the modal.
    pub(super) fn render_enum_picker(&mut self, content: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some(picker) = self.enum_picker.clone() else {
            return;
        };
        let Some(options) = self.enum_options_for(picker.field_index) else {
            self.enum_picker = None;
            return;
        };
        let Some(inner) = self.last_content_area else {
            return;
        };

        // Row the field occupies on screen, so the list can hang off it.
        let row_index = self
            .content_rows()
            .iter()
            .position(|row| matches!(row, ContentRow::Field(i) if *i == picker.field_index));
        let Some(row_index) = row_index else {
            return;
        };
        let field_y = inner.y as usize + row_index.saturating_sub(self.content_scroll);

        let visible = ENUM_PICKER_MAX_VISIBLE.min(options.labels.len());
        let height = visible as u16 + 2; // borders
        let width = options
            .labels
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(10)
            .max(12) as u16
            + 4;
        let width = width.min(inner.width.max(12));

        // Below the field if it fits, above it otherwise.
        let below_y = field_y as u16 + 1;
        let y = if below_y + height <= content.y + content.height {
            below_y
        } else {
            (field_y as u16).saturating_sub(height)
        };
        // Aligned with the value column the form uses, so the list drops out
        // of the value it replaces rather than out of the label.
        let value_column = 2 + label_column_width(self.active_tab, inner.width) as u16;
        let x = inner.x + value_column.min(inner.width.saturating_sub(width));
        let rect = Rect::new(x, y, width, height);

        Clear.render(rect, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accented_fg))
            .style(Style::default().bg(theme.bg));
        let list_area = block.inner(rect);
        block.render(rect, buf);

        for (line, option_index) in (picker.scroll..options.labels.len())
            .take(visible)
            .enumerate()
        {
            let selected = option_index == picker.cursor;
            let is_current = options.current == Some(option_index);
            let style = if selected {
                Style::default().fg(theme.bg).bg(theme.accented_fg)
            } else {
                Style::default().fg(theme.fg).bg(theme.bg)
            };
            // A dot marks the value actually stored, which is not the same as
            // the one under the cursor while the user is browsing.
            let marker = if is_current { "●" } else { " " };
            let label = format!(" {marker} {}", options.labels[option_index]);
            let padded = format!("{label:<width$}", width = list_area.width as usize);
            buf.set_stringn(
                list_area.x,
                list_area.y + line as u16,
                padded,
                list_area.width as usize,
                style,
            );
        }

        if let Some(picker) = self.enum_picker.as_mut() {
            picker.area = Some(rect);
        }
    }
}

#[cfg(test)]
mod label_column_tests {
    use super::*;

    /// Values must not butt up against the longest label — "Resource monitor
    /// interval (ms)" is 31 columns and used to render as
    /// `…interval (ms)2000` once the column was measured rather than fixed.
    #[test]
    fn the_longest_label_still_leaves_a_gap() {
        for tab in [
            SettingsTab::General,
            SettingsTab::Editor,
            SettingsTab::FileManager,
            SettingsTab::Terminal,
            SettingsTab::Lsp,
            SettingsTab::Logging,
            SettingsTab::Vfs,
        ] {
            let width = label_column_width(tab, 100);
            let widest = fields_for_tab(tab)
                .iter()
                .map(|d| d.label.width())
                .max()
                .unwrap_or(0);
            assert!(
                width > widest,
                "{tab:?}: column {width} must exceed the longest label {widest}"
            );
        }
    }

    /// A narrow modal must not let labels eat the values.
    #[test]
    fn the_column_is_capped_on_narrow_layouts() {
        let width = label_column_width(SettingsTab::General, 40);
        assert!(width <= 40 - 12, "got {width}");
    }

    /// At the widths this modal actually opens at, the gap must survive the
    /// cap — this is what failed in practice while the unit test passed at
    /// width 100.
    #[test]
    fn the_gap_survives_realistic_widths() {
        let widest = fields_for_tab(SettingsTab::General)
            .iter()
            .map(|d| d.label.width())
            .max()
            .unwrap_or(0);

        for area in [56u16, 60, 62, 70, 80, 100] {
            let width = label_column_width(SettingsTab::General, area);
            assert!(
                width > widest,
                "area {area}: column {width} leaves no gap after a {widest}-column label"
            );
        }
    }

    #[test]
    fn a_long_value_is_cut_by_width_not_bytes() {
        // A byte cut at column 40 would land inside a Cyrillic letter.
        let value = "< ask — спрашивать перед каждым изменением и командой >".to_string();
        let cut = fit_width(value.clone(), 40);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(UnicodeWidthStr::width(cut.as_str()) <= 40, "{cut}");
        // A value that fits is left as it is.
        assert_eq!(fit_width("ask".into(), 40), "ask");
    }
}
