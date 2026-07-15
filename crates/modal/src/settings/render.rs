//! Settings modal rendering: sidebar, content fields, LSP edit form,
//! keybindings list, and the action-button bar.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::base::button_style;

use super::fields::{fields_for_tab, get_field_value, ContentRow, FieldDescriptor, FieldType};
use super::kb::{get_kb_value, kb_binding_names, KB_SECTIONS};
use super::{
    button_labels, FocusArea, KbMode, LspMode, SettingsModal, SettingsTab, SidebarRow, BUTTON_RESET,
};

/// Truncate `s` to at most `max_chars` Unicode scalar values, safe for UTF-8 slicing.
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
                SidebarRow::KbChild(i) => ("   ".to_string(), KB_SECTIONS[i].to_string()),
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

        let spacing = 4;
        let labels = button_labels(self.project_override_active);
        let total_label_len: usize = labels.iter().map(|l| l.len() + 4).sum::<usize>() // "[ label ]"
            + spacing * (labels.len().saturating_sub(1));
        let mut x = area.x as usize + (area.width as usize).saturating_sub(total_label_len) / 2;

        for (i, label) in labels.iter().enumerate() {
            let is_selected = self.focus == FocusArea::Buttons && self.selected_button == i;
            let style = if i == BUTTON_RESET && !self.dirty && !is_selected {
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
            if i < labels.len() - 1 {
                for _ in 0..spacing {
                    if x < (area.x as usize) + area.width as usize {
                        buf[(x as u16, by)]
                            .set_char(' ')
                            .set_style(Style::default());
                        x += 1;
                    }
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
            let section_name = KB_SECTIONS.get(self.kb_section).copied().unwrap_or("");
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
        let label_width = 32;
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

                    let display_value = if value.len() > max_value_width && max_value_width > 2 {
                        format!("{}…", &value[..max_value_width - 1])
                    } else {
                        value
                    };
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
                        let display_cmd = if cmd_info.len() > max_cmd && max_cmd > 2 {
                            format!("{}…", &cmd_info[..max_cmd - 1])
                        } else {
                            cmd_info
                        };
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
            let display_val = if value.len() > max_val && max_val > 2 {
                format!("{}…", &value[..max_val - 1])
            } else {
                value
            };
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
                let display_val = if val.len() > max_val && max_val > 2 {
                    format!("{}…", &val[..max_val - 1])
                } else {
                    val
                };
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
