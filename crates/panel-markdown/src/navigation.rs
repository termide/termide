//! Cursor movement, scrolling, selection, and line metrics for the preview.

use crate::render::{self, LinkSpan};
use crate::text::{char_col_to_display, slice_chars};
use crate::{MarkdownPanel, Pos};

impl MarkdownPanel {
    pub(crate) fn line_count(&self) -> usize {
        self.doc.lines.len()
    }

    /// Plain text of a rendered line (concatenated span contents).
    pub(crate) fn line_text(&self, i: usize) -> String {
        self.doc
            .lines
            .get(i)
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default()
    }

    /// Character count of a rendered line.
    pub(crate) fn line_len(&self, i: usize) -> usize {
        self.line_text(i).chars().count()
    }

    pub(crate) fn viewport_height(&self) -> usize {
        self.last_area.height.max(1) as usize
    }

    pub(crate) fn max_top(&self) -> usize {
        self.line_count().saturating_sub(self.viewport_height())
    }

    pub(crate) fn relayout_if_needed(&mut self, width: u16) {
        if width == self.layout_width || width == 0 {
            return;
        }
        self.doc = render::render_markdown(&self.source, width, &self.colors, self.is_light);
        self.layout_width = width;
        self.clamp_cursor();
        self.run_search(); // re-locate matches against the new wrapping
        self.top = self.top.min(self.max_top());
        // Jump to a pending `#fragment` now that anchor line indices exist.
        if let Some(frag) = self.pending_anchor.take() {
            self.scroll_to_anchor(&frag);
        }
    }

    pub(crate) fn clamp_cursor(&mut self) {
        let lines = self.line_count();
        if lines == 0 {
            self.cursor = (0, 0);
            return;
        }
        self.cursor.0 = self.cursor.0.min(lines - 1);
        self.cursor.1 = self.cursor.1.min(self.line_len(self.cursor.0));
    }

    pub(crate) fn scroll_by(&mut self, delta: i32) {
        let max = self.max_top();
        self.top = (self.top as i64 + delta as i64).clamp(0, max as i64) as usize;
    }

    /// Keep the cursor line within the viewport.
    pub(crate) fn ensure_cursor_visible(&mut self) {
        let h = self.viewport_height();
        if self.cursor.0 < self.top {
            self.top = self.cursor.0;
        } else if self.cursor.0 >= self.top + h {
            self.top = self.cursor.0 + 1 - h;
        }
        self.top = self.top.min(self.max_top());
    }

    /// Move the cursor, optionally extending the selection.
    pub(crate) fn move_cursor(&mut self, to: Pos, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = to;
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    pub(crate) fn move_vertical(&mut self, delta: i32, extend: bool) {
        let lines = self.line_count();
        if lines == 0 {
            return;
        }
        let line = (self.cursor.0 as i64 + delta as i64).clamp(0, lines as i64 - 1) as usize;
        let col = self.cursor.1.min(self.line_len(line));
        self.move_cursor((line, col), extend);
    }

    pub(crate) fn move_horizontal(&mut self, forward: bool, extend: bool) {
        let (mut line, mut col) = self.cursor;
        if forward {
            if col < self.line_len(line) {
                col += 1;
            } else if line + 1 < self.line_count() {
                line += 1;
                col = 0;
            }
        } else if col > 0 {
            col -= 1;
        } else if line > 0 {
            line -= 1;
            col = self.line_len(line);
        }
        self.move_cursor((line, col), extend);
    }

    /// Normalized selection range `(start, end)` with `start <= end`.
    pub(crate) fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        let b = self.cursor;
        if a <= b {
            Some((a, b))
        } else {
            Some((b, a))
        }
    }

    /// Text of the current selection, or the cursor's line when none.
    pub(crate) fn selected_text(&self) -> String {
        let Some((start, end)) = self.selection() else {
            return self.line_text(self.cursor.0);
        };
        if start.0 == end.0 {
            return slice_chars(&self.line_text(start.0), start.1, end.1);
        }
        let mut out = String::new();
        for line in start.0..=end.0 {
            let text = self.line_text(line);
            let part = if line == start.0 {
                slice_chars(&text, start.1, text.chars().count())
            } else if line == end.0 {
                slice_chars(&text, 0, end.1)
            } else {
                text
            };
            out.push_str(&part);
            if line != end.0 {
                out.push('\n');
            }
        }
        out
    }

    /// The link under display column `col` on the given rendered line.
    pub(crate) fn link_at(&self, line: usize, col: u16) -> Option<&LinkSpan> {
        self.doc
            .links
            .iter()
            .find(|l| l.line == line && col >= l.start && col < l.end)
    }

    /// The link under the cursor (cursor column is a char index; compare in
    /// display columns, which match for the common ASCII case).
    pub(crate) fn link_under_cursor(&self) -> Option<&LinkSpan> {
        let (line, col) = self.cursor;
        let disp = char_col_to_display(&self.line_text(line), col);
        self.link_at(line, disp as u16)
    }
}
