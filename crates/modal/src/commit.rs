//! Git commit modal dialog with multi-line text area.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::base::{button_style, render_modal_block};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use termide_core::ThemeColors;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::{ScrollBar, TextArea};

use crate::{centered_rect_with_size, Modal, ModalResult};

/// Focus area in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Textarea,
    Buttons,
}

/// Git commit modal window.
#[derive(Debug)]
pub struct CommitModal {
    /// Number of staged files (for title).
    staged_count: usize,
    /// Repository name (for title).
    repo_name: String,
    /// Branch name (for title).
    branch_name: String,
    /// Multi-line text area for commit message.
    textarea: TextArea,
    /// Current focus area.
    focus: FocusArea,
    /// Selected button index (0 = Commit, 1 = Cancel).
    selected_button: usize,
    /// Last rendered textarea area (for mouse click positioning).
    last_textarea_area: Option<Rect>,
    /// Last rendered textarea block area with border (for scroll detection).
    last_textarea_block_area: Option<Rect>,
    /// Last rendered buttons area (for mouse).
    last_buttons_area: Option<Rect>,
    /// Visible textarea height (for scrolling).
    visible_height: usize,
    /// Whether scroll should follow cursor (false after mouse scroll).
    scroll_follows_cursor: bool,
    /// A press landed in the message and the button is still down.
    dragging: bool,
}

impl CommitModal {
    /// Create a new commit modal.
    pub fn new(staged_count: usize, repo_name: String, branch_name: String) -> Self {
        Self {
            staged_count,
            repo_name,
            branch_name,
            textarea: TextArea::new(),
            focus: FocusArea::Textarea,
            selected_button: 0,
            last_textarea_area: None,
            last_textarea_block_area: None,
            last_buttons_area: None,
            visible_height: 3,
            scroll_follows_cursor: true,
            dragging: false,
        }
    }

    /// The message position under screen cell `(column, row)` of the
    /// message drawn in `area`, `row` inside it: below the last line it is
    /// the end of the text, left of the text its line's start.
    fn message_position(&self, area: Rect, column: u16, row: u16) -> (usize, usize) {
        let drawn_row = usize::from(row.saturating_sub(area.y));
        let last = self.textarea.line_count().saturating_sub(1);
        if self.textarea.scroll_offset() + drawn_row > last {
            return (last, usize::MAX);
        }
        let pos = self
            .textarea
            .position_at_drawn_row(drawn_row, usize::from(column.saturating_sub(area.x)));
        (pos.row, pos.col)
    }

    /// Calculate modal size based on terminal size.
    fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        // Width: 60% of screen, min 40, max 80
        let width = ((screen_width as f32 * 0.6) as u16)
            .clamp(40, 80)
            .min(screen_width);

        // Height: adaptive based on screen
        // Layout: border(1) + textarea(adaptive) + border(1) + buttons(1) + border(1)
        let min_textarea_height = 3u16;
        let overhead = 4u16; // borders + buttons row
        let available = screen_height.saturating_sub(overhead);
        let textarea_height = available.max(min_textarea_height).min(15);
        let height = textarea_height + overhead;

