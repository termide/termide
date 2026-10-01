//! Checklist modal: checkboxes under group headings, applied together.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_core::{ChecklistButton, ChecklistGroup, ChecklistItem, ChecklistOutcome};
use termide_theme::Theme;

use crate::{
    base::render_modal_block, centered_rect_with_size, fit_modal_width, is_click_outside, Modal,
    ModalResult,
};

/// Rows the list shows at most before it scrolls.
const MAX_ROWS: usize = 20;

/// The disclosure arrows, as the file trees draw them.
const ARROW_COLLAPSED: &str = if cfg!(windows) { "►" } else { "▶" };
const ARROW_EXPANDED: &str = "▼";

/// Columns of a row before its checkbox: a space, then the arrow and a space
/// on a heading, or as much indent on an item of a group.
const LEAD: usize = 3;

/// A run of consecutive items with the same group. One with no name has no
/// heading, and its items are always shown; one with no items is a heading
/// alone, with nothing to open.
#[derive(Debug, Clone)]
struct Group {
    name: String,
    items: std::ops::Range<usize>,
    expanded: bool,
    note: String,
    buttons: Vec<ChecklistButton>,
}

/// One row of the list: a group's heading, or the item at an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Heading(usize),
    Item(usize),
}

/// Checkboxes listed under group headings, every group collapsed at first:
/// its heading shows the group's mark and how many of its items are on.
/// `→` (or a click on the arrow) opens a group and `←` closes it, or goes
/// from an item to its heading. `Space` (or a click) toggles the item under
/// the cursor, or every item of the group when the cursor is on its heading;
/// `Enter` applies them all, `Esc` leaves everything as it was. A locked item
/// is shown greyed and keeps its state. A heading's buttons sit at its right
/// end; a click on one, or its key on the heading, applies the list as
/// `Enter` does and names the button.
#[derive(Debug)]
pub struct ChecklistModal {
    title: String,
    prompt: String,
    items: Vec<ChecklistItem>,
    groups: Vec<Group>,
    rows: Vec<Row>,
    /// Index into `rows` of the row under the cursor.
    cursor: usize,
    /// First row shown.
    scroll: usize,
    /// Screen rect of the modal from the last render, for clicks beside it.
    modal_area: Option<Rect>,
    /// Screen rect of the list from the last render, for clicks.
    list_area: Option<Rect>,
    /// The buttons drawn by the last render: the row, the columns, the id.
    button_areas: Vec<(usize, std::ops::Range<u16>, String)>,
}

