//! Projects selection modal dialog.
//!
//! Lists the projects as the Projects menu does: the open ones first, then
//! the others below a separator. Each takes one row — the mark (● current,
//! ○ open in the background), when it was last worked on, the path, and the
//! attention mark when a panel of it waits for the user — and the row under the cursor is
//! inverted, as in the agent's session list.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::base::render_modal_block;
use std::path::{Path, PathBuf};

use termide_theme::Theme;
use termide_ui::str_display_width;

use crate::{calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig};

/// Marks the current project.
pub const CURRENT_MARK: &str = "●";
/// Marks a project open in the background.
pub const OPEN_MARK: &str = "○";

/// Action returned by the projects modal
#[derive(Debug, Clone)]
pub enum ProjectAction {
    /// Switch to the selected project
    Switch(PathBuf),
    /// Close the selected open project, the current one included
    Close(PathBuf),
    /// Request deletion of the selected project's layout
    Delete(PathBuf),
}

/// Item representing a project in the list
#[derive(Debug, Clone)]
pub struct ProjectItem {
    /// Original project path
    pub project_path: PathBuf,
    /// Display path (potentially shortened)
    pub display_path: String,
    /// When the project was last worked on (`2026-10-03 14:22`), empty when
    /// unknown
    pub modified: String,
    /// Whether this is the current project
    pub is_current: bool,
    /// Whether the project is open in this instance (the current one is)
    pub is_open: bool,
    /// Whether a panel of this project, open in the background, waits for
    /// the user
    pub attention: bool,
}

impl ProjectItem {
    /// The row in its three parts: the mark, when the project was last
    /// worked on (drawn dimmed, empty when unknown) and the path with the
    /// bell. The path is cut from its start to keep the row within `width`
    /// columns, so the project's own name and the bell stay in view.
    fn segments(&self, width: usize) -> (String, String, String) {
        let mark = if self.is_current {
            format!("{CURRENT_MARK} ")
        } else if self.is_open {
            format!("{OPEN_MARK} ")
        } else {
            "  ".to_string()
        };
        let time = if self.modified.is_empty() {
            String::new()
        } else {
            format!("{} ", self.modified)
        };
        let tail = if self.attention {
            format!(" {}", termide_core::attention_mark())
        } else {
            String::new()
        };
        let room = width.saturating_sub(
            str_display_width(&mark) + str_display_width(&time) + str_display_width(&tail),
        );
        let path = termide_ui::path_utils::truncate_left(&self.display_path, room);
        (mark, time, format!("{path}{tail}"))
    }

    /// The row's text, without the padding around it.
    fn label(&self) -> String {
        let (mark, time, rest) = self.segments(usize::MAX);
        format!("{mark}{time}{rest}")
    }
}

/// A row of the list: a project, by its position among the filtered ones,
/// or the separator between the open projects and the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Item(usize),
    Separator,
}

/// Projects selection modal window
#[derive(Debug)]
pub struct ProjectsModal {
    title: String,
    items: Vec<ProjectItem>,
    /// Position of the selected project among `filtered_indices`.
    cursor: usize,
    /// First row shown.
    scroll_offset: usize,
    last_list_area: Option<Rect>,
    filter: String,
    filtered_indices: Vec<usize>,
}

/// Maximum number of rows visible at once
const MAX_VISIBLE_ROWS: usize = 15;

/// Height of the empty line + filter row + separator above the list
const FILTER_ROWS: u16 = 3;

impl ProjectsModal {
    /// Create a new projects modal
    pub fn new(title: impl Into<String>, items: Vec<ProjectItem>) -> Self {
        let filtered_indices = (0..items.len()).collect();
        Self {
            title: title.into(),
            items,
            cursor: 0,
            scroll_offset: 0,
            last_list_area: None,
            filter: String::new(),
            filtered_indices,
        }
    }

    /// Set initial cursor position (an index into the items)
    pub fn with_cursor(mut self, index: usize) -> Self {
        // filtered_indices starts as 0..items.len(), so cursor == item index
        self.cursor = index.min(self.filtered_indices.len().saturating_sub(1));
        self.adjust_scroll();
        self
    }