        (width, height.min(screen_height))
    }

    /// Render the textarea content.
    fn render_textarea(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let cursor = self.textarea.cursor();
        let scroll_offset = self.textarea.scroll_offset();
        let is_focused = self.focus == FocusArea::Textarea;

        // Get selection range for highlighting
        let selection_range = self.textarea.selection_range();

        for (i, line_idx) in (scroll_offset..(scroll_offset + area.height as usize)).enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }

            let line = self
                .textarea
                .lines()
                .get(line_idx)
                .map(|s| s.as_str())
                .unwrap_or("");

            // Determine if this line has selection
            let (sel_start, sel_end) = if let Some((start, end)) = selection_range {
                if line_idx >= start.row && line_idx <= end.row {
                    let line_start = if line_idx == start.row { start.col } else { 0 };
                    let line_end = if line_idx == end.row {
                        end.col
                    } else {
                        line.chars().count()
                    };
                    (Some(line_start), Some(line_end))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

            // Render line with selection highlighting and cursor
            let mut x = area.x;
            let line_len = line.chars().count();
            let is_cursor_line = is_focused && line_idx == cursor.row;

            for (char_idx, ch) in line.chars().enumerate() {
                if x >= area.x + area.width {
                    break;
                }

                let is_cursor_here = is_cursor_line && char_idx == cursor.col;
                let is_selected = sel_start.is_some()
                    && sel_end.is_some()
                    && char_idx >= sel_start.unwrap()
                    && char_idx < sel_end.unwrap();

                let ch_width = ch.width().unwrap_or(1) as u16;

                if is_cursor_here {
                    // Inverted cursor - show character with swapped colors
                    buf.set_string(
                        x,
                        y,
                        ch.to_string(),
                        Style::default().fg(theme.bg).bg(theme.fg),
                    );
                } else if is_selected {
                    buf.set_string(
                        x,
                        y,
                        ch.to_string(),
                        Style::default().fg(theme.selected_fg).bg(theme.selected_bg),
                    );
                } else {
                    buf.set_string(x, y, ch.to_string(), Style::default().fg(theme.fg));
                }
                x += ch_width;
            }

            // Show cursor at end of line or on empty line (inverted space)
            if is_cursor_line && cursor.col >= line_len && x < area.x + area.width {
                buf.set_string(x, y, " ", Style::default().bg(theme.fg));
                x += 1;
            }

            // Fill rest of line with background
            while x < area.x + area.width {
                buf.set_string(x, y, " ", Style::default().bg(theme.bg));
                x += 1;
            }
        }
    }
}

impl Modal for CommitModal {
    type Result = String;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (modal_width, modal_height) = self.calculate_modal_size(area.width, area.height);
        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        // Title with file count, repo and branch
        let t = i18n::t();
        let title = t.git_commit_title(self.staged_count, &self.repo_name, &self.branch_name);

        let inner = render_modal_block(modal_area, buf, &title, theme);