impl ChecklistModal {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        prompt: impl Into<String>,
        items: Vec<ChecklistItem>,
        headings: Vec<ChecklistGroup>,
    ) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            match groups.last_mut() {
                Some(group) if group.name == item.group => group.items.end = index + 1,
                _ => groups.push(Group {
                    name: item.group.clone(),
                    items: index..index + 1,
                    expanded: false,
                    note: String::new(),
                    buttons: Vec::new(),
                }),
            }
        }
        for heading in headings {
            let group = match groups.iter_mut().find(|g| g.name == heading.name) {
                Some(group) => group,
                None => {
                    groups.push(Group {
                        name: heading.name.clone(),
                        items: items.len()..items.len(),
                        expanded: false,
                        note: String::new(),
                        buttons: Vec::new(),
                    });
                    groups.last_mut().expect("just pushed")
                }
            };
            group.note = heading.note;
            group.buttons = heading.buttons;
        }
        let mut modal = Self {
            title: title.into(),
            prompt: prompt.into(),
            items,
            groups,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            modal_area: None,
            list_area: None,
            button_areas: Vec::new(),
        };
        modal.rebuild_rows();
        modal
    }

    /// What `Enter` applies, with the button that closed the list, if any.
    fn outcome(&self, pressed: Option<String>) -> ChecklistOutcome {
        ChecklistOutcome {
            checked: self.checked(),
            pressed,
        }
    }

    /// The keys of the items checked now, in list order.
    #[must_use]
    pub fn checked(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.checked)
            .map(|item| item.key.clone())
            .collect()
    }

    /// The rows the groups show now; the cursor stays on the row it was on,
    /// or on the heading of the group that closed under it.
    fn rebuild_rows(&mut self) {
        let current = self.rows.get(self.cursor).copied();
        self.rows.clear();
        for (index, group) in self.groups.iter().enumerate() {
            let headed = !group.name.is_empty();
            if headed {
                self.rows.push(Row::Heading(index));
            }
            if !headed || group.expanded {
                self.rows.extend(group.items.clone().map(Row::Item));
            }
        }
        let target = match current {
            Some(Row::Item(item)) if !self.rows.contains(&Row::Item(item)) => {
                self.group_of(item).map(Row::Heading)
            }
            other => other,
        };
        self.cursor = target
            .and_then(|row| self.rows.iter().position(|r| *r == row))
            .unwrap_or(0);
    }

    /// The group an item belongs to, when it has a heading.
    fn group_of(&self, item: usize) -> Option<usize> {
        self.groups
            .iter()
            .position(|group| group.items.contains(&item))
            .filter(|&group| !self.groups[group].name.is_empty())
    }

    fn set_expanded(&mut self, group: usize, expanded: bool) {
        if self.groups[group].expanded != expanded {
            self.groups[group].expanded = expanded;
            self.rebuild_rows();
        }
    }

    fn toggle(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            if item.enabled {
                item.checked = !item.checked;
            }
        }
    }

    /// Whether any item of `group` can be toggled.
    fn group_enabled(&self, group: usize) -> bool {
        self.items[self.groups[group].items.clone()]
            .iter()
            .any(|item| item.enabled)
    }

    /// The heading's mark: checked when every item of the group is, empty
    /// when none is, partial otherwise.
    fn group_mark(&self, group: usize) -> &'static str {
        let items = &self.items[self.groups[group].items.clone()];
        if items.is_empty() {
            ""
        } else if items.iter().all(|item| item.checked) {
            termide_ui::checkbox(true)
        } else if items.iter().any(|item| item.checked) {
            "[-]"
        } else {
            termide_ui::checkbox(false)
        }
    }

    /// The heading's text after the arrow: the mark, the name and how many
    /// of the group's items are on, which a collapsed group shows nothing else
    /// of.
    fn heading_text(&self, group: usize) -> String {
        let range = self.groups[group].items.clone();
        let on = self.items[range.clone()]
            .iter()
            .filter(|item| item.checked)
            .count();
        let Group { name, note, .. } = &self.groups[group];
        let mut text = if range.is_empty() {
            // Nothing to switch: no mark, no count, room for the remark.
            name.clone()
        } else {
            format!("{} {name}  {on}/{}", self.group_mark(group), range.len())
        };
        if !note.is_empty() {
            text.push_str(" — ");
            text.push_str(note);
        }
        text
    }

    /// The buttons as drawn, `[icon]` after `[icon]`.
    fn buttons_text(buttons: &[ChecklistButton]) -> String {
        buttons.iter().map(|b| format!("[{}]", b.icon)).collect()
    }

    /// Toggles the row under the cursor: an item, or all the unlocked items
    /// of a group — on unless every one of them is on already.
    fn toggle_row(&mut self, row: usize) {
        match self.rows.get(row).copied() {
            Some(Row::Item(index)) => self.toggle(index),
            Some(Row::Heading(group)) => {
                let range = self.groups[group].items.clone();
                let items = &mut self.items[range];
                let on = !items
                    .iter()
                    .filter(|item| item.enabled)
                    .all(|item| item.checked);
                for item in items.iter_mut().filter(|item| item.enabled) {
                    item.checked = on;
                }
            }
            None => {}
        }
    }

    /// `→`: open the group under the cursor, or step into it when it is open.
    fn expand(&mut self) {
        if let Some(Row::Heading(group)) = self.rows.get(self.cursor).copied() {
            if self.groups[group].items.is_empty() {
                return;
            }
            if self.groups[group].expanded {
                self.move_by(1);
            } else {
                self.set_expanded(group, true);
            }
        }
    }

    /// `←`: close the group under the cursor, or go from an item up to its
    /// group's heading.
    fn collapse(&mut self) {
        match self.rows.get(self.cursor).copied() {
            Some(Row::Heading(group)) => self.set_expanded(group, false),
            Some(Row::Item(item)) => {
                if let Some(group) = self.group_of(item) {
                    if let Some(row) = self.rows.iter().position(|r| *r == Row::Heading(group)) {
                        self.cursor = row;
                    }
                }
            }
            None => {}
        }
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last.max(0)) as usize;
    }

    /// The text of an item's row after the checkbox.
    fn item_text(item: &ChecklistItem) -> String {
        if item.note.is_empty() {
            item.label.clone()
        } else {
            format!("{} — {}", item.label, item.note)
        }
    }

    /// The modal's width from its title and rows; the prompt is wrapped to
    /// it rather than widening it.
    fn modal_width(&self, screen_width: u16) -> u16 {
        let title = UnicodeWidthStr::width(self.title.as_str()) as u16 + 2;
        // The lead and "[✓] " before a row's text, a space after it; the
        // widths are those of every group open, so opening one does not
        // resize the modal.
        let items = self
            .items
            .iter()
            .map(|item| {
                UnicodeWidthStr::width(Self::item_text(item).as_str()) as u16 + LEAD as u16 + 5
            })
            .chain((0..self.groups.len()).map(|group| {
                let buttons = Self::buttons_text(&self.groups[group].buttons);
                let buttons = if buttons.is_empty() {
                    0
                } else {
                    UnicodeWidthStr::width(buttons.as_str()) as u16 + 1
                };
                UnicodeWidthStr::width(self.heading_text(group).as_str()) as u16
                    + LEAD as u16
                    + 1
                    + buttons
            }))
            .max()
            .unwrap_or(0);
        fit_modal_width(title.max(items), screen_width)
    }
}