    /// The rows as shown: the filtered projects, with a separator between
    /// the open ones and the others while the list keeps its own order.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.filtered_indices.len() + 1);
        for (pos, &index) in self.filtered_indices.iter().enumerate() {
            if self.filter.is_empty()
                && pos > 0
                && self.items[self.filtered_indices[pos - 1]].is_open
                && !self.items[index].is_open
            {
                rows.push(Row::Separator);
            }
            rows.push(Row::Item(pos));
        }
        rows
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = str_display_width(&self.title) as u16 + 4;

        // Widest row across all items (not just filtered, to keep stable
        // width), with a column of padding on either side.
        let max_row_width = self
            .items
            .iter()
            .map(|item| str_display_width(&item.label()) as u16 + 2)
            .max()
            .unwrap_or(40);

        // Filter row needs space for "  Filter: " prefix + input text
        let filter_prefix_width = 12u16; // "  Filter: ".width()

        calculate_modal_width(
            [title_width, max_row_width, filter_prefix_width].into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }

    /// Recompute filtered_indices from current filter value
    fn apply_filter(&mut self) {
        // Fuzzy on the path, best match first; an empty filter keeps the
        // list's own order.
        let mut query = termide_ui::fuzzy::Query::fuzzy_path(&self.filter);
        self.filtered_indices = termide_ui::fuzzy::rank(
            self.items
                .iter()
                .map(|item| query.score(&item.display_path)),
        );
        self.cursor = self
            .cursor
            .min(self.filtered_indices.len().saturating_sub(1));
        self.scroll_offset = 0;
        self.adjust_scroll();
    }

    /// Move cursor up
    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    /// Move cursor down
    fn cursor_down(&mut self) {
        if self.cursor < self.filtered_indices.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    /// Go to first item
    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    /// Go to last item
    fn cursor_end(&mut self) {
        self.cursor = self.filtered_indices.len().saturating_sub(1);
        self.adjust_scroll();
    }

    /// Adjust scroll to keep the cursor's row visible
    fn adjust_scroll(&mut self) {
        let row = self
            .rows()
            .iter()
            .position(|row| *row == Row::Item(self.cursor))
            .unwrap_or(0);
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, row, MAX_VISIBLE_ROWS);
    }

    /// The open project under the cursor, while the list is not filtered:
    /// a filtered list is ranked by match, so a place in it means nothing.
    pub fn selected_open_project(&self) -> Option<&Path> {
        if !self.filter.is_empty() {
            return None;
        }
        self.get_selected()
            .filter(|item| item.is_open)
            .map(|item| item.project_path.as_path())
    }

    /// Move the open project under the cursor to place `to` among the open
    /// ones, which lead the list, the cursor with it. Nothing moves while
    /// the list is filtered.
    pub fn move_selected_open_project(&mut self, to: usize) {
        if self.selected_open_project().is_none() {
            return;
        }
        // Unfiltered, the list shows the items in their order.
        let from = self.cursor;
        let open = self.items.iter().take_while(|item| item.is_open).count();
        let to = to.min(open.saturating_sub(1));
        let item = self.items.remove(from);
        self.items.insert(to, item);
        self.cursor = to;
        self.adjust_scroll();
    }

    /// Get the selected project from filtered list
    fn get_selected(&self) -> Option<&ProjectItem> {
        self.filtered_indices
            .get(self.cursor)
            .and_then(|&i| self.items.get(i))
    }
}

impl Modal for ProjectsModal {
    type Result = ProjectAction;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.calculate_modal_width(area.width);
        let rows = self.rows();

        // The list is preceded by filter row + separator
        let list_height = rows.len().min(MAX_VISIBLE_ROWS) as u16;

        // Height: 1 (top border) + FILTER_ROWS + list_height + 1 (bottom border)
        let modal_height = 2 + FILTER_ROWS + list_height;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // --- Filter input row ---
        let filter_label = "  Filter: ";
        let filter_text = format!("{}{}", filter_label, self.filter);
        // Pad to full inner width, reserving 1 cell for the block cursor
        let padding_len =
            (inner.width as usize).saturating_sub(str_display_width(&filter_text) + 1);
        let padding = " ".repeat(padding_len);

        let filter_style = Style::default().fg(theme.fg);
        let cursor_style = Style::default()
            .fg(theme.bg)
            .bg(theme.fg)
            .add_modifier(Modifier::BOLD);

        let filter_line = Line::from(vec![
            Span::styled(filter_text, filter_style),
            Span::styled("█", cursor_style),
            Span::styled(padding, filter_style),
        ]);