        // Split inner area: textarea + buttons
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // Textarea (takes remaining space)
                Constraint::Length(1), // Buttons
            ])
            .split(inner);

        // Textarea area with border
        let textarea_block = Block::default().borders(Borders::ALL).border_style(
            if self.focus == FocusArea::Textarea {
                Style::default().fg(theme.accented_fg)
            } else {
                Style::default().fg(theme.disabled)
            },
        );
        let textarea_inner = textarea_block.inner(chunks[0]);
        textarea_block.render(chunks[0], buf);

        // Update visible height and ensure cursor is visible (unless user scrolled with mouse)
        self.visible_height = textarea_inner.height as usize;
        if self.scroll_follows_cursor {
            self.textarea.ensure_cursor_visible(self.visible_height);
        }

        // Render textarea content
        self.render_textarea(textarea_inner, buf, theme);
        self.last_textarea_area = Some(textarea_inner);
        self.last_textarea_block_area = Some(chunks[0]);

        // Render scrollbar for textarea
        let total_lines = self.textarea.line_count();
        let theme_colors = ThemeColors::from(theme);
        ScrollBar::render(
            buf,
            chunks[0].x + chunks[0].width.saturating_sub(1), // Right border position
            chunks[0].y + 1,                                 // Inside top border
            chunks[0].height.saturating_sub(2),              // Inside borders
            self.textarea.scroll_offset(),
            self.visible_height,
            total_lines,
            &theme_colors,
            self.focus == FocusArea::Textarea, // is_focused
        );

        // Render buttons
        let commit_label = format!("[ {} ]", t.git_action_commit());
        let cancel_label = format!("[ {} ]", t.ui_cancel());

        let commit_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 0,
            theme,
        );
        let cancel_style = button_style(
            self.focus == FocusArea::Buttons && self.selected_button == 1,
            theme,
        );

        let buttons = Line::from(vec![
            Span::styled(commit_label, commit_style),
            Span::raw("    "),
            Span::styled(cancel_label, cancel_style),
        ]);

        let buttons_paragraph = Paragraph::new(buttons).alignment(Alignment::Center);
        buttons_paragraph.render(chunks[1], buf);
        self.last_buttons_area = Some(chunks[1]);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        // Any keyboard input means user is interacting - follow cursor again
        self.scroll_follows_cursor = true;

        // Escape always cancels
        if key.code == KeyCode::Esc {
            return Ok(Some(ModalResult::Cancelled));
        }

        // Ctrl+Enter commits from anywhere
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            let text = self.textarea.text();
            if text.trim().is_empty() {
                return Ok(Some(ModalResult::Cancelled));
            }
            return Ok(Some(ModalResult::Confirmed(text)));
        }

        match self.focus {
            FocusArea::Textarea => {
                match key.code {
                    KeyCode::Tab => {
                        self.focus = FocusArea::Buttons;
                    }
                    KeyCode::BackTab => {
                        self.focus = FocusArea::Buttons;
                    }
                    KeyCode::Enter => {
                        self.textarea.insert_newline();
                    }
                    KeyCode::PageUp => {
                        let page_size = self.visible_height.max(1);
                        // Move cursor up by page_size lines
                        let cursor = self.textarea.cursor();
                        let new_row = cursor.row.saturating_sub(page_size);
                        self.textarea.set_cursor(new_row, cursor.col);
                    }
                    KeyCode::PageDown => {
                        let page_size = self.visible_height.max(1);
                        // Move cursor down by page_size lines
                        let cursor = self.textarea.cursor();
                        let max_row = self.textarea.line_count().saturating_sub(1);
                        let new_row = (cursor.row + page_size).min(max_row);
                        self.textarea.set_cursor(new_row, cursor.col);
                    }
                    // Everything else edits the message like any other field.
                    _ => {
                        termide_ui::edit_text_area(&mut self.textarea, key);
                    }
                }
            }
            FocusArea::Buttons => {
                match key.code {
                    KeyCode::Tab | KeyCode::Up => {
                        self.focus = FocusArea::Textarea;
                    }
                    KeyCode::BackTab => {
                        self.focus = FocusArea::Textarea;
                    }
                    KeyCode::Left => {
                        self.selected_button = if self.selected_button == 0 { 1 } else { 0 };
                    }
                    KeyCode::Right => {
                        self.selected_button = if self.selected_button == 1 { 0 } else { 1 };
                    }
                    KeyCode::Enter => {
                        if self.selected_button == 0 {
                            // Commit button
                            let text = self.textarea.text();
                            if text.trim().is_empty() {
                                return Ok(Some(ModalResult::Cancelled));
                            }
                            return Ok(Some(ModalResult::Confirmed(text)));
                        } else {
                            // Cancel button
                            return Ok(Some(ModalResult::Cancelled));
                        }
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Switch back to textarea and insert
                        self.focus = FocusArea::Textarea;
                        self.textarea.insert(c);
                    }
                    _ => {}
                }
            }
        }

        Ok(None)
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        // Handle scroll events for textarea (use full block area with border)
        if let Some(block_area) = self.last_textarea_block_area {
            if mouse.row >= block_area.y
                && mouse.row < block_area.y + block_area.height
                && mouse.column >= block_area.x
                && mouse.column < block_area.x + block_area.width
            {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.scroll_follows_cursor = false;
                        let new_offset = self.textarea.scroll_offset().saturating_sub(3);
                        self.textarea.set_scroll_offset(new_offset);
                        return Ok(None);
                    }
                    MouseEventKind::ScrollDown => {
                        self.scroll_follows_cursor = false;
                        let max_offset = self
                            .textarea
                            .line_count()
                            .saturating_sub(self.visible_height);
                        let new_offset = (self.textarea.scroll_offset() + 3).min(max_offset);
                        self.textarea.set_scroll_offset(new_offset);
                        return Ok(None);
                    }
                    _ => {}
                }
            }
        }

        match mouse.kind {
            // A drag from a press in the message extends its selection; past
            // the message's top or bottom it scrolls one row per move, as the
            // editor's drag does, and past its sides it stops at the nearest
            // column.
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (true, Some(area)) = (self.dragging, self.last_textarea_area) {
                    let scroll = self.textarea.scroll_offset();
                    if mouse.row < area.y && scroll > 0 {
                        self.textarea.set_scroll_offset(scroll - 1);
                    } else if mouse.row >= area.bottom()
                        && scroll + usize::from(area.height) < self.textarea.line_count()
                    {
                        self.textarea.set_scroll_offset(scroll + 1);
                    }
                    self.scroll_follows_cursor = true;
                    let row = mouse
                        .row
                        .clamp(area.y, area.y + area.height.saturating_sub(1));
                    let (row, col) = self.message_position(area, mouse.column, row);
                    self.textarea.extend_selection_to(row, col);
                }
                return Ok(None);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                return Ok(None);
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return Ok(None),
        }

        // Check textarea click
        if let Some(textarea_area) = self.last_textarea_area {
            if mouse.row >= textarea_area.y
                && mouse.row < textarea_area.y + textarea_area.height
                && mouse.column >= textarea_area.x
                && mouse.column < textarea_area.x + textarea_area.width
            {
                self.focus = FocusArea::Textarea;
                self.scroll_follows_cursor = true; // Click means follow cursor again
                let (row, col) = self.message_position(textarea_area, mouse.column, mouse.row);
                self.textarea.mouse_press(row, col);
                self.dragging = true;
                return Ok(None);
            }
        }

        // Check buttons click
        if let Some(buttons_area) = self.last_buttons_area {
            if mouse.row >= buttons_area.y
                && mouse.row < buttons_area.y + buttons_area.height
                && mouse.column >= buttons_area.x
                && mouse.column < buttons_area.x + buttons_area.width
            {
                let t = i18n::t();
                let commit_label = format!("[ {} ]", t.git_action_commit());
                let cancel_label = format!("[ {} ]", t.ui_cancel());
                let total_width = commit_label.width() + 4 + cancel_label.width();

                let start_col =
                    buttons_area.x + (buttons_area.width.saturating_sub(total_width as u16)) / 2;
                let commit_end = start_col + commit_label.width() as u16;
                let cancel_start = commit_end + 4;
                let cancel_end = cancel_start + cancel_label.width() as u16;

                if mouse.column >= start_col && mouse.column < commit_end {
                    // Commit clicked
                    let text = self.textarea.text();
                    if text.trim().is_empty() {
                        return Ok(Some(ModalResult::Cancelled));
                    }
                    return Ok(Some(ModalResult::Confirmed(text)));
                } else if mouse.column >= cancel_start && mouse.column < cancel_end {
                    // Cancel clicked
                    return Ok(Some(ModalResult::Cancelled));
                }
            }
        }

        Ok(None)
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        self.focus = FocusArea::Textarea;
        self.scroll_follows_cursor = true;
        self.textarea
            .insert_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn modal() -> CommitModal {
        CommitModal::new(1, "repo".into(), "main".into())
    }

    fn press(modal: &mut CommitModal, code: KeyCode, modifiers: KeyModifiers) {
        modal
            .handle_key(termide_core::KeyChord::identity(KeyEvent::new(
                code, modifiers,
            )))
            .unwrap();
    }

    fn type_text(modal: &mut CommitModal, text: &str) {
        for ch in text.chars() {
            press(modal, KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }

    #[test]
    fn the_message_edits_at_its_cursor_and_moves_by_words() {
        let mut modal = modal();
        type_text(&mut modal, "fix bug");
        press(&mut modal, KeyCode::Left, KeyModifiers::CONTROL);
        type_text(&mut modal, "the ");
        assert_eq!(modal.textarea.text(), "fix the bug");
    }

    #[test]
    fn a_selection_is_deleted_as_one() {
        let mut modal = modal();
        type_text(&mut modal, "fix bugs");
        press(&mut modal, KeyCode::Left, KeyModifiers::SHIFT);
        press(&mut modal, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(modal.textarea.text(), "fix bug");
    }

    #[test]
    fn ctrl_shift_z_redoes() {
        let mut modal = modal();
        type_text(&mut modal, "a");
        press(&mut modal, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(modal.textarea.text(), "");
        press(
            &mut modal,
            KeyCode::Char('z'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(modal.textarea.text(), "a");
    }

    #[test]
    fn ctrl_v_does_not_type_a_v_even_from_the_buttons() {
        let mut modal = modal();
        type_text(&mut modal, "x");
        // A stand-in clipboard, so the user's is never read or written.
        termide_ui::clipboard::use_fake_clipboard("PASTE");
        let clip = "PASTE";
        press(&mut modal, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(modal.textarea.text(), format!("x{clip}"));

        press(&mut modal, KeyCode::Tab, KeyModifiers::NONE);
        let before = modal.textarea.text();
        press(&mut modal, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(modal.textarea.text(), before);
    }

    #[test]
    fn enter_breaks_the_line_and_a_paste_keeps_its_lines() {
        let mut modal = modal();
        type_text(&mut modal, "title");
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        assert!(modal.handle_paste("body\r\nmore"));
        assert_eq!(modal.textarea.text(), "title\nbody\nmore");
    }

    #[test]
    fn a_click_places_the_cursor_and_a_drag_selects_in_the_message() {
        use crate::base::test_mouse::{drag, press as down, release};
        let mut modal = modal();
        assert!(modal.handle_paste("fix: the bug\nbody text"));
        let area = Rect::new(0, 0, 100, 40);
        modal.render(area, &mut Buffer::empty(area), &Theme::default());
        let text = modal.last_textarea_area.unwrap();
        modal.handle_mouse(down(text.x + 5, text.y), area).unwrap();
        let cursor = modal.textarea.cursor();
        assert_eq!((cursor.row, cursor.col), (0, 5));
        modal
            .handle_mouse(drag(text.x + 4, text.y + 1), area)
            .unwrap();
        assert_eq!(
            modal.textarea.selected_text().as_deref(),
            Some("the bug\nbody")
        );
        // Below the text, even below the field, the drag selects to its end.
        modal
            .handle_mouse(drag(text.x, text.bottom() + 5), area)
            .unwrap();
        assert_eq!(
            modal.textarea.selected_text().as_deref(),
            Some("the bug\nbody text")
        );
        modal.handle_mouse(release(0, 0), area).unwrap();
        modal.handle_mouse(drag(text.x, text.y), area).unwrap();
        assert_eq!(
            modal.textarea.selected_text().as_deref(),
            Some("the bug\nbody text"),
            "after the release a drag selects nothing"
        );
    }

    /// Dragged past the message's bottom or top, the selection keeps going:
    /// each move scrolls one row and extends the selection onto it.
    #[test]
    fn a_drag_past_the_message_edge_scrolls_it() {
        use crate::base::test_mouse::{drag, press as down, release};
        let mut modal = modal();
        let lines: Vec<String> = (0..30).map(|i| format!("line {i:02}")).collect();
        assert!(modal.handle_paste(&lines.join("\n")));
        let area = Rect::new(0, 0, 100, 14);
        let render = |modal: &mut CommitModal| {
            modal.render(area, &mut Buffer::empty(area), &Theme::default());
        };
        // Back to the top, then a press on the first row.
        modal.textarea.set_cursor(0, 0);
        render(&mut modal);
        let text = modal.last_textarea_area.unwrap();
        let height = usize::from(text.height);
        assert!(height < 30, "the message is taller than the field");
        assert_eq!(modal.textarea.scroll_offset(), 0);
        modal.handle_mouse(down(text.x + 5, text.y), area).unwrap();

        // Three moves below the field: three rows scrolled, the selection on
        // the last row now shown.
        for _ in 0..3 {
            modal
                .handle_mouse(drag(text.x + 7, text.bottom() + 2), area)
                .unwrap();
            render(&mut modal);
        }
        assert_eq!(modal.textarea.scroll_offset(), 3);
        let cursor = modal.textarea.cursor();
        assert_eq!((cursor.row, cursor.col), (height + 2, 7));
        let selected = modal.textarea.selected_text().unwrap();
        assert!(selected.starts_with("00\nline 01"));
        assert!(selected.ends_with(&format!("line {:02}", height + 2)));

        // Back above the field: it scrolls up again, one row per move.
        modal
            .handle_mouse(drag(text.x + 2, text.y - 1), area)
            .unwrap();
        render(&mut modal);
        assert_eq!(modal.textarea.scroll_offset(), 2);
        let cursor = modal.textarea.cursor();
        assert_eq!((cursor.row, cursor.col), (2, 2));
        assert_eq!(
            modal.textarea.selected_text().as_deref(),
            Some("00\nline 01\nli")
        );
        modal.handle_mouse(release(0, 0), area).unwrap();
    }
}