impl Modal for ChecklistModal {
    /// The keys left checked, and the button pressed.
    type Result = ChecklistOutcome;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let width = self.modal_width(area.width);
        let prompt: Vec<String> = if self.prompt.is_empty() {
            Vec::new()
        } else {
            // One column of padding on either side, as the rows have.
            termide_ui::choice_form::wrap(&self.prompt, width.saturating_sub(4) as usize)
        };
        let prompt_lines = prompt.len() as u16;
        let screen_rows = area.height.saturating_sub(4 + prompt_lines) as usize;
        let visible = self.rows.len().min(MAX_ROWS).min(screen_rows.max(1));
        // Keep the cursor's row, and its group heading when it is the first
        // item, in view.
        let row = self.cursor;
        let first = if row > 0 && matches!(self.rows[row - 1], Row::Heading(_)) {
            row - 1
        } else {
            row
        };
        if first < self.scroll {
            self.scroll = first;
        } else if row >= self.scroll + visible {
            self.scroll = row + 1 - visible;
        }
        let height = 2 + prompt_lines + u16::from(prompt_lines > 0) + visible as u16;
        let modal = centered_rect_with_size(width, height, area);
        self.modal_area = Some(modal);
        let inner = render_modal_block(modal, buf, &self.title, theme);

        let dim = Style::default().fg(theme.disabled);
        for (i, line) in prompt.iter().enumerate() {
            buf.set_stringn(
                inner.x + 1,
                inner.y + i as u16,
                line,
                inner.width.saturating_sub(2) as usize,
                dim,
            );
        }
        let top = inner.y + prompt_lines + u16::from(prompt_lines > 0);
        let list = Rect {
            x: inner.x,
            y: top,
            width: inner.width,
            height: visible as u16,
        };
        self.list_area = Some(list);
        self.button_areas.clear();
        for (line, row) in self.rows.iter().skip(self.scroll).take(visible).enumerate() {
            let y = top + line as u16;
            match *row {
                Row::Heading(group) => {
                    let row = self.scroll + line;
                    let arrow = if self.groups[group].items.is_empty() {
                        " "
                    } else if self.groups[group].expanded {
                        ARROW_EXPANDED
                    } else {
                        ARROW_COLLAPSED
                    };
                    let text = format!(" {arrow} {}", self.heading_text(group));
                    let style = if row == self.cursor {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else if self.group_enabled(group) {
                        Style::default().fg(theme.accented_fg)
                    } else {
                        dim
                    }
                    .add_modifier(Modifier::BOLD);
                    let padded = format!("{text:<width$}", width = inner.width as usize);
                    buf.set_stringn(inner.x, y, padded, inner.width as usize, style);
                    // The buttons at the right end, one column from the edge,
                    // each a target of its own.
                    let buttons = &self.groups[group].buttons;
                    let total = UnicodeWidthStr::width(Self::buttons_text(buttons).as_str()) as u16;
                    let mut x = (inner.x + inner.width).saturating_sub(total + 1);
                    for button in buttons {
                        let label = format!("[{}]", button.icon);
                        let width = UnicodeWidthStr::width(label.as_str()) as u16;
                        buf.set_string(x, y, &label, style);
                        self.button_areas
                            .push((row, x..x + width, button.id.clone()));
                        x += width;
                    }
                }
                Row::Item(index) => {
                    let item = &self.items[index];
                    let mark = termide_ui::checkbox(item.checked);
                    // Under a heading, indented past its arrow.
                    let lead = if self.group_of(index).is_some() {
                        LEAD
                    } else {
                        1
                    };
                    let text = format!("{:lead$}{mark} {}", "", Self::item_text(item));
                    let style = if self.scroll + line == self.cursor {
                        Style::default().fg(theme.bg).bg(theme.fg)
                    } else if item.enabled {
                        Style::default().fg(theme.fg)
                    } else {
                        dim
                    };
                    let padded = format!("{text:<width$}", width = inner.width as usize);
                    buf.set_stringn(inner.x, y, padded, inner.width as usize, style);
                }
            }
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        match chord.canonical.code {
            KeyCode::Esc => return Ok(Some(ModalResult::Cancelled)),
            KeyCode::Enter => return Ok(Some(ModalResult::Confirmed(self.outcome(None)))),
            KeyCode::Char(' ') => self.toggle_row(self.cursor),
            KeyCode::Char(c) => {
                if let Some(Row::Heading(group)) = self.rows.get(self.cursor).copied() {
                    let pressed = self.groups[group]
                        .buttons
                        .iter()
                        .find(|button| button.key == c)
                        .map(|button| button.id.clone());
                    if pressed.is_some() {
                        return Ok(Some(ModalResult::Confirmed(self.outcome(pressed))));
                    }
                }
            }
            KeyCode::Right => self.expand(),
            KeyCode::Left => self.collapse(),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-(MAX_ROWS as isize)),
            KeyCode::PageDown => self.move_by(MAX_ROWS as isize),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.rows.len().saturating_sub(1),
            _ => {}
        }
        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        // A click beside the modal leaves everything as it was, as Esc does.
        if is_click_outside(&mouse, self.modal_area) {
            return Ok(Some(ModalResult::Cancelled));
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(list) = self.list_area else {
                    return Ok(None);
                };
                let inside = mouse.column >= list.x
                    && mouse.column < list.x + list.width
                    && mouse.row >= list.y
                    && mouse.row < list.y + list.height;
                if inside {
                    let row = self.scroll + (mouse.row - list.y) as usize;
                    let pressed = self
                        .button_areas
                        .iter()
                        .find(|(at, columns, _)| *at == row && columns.contains(&mouse.column));
                    if let Some((_, _, id)) = pressed {
                        let id = id.clone();
                        return Ok(Some(ModalResult::Confirmed(self.outcome(Some(id)))));
                    }
                    // A click on a heading's arrow opens or closes the group;
                    // anywhere else on a row it moves the cursor there and
                    // toggles the row.
                    let on_arrow = usize::from(mouse.column - list.x) < LEAD;
                    match self.rows.get(row).copied() {
                        Some(Row::Heading(group)) if on_arrow => {
                            self.cursor = row;
                            let expanded = self.groups[group].expanded;
                            self.set_expanded(group, !expanded);
                        }
                        Some(_) => {
                            self.cursor = row;
                            self.toggle_row(row);
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use termide_core::{ChecklistButton, ChecklistGroup};

    fn item(key: &str, group: &str, checked: bool, enabled: bool) -> ChecklistItem {
        ChecklistItem {
            key: key.into(),
            label: key.into(),
            group: group.into(),
            checked,
            enabled,
            note: if enabled {
                String::new()
            } else {
                "locked".into()
            },
        }
    }

    fn key(code: KeyCode) -> termide_core::KeyChord {
        termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn rows(modal: &mut ChecklistModal) -> Vec<String> {
        let area = Rect::new(0, 0, 50, 16);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &Theme::default());
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn arrow(open: bool) -> &'static str {
        if open {
            ARROW_EXPANDED
        } else {
            ARROW_COLLAPSED
        }
    }

    #[test]
    fn groups_start_collapsed_with_their_count_and_open_with_the_arrows() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "Off before the first request",
            vec![
                item("read", "Built-in", true, true),
                item("bash", "Built-in", true, true),
                item("review", "Skills", false, false),
            ],
            vec![],
        );
        let shown = rows(&mut modal);
        let closed = format!("{} [✓] Built-in  2/2", arrow(false));
        assert!(shown.iter().any(|r| r.contains(&closed)), "{shown:?}");
        assert!(!shown.iter().any(|r| r.contains("read")), "{shown:?}");
        // The headings follow one another: no blank line between groups.
        let built_in = shown.iter().position(|r| r.contains("Built-in")).unwrap();
        assert!(shown[built_in + 1].contains("Skills  0/1"), "{shown:?}");

        // → opens the group, a second → steps into it, ← goes back up to
        // the heading and a second ← closes it again.
        modal.handle_key(key(KeyCode::Right)).unwrap();
        let shown = rows(&mut modal);
        let open = format!("{} [✓] Built-in", arrow(true));
        assert!(shown.iter().any(|r| r.contains(&open)), "{shown:?}");
        assert!(shown.iter().any(|r| r.contains("   [✓] read")), "{shown:?}");
        modal.handle_key(key(KeyCode::Right)).unwrap();
        modal.handle_key(key(KeyCode::Down)).unwrap();
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert_eq!(modal.checked(), ["read"]);
        modal.handle_key(key(KeyCode::Left)).unwrap();
        assert_eq!(modal.cursor, 0);
        modal.handle_key(key(KeyCode::Left)).unwrap();
        let shown = rows(&mut modal);
        let closed = format!("{} [-] Built-in  1/2", arrow(false));
        assert!(shown.iter().any(|r| r.contains(&closed)), "{shown:?}");

        // Down to the skills, open: the locked skill stays off.
        modal.handle_key(key(KeyCode::Down)).unwrap();
        modal.handle_key(key(KeyCode::Right)).unwrap();
        modal.handle_key(key(KeyCode::Down)).unwrap();
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert!(rows(&mut modal)
            .iter()
            .any(|r| r.contains("[ ] review — locked")));
        let Some(ModalResult::Confirmed(outcome)) = modal.handle_key(key(KeyCode::Enter)).unwrap()
        else {
            panic!("Enter applies");
        };
        assert_eq!(outcome.checked, vec!["read".to_string()]);
        assert_eq!(outcome.pressed, None);
    }

    #[test]
    fn a_group_closed_under_the_cursor_leaves_it_on_the_heading() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "",
            vec![
                item("read", "Built-in", true, true),
                item("fetch", "Web", true, true),
            ],
            vec![],
        );
        modal.handle_key(key(KeyCode::Right)).unwrap();
        modal.handle_key(key(KeyCode::Down)).unwrap();
        assert_eq!(modal.rows[modal.cursor], Row::Item(0));
        // Closed by a click on its arrow while the cursor is inside it.
        let shown = rows(&mut modal);
        let y = shown.iter().position(|r| r.contains("Built-in")).unwrap();
        let x = shown[y][..shown[y].find(arrow(true)).unwrap()]
            .chars()
            .count() as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y as u16,
            modifiers: KeyModifiers::NONE,
        };
        modal.handle_mouse(click, Rect::default()).unwrap();
        assert_eq!(modal.rows, [Row::Heading(0), Row::Heading(1)]);
        assert_eq!(modal.cursor, 0);
        // The arrow opens and closes; nothing was toggled on the way.
        assert_eq!(modal.checked(), ["read", "fetch"]);
    }