        // Render filter row on inner.y + 1 (row 0 is blank padding)
        let filter_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        };
        Paragraph::new(filter_line).render(filter_area, buf);

        // --- Separator row ---
        let sep_y = inner.y + 2;
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep_y)]
                .set_symbol("─")
                .set_style(Style::default().fg(theme.accented_bg));
        }

        // --- Project list (below filter + separator) ---
        let list_area = Rect {
            x: inner.x,
            y: inner.y + FILTER_ROWS,
            width: inner.width,
            height: inner.height.saturating_sub(FILTER_ROWS),
        };

        let row_width = list_area.width as usize;
        let lines: Vec<Line> = rows
            .iter()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ROWS)
            .map(|row| match *row {
                Row::Separator => Line::from(Span::styled(
                    "─".repeat(row_width),
                    Style::default().fg(theme.accented_bg),
                )),
                Row::Item(pos) => {
                    let item = &self.items[self.filtered_indices[pos]];
                    // One column of padding on either side.
                    let (mark, time, rest) = item.segments(row_width.saturating_sub(2));
                    let pad = row_width.saturating_sub(
                        1 + str_display_width(&mark)
                            + str_display_width(&time)
                            + str_display_width(&rest),
                    );
                    let selected = pos == self.cursor;
                    let style = if selected {
                        Style::default()
                            .fg(theme.bg)
                            .bg(theme.fg)
                            .add_modifier(Modifier::BOLD)
                    } else if item.is_current {
                        Style::default().fg(theme.accented_fg)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    // The time is a reminder, not a thing to read first; on
                    // the inverted row it keeps the row's colours.
                    let time_style = if selected {
                        style
                    } else {
                        Style::default().fg(theme.disabled)
                    };
                    Line::from(vec![
                        Span::styled(format!(" {mark}"), style),
                        Span::styled(time, time_style),
                        Span::styled(rest, style),
                        Span::styled(" ".repeat(pad), style),
                    ])
                }
            })
            .collect();
        Paragraph::new(lines)
            .style(Style::default().bg(theme.bg))
            .render(list_area, buf);

        self.last_list_area = Some(list_area);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),

            // Navigation
            KeyCode::Up => {
                self.cursor_up();
                Ok(None)
            }
            KeyCode::Down => {
                self.cursor_down();
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor_home();
                Ok(None)
            }
            KeyCode::End => {
                self.cursor_end();
                Ok(None)
            }

            // Confirm selection
            KeyCode::Enter => {
                if let Some(item) = self.get_selected() {
                    if item.is_current {
                        Ok(Some(ModalResult::Cancelled))
                    } else {
                        Ok(Some(ModalResult::Confirmed(ProjectAction::Switch(
                            item.project_path.clone(),
                        ))))
                    }
                } else {
                    Ok(None)
                }
            }

            // Delete or F8: close an open project (the current one too: the
            // app switches to another first), delete the saved layout of one
            // that is not open.
            KeyCode::Delete | KeyCode::F(8) => Ok(self.get_selected().map(|item| {
                let path = item.project_path.clone();
                if item.is_current || item.is_open {
                    ModalResult::Confirmed(ProjectAction::Close(path))
                } else {
                    ModalResult::Confirmed(ProjectAction::Delete(path))
                }
            })),

            // Filter text input
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter();
                Ok(None)
            }
            KeyCode::Char(ch) => {
                self.filter.push(ch);
                self.apply_filter();
                Ok(None)
            }

            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                for _ in 0..3 {
                    self.cursor_up();
                }
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.cursor_down();
                }
                return Ok(None);
            }
            _ => {}
        }

        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }
        let Some(list_area) = self.last_list_area else {
            return Ok(None);
        };
        if mouse.row < list_area.y
            || mouse.row >= list_area.y + list_area.height
            || mouse.column < list_area.x
            || mouse.column >= list_area.x + list_area.width
        {
            return Ok(None);
        }
        let clicked = self.scroll_offset + (mouse.row - list_area.y) as usize;
        let Some(Row::Item(pos)) = self.rows().get(clicked).copied() else {
            return Ok(None);
        };
        self.cursor = pos;
        let item = &self.items[self.filtered_indices[pos]];
        if item.is_current {
            Ok(Some(ModalResult::Cancelled))
        } else {
            Ok(Some(ModalResult::Confirmed(ProjectAction::Switch(
                item.project_path.clone(),
            ))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use std::path::Path;
    use termide_core::KeyChord;

    fn item(path: &str, is_current: bool, is_open: bool) -> ProjectItem {
        ProjectItem {
            project_path: PathBuf::from(path),
            display_path: path.to_string(),
            modified: String::new(),
            is_current,
            is_open,
            attention: false,
        }
    }

    fn press(modal: &mut ProjectsModal, code: KeyCode) -> Option<ModalResult<ProjectAction>> {
        modal
            .handle_key(KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE)))
            .unwrap()
    }

    #[test]
    fn delete_closes_an_open_project_and_deletes_the_layout_of_another() {
        let items = vec![
            item("/current", true, true),
            item("/background", false, true),
            item("/closed", false, false),
        ];
        let mut modal = ProjectsModal::new("Projects", items).with_cursor(0);
        assert!(matches!(
            press(&mut modal, KeyCode::Delete),
            Some(ModalResult::Confirmed(ProjectAction::Close(path))) if path == Path::new("/current")
        ));
        press(&mut modal, KeyCode::Down);
        assert!(matches!(
            press(&mut modal, KeyCode::Delete),
            Some(ModalResult::Confirmed(ProjectAction::Close(path))) if path == Path::new("/background")
        ));
        press(&mut modal, KeyCode::Down);
        assert!(matches!(
            press(&mut modal, KeyCode::F(8)),
            Some(ModalResult::Confirmed(ProjectAction::Delete(path))) if path == Path::new("/closed")
        ));
    }

    #[test]
    fn an_open_project_moves_among_the_open_ones_until_filtered() {
        let items = vec![
            item("/a", true, true),
            item("/b", false, true),
            item("/c", false, true),
            item("/d", false, false),
        ];
        let mut modal = ProjectsModal::new("Projects", items).with_cursor(2);
        assert_eq!(modal.selected_open_project(), Some(Path::new("/c")));
        modal.move_selected_open_project(0);
        let paths: Vec<_> = modal
            .items
            .iter()
            .map(|i| i.display_path.as_str())
            .collect();
        assert_eq!(paths, ["/c", "/a", "/b", "/d"]);
        assert_eq!(modal.selected_open_project(), Some(Path::new("/c")));

        modal.move_selected_open_project(99);
        let paths: Vec<_> = modal
            .items
            .iter()
            .map(|i| i.display_path.as_str())
            .collect();
        assert_eq!(paths, ["/a", "/b", "/c", "/d"], "never past the open ones");

        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.selected_open_project(), None, "not open");
        press(&mut modal, KeyCode::Up);
        press(&mut modal, KeyCode::Char('c'));
        assert_eq!(modal.selected_open_project(), None, "filtered");
    }

    #[test]
    fn a_row_carries_the_mark_the_time_the_path_and_the_bell() {
        let mut background = item("~/api", false, true);
        background.modified = "2026-10-03 14:22".into();
        background.attention = true;
        assert_eq!(
            background.label(),
            format!(
                "○ 2026-10-03 14:22 ~/api {}",
                termide_core::attention_mark()
            )
        );
        assert_eq!(item("~/x", true, true).label(), "● ~/x");
        assert_eq!(item("~/y", false, false).label(), "  ~/y");
    }

    #[test]
    fn a_long_path_loses_its_start_not_the_project_or_the_bell() {
        let mut long = item("~/very/long/path/to/the/project", false, true);
        long.attention = true;
        let (mark, time, rest) = long.segments(20);
        let fitted = format!("{mark}{time}{rest}");
        let tail = format!("project {}", termide_core::attention_mark());
        assert!(fitted.ends_with(&tail), "{fitted}");
        assert!(fitted.starts_with("○ …"), "{fitted}");
        assert_eq!(str_display_width(&fitted), 20);
    }

    #[test]
    fn a_separator_parts_open_projects_from_the_rest_until_filtered() {
        let items = vec![
            item("/a", true, true),
            item("/b", false, true),
            item("/c", false, false),
        ];
        let mut modal = ProjectsModal::new("Projects", items);
        assert_eq!(
            modal.rows(),
            vec![Row::Item(0), Row::Item(1), Row::Separator, Row::Item(2)]
        );
        press(&mut modal, KeyCode::Char('c'));
        assert!(!modal.rows().contains(&Row::Separator));
    }
}