    #[test]
    fn a_heading_toggles_the_unlocked_items_of_its_group() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "",
            vec![
                item("read", "Built-in", true, true),
                item("bash", "Built-in", false, true),
                item("grep", "Built-in", true, false),
                item("fetch", "Web", true, true),
            ],
            vec![],
        );
        let shown = rows(&mut modal);
        // Collapsed, the heading still toggles its whole group.
        assert!(
            shown.iter().any(|r| r.contains("[-] Built-in  2/3")),
            "{shown:?}"
        );
        assert!(shown.iter().any(|r| r.contains("[✓] Web")), "{shown:?}");
        // Partly on: the heading turns every unlocked item on.
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert_eq!(modal.checked(), vec!["read", "bash", "grep", "fetch"]);
        assert!(rows(&mut modal).iter().any(|r| r.contains("[✓] Built-in")));
        // All on: off again, except the locked one; the other group is untouched.
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert_eq!(modal.checked(), vec!["grep", "fetch"]);
    }

    fn heading(name: &str, note: &str, buttons: &[(&str, &str, char)]) -> ChecklistGroup {
        ChecklistGroup {
            name: name.into(),
            note: note.into(),
            buttons: buttons
                .iter()
                .map(|(id, icon, key)| ChecklistButton {
                    id: (*id).into(),
                    icon: (*icon).into(),
                    key: *key,
                })
                .collect(),
        }
    }

    #[test]
    fn heading_buttons_close_the_list_and_an_empty_group_is_a_heading_alone() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "",
            vec![item("db__q", "MCP db", true, true)],
            vec![
                heading("MCP db", "", &[("reload:db", "↻", 'r')]),
                heading(
                    "MCP plane",
                    "needs sign-in",
                    &[("reload:plane", "↻", 'r'), ("login:plane", "⇥", 'l')],
                ),
            ],
        );
        let shown = rows(&mut modal);
        let db = shown.iter().find(|r| r.contains("MCP db")).unwrap();
        assert!(
            db.contains("[✓] MCP db  1/1") && db.contains("[↻]"),
            "{shown:?}"
        );
        // No items: no arrow, no mark, no count; the remark and both buttons.
        let plane = shown.iter().find(|r| r.contains("MCP plane")).unwrap();
        assert!(plane.contains("   MCP plane — needs sign-in"), "{shown:?}");
        assert!(plane.contains("[↻][⇥]"), "{shown:?}");
        assert!(!plane.contains(ARROW_COLLAPSED) && !plane.contains("0/0"));

        // A key works on its own heading only; elsewhere it is nothing.
        assert!(modal.handle_key(key(KeyCode::Char('l'))).unwrap().is_none());
        modal.handle_key(key(KeyCode::Down)).unwrap();
        // `→` on a heading with nothing under it stays put.
        modal.handle_key(key(KeyCode::Right)).unwrap();
        assert_eq!(modal.rows.len(), 2);
        // Unchecked first, then the button: the list goes back with both.
        modal.handle_key(key(KeyCode::Up)).unwrap();
        modal.handle_key(key(KeyCode::Char(' '))).unwrap();
        modal.handle_key(key(KeyCode::Down)).unwrap();
        let Some(ModalResult::Confirmed(outcome)) =
            modal.handle_key(key(KeyCode::Char('l'))).unwrap()
        else {
            panic!("the button closes the list");
        };
        assert_eq!(outcome.pressed.as_deref(), Some("login:plane"));
        assert!(outcome.checked.is_empty());

        // A click on a button presses it.
        let shown = rows(&mut modal);
        let y = shown.iter().position(|r| r.contains("MCP db")).unwrap();
        let x = shown[y][..shown[y].find("[↻]").unwrap()].chars().count() as u16 + 1;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y as u16,
            modifiers: KeyModifiers::NONE,
        };
        let Some(ModalResult::Confirmed(outcome)) =
            modal.handle_mouse(click, Rect::default()).unwrap()
        else {
            panic!("a click on the button closes the list");
        };
        assert_eq!(outcome.pressed.as_deref(), Some("reload:db"));
    }

    #[test]
    fn a_long_prompt_wraps_to_the_rows_instead_of_widening_the_modal() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "Off before the first request: kept out of the context. Later: refused.",
            vec![item("read", "Built-in", true, true)],
            vec![],
        );
        let shown = rows(&mut modal);
        let top = shown.iter().find(|r| !r.trim().is_empty()).unwrap();
        let width = top.trim().chars().count() as u16;
        // As narrow as the rows allow: the minimum width, not the prompt's.
        assert_eq!(width, crate::fit_modal_width(0, 50), "{shown:?}");
        assert!(shown.iter().any(|r| r.contains("kept out")), "{shown:?}");
        assert!(shown.iter().any(|r| r.contains("refused.")), "{shown:?}");
    }

    #[test]
    fn a_click_toggles_the_item_under_it_and_esc_changes_nothing() {
        let mut modal = ChecklistModal::new(
            "Tools",
            "",
            vec![item("read", "Built-in", true, true)],
            vec![],
        );
        modal.handle_key(key(KeyCode::Right)).unwrap();
        let shown = rows(&mut modal);
        let y = shown.iter().position(|r| r.contains("[✓] read")).unwrap();
        let row = &shown[y];
        let x = row[..row.find("[✓]").unwrap()].chars().count() as u16;
        let y = y as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        modal.handle_mouse(click, Rect::default()).unwrap();
        assert!(modal.checked().is_empty());
        assert!(matches!(
            modal.handle_key(key(KeyCode::Esc)).unwrap(),
            Some(ModalResult::Cancelled)
        ));
    }
}
