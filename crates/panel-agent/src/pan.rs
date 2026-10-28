//! Panning a diagram wider than the transcript: its rows are laid out whole
//! and shifted only when drawn, so the cached lines, the copy marks and the
//! links on them stay those of the unpanned diagram.

use std::borrow::Cow;

use ratatui::text::{Line, Span};
use termide_core::PanelEvent;

use crate::AgentPanel;

/// Columns a diagram pans by per wheel notch, horizontal or Shift+wheel.
pub(crate) const WHEEL_STEP: usize = 3;
/// Columns a diagram pans by per Shift+Left / Shift+Right.
pub(crate) const KEY_STEP: usize = 4;

/// Which diagram of the transcript: its item and its ordinal in the item's
/// text, folded ones counted, so the key outlives a change of width.
pub(crate) type DiagramKey = (usize, usize);

/// A diagram wider than the room it has, on the flattened lines: `rows`
/// rows from `line`, its content from column `col`, `cols` wide, of which
/// `view` columns show; row `line + rows` is its scrollbar's track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiagramSpot {
    pub line: usize,
    pub rows: usize,
    pub col: u16,
    pub cols: u16,
    pub view: u16,
    pub item: usize,
    pub index: usize,
}

impl DiagramSpot {
    pub fn key(&self) -> DiagramKey {
        (self.item, self.index)
    }

    /// The farthest the diagram pans: its last column at the right edge.
    pub fn max_pan(&self) -> usize {
        usize::from(self.cols.saturating_sub(self.view))
    }

    /// The flattened line its scrollbar is drawn on.
    pub fn track(&self) -> usize {
        self.line + self.rows
    }

    /// Whether flattened line `line` is one of its rows or its track.
    pub fn covers(&self, line: usize) -> bool {
        (self.line..=self.track()).contains(&line)
    }

    /// Whether flattened line `line` is one of its rows.
    pub fn is_row(&self, line: usize) -> bool {
        (self.line..self.track()).contains(&line)
    }
}

/// `line` as shown panned by `skip` columns: its first `keep` columns (the
/// mark and the bar) stay, the `skip` columns after them go. A wide
/// character cut by the left edge leaves blanks for its visible half.
pub(crate) fn pan_line(line: &Line<'static>, keep: usize, skip: usize) -> Line<'static> {
    if skip == 0 {
        return line.clone();
    }
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut col = 0;
    for span in &line.spans {
        let mut text = String::new();
        for ch in span.content.chars() {
            let w = termide_ui::str_display_width(ch.encode_utf8(&mut [0; 4]));
            let (start, end) = (col, col + w);
            col = end;
            if start < keep || start >= keep + skip {
                text.push(ch);
            } else if end > keep + skip {
                text.push_str(&" ".repeat(end - keep - skip));
            }
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
    }
    Line::from(spans).style(line.style)
}

impl AgentPanel {
    /// The wide diagram whose rows or scrollbar are at screen cell
    /// `(column, row)` of the transcript.
    pub(crate) fn diagram_under(&self, column: u16, row: u16) -> Option<DiagramSpot> {
        let area = self.transcript_area;
        let inside = column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height;
        if !inside || self.banner_shown() {
            return None;
        }
        let line = self.top + usize::from(row - area.y);
        self.transcript
            .wide_diagrams()
            .iter()
            .find(|spot| spot.covers(line))
            .copied()
    }

    /// The wide diagram of the selected block the keys pan: the first one
    /// on screen, else its first.
    pub(crate) fn selected_wide_diagram(&self) -> Option<DiagramSpot> {
        let shown = self.top..self.top + self.viewport_height();
        let mut spots = self
            .transcript
            .wide_diagrams()
            .iter()
            .filter(|spot| spot.item == self.selected);
        let first = spots.clone().next().copied();
        spots
            .find(|spot| shown.contains(&spot.line) || shown.contains(&spot.track()))
            .copied()
            .or(first)
    }

    /// Pan `spot` by `delta` columns; a redraw only when it moved.
    pub(crate) fn pan_diagram(&mut self, spot: &DiagramSpot, delta: isize) -> Vec<PanelEvent> {
        if self.transcript.pan_by(spot, delta) {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        }
    }

    /// A press on a diagram's scrollbar: on the thumb it is grabbed where
    /// pressed; on the track the thumb jumps to center on the press and is
    /// grabbed there. `None` when the press is on no diagram's scrollbar.
    pub(crate) fn press_diagram_bar(&mut self, column: u16, row: u16) -> Option<Vec<PanelEvent>> {
        let (spot, bar) = *self
            .diagram_bars
            .iter()
            .find(|(_, bar)| bar.hits_bar(column, row))?;
        let grab = bar.grab_offset(column, row).unwrap_or(bar.thumb_len / 2);
        self.diagram_drag = Some((spot.key(), grab));
        let moved = self.drag_diagram_bar(column).unwrap_or_default();
        Some(moved)
    }

    /// A drag of a grabbed scrollbar thumb to screen column `column`.
    /// `None` when no thumb is held.
    pub(crate) fn drag_diagram_bar(&mut self, column: u16) -> Option<Vec<PanelEvent>> {
        let (key, grab) = self.diagram_drag?;
        let Some(&(spot, bar)) = self.diagram_bars.iter().find(|(spot, _)| spot.key() == key)
        else {
            // Scrolled out of sight: the grab holds, nothing moves.
            return Some(vec![]);
        };
        let start = column.saturating_sub(bar.x).saturating_sub(grab);
        let to = bar.offset_for_thumb_start(start);
        Some(if self.transcript.set_pan(&spot, to) {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        })
    }

    /// The transcript's rows as they are shown: a panned diagram's rows
    /// shifted, so a mouse selection copies what it covers on screen.
    pub(crate) fn shown_rows(&self) -> Cow<'_, [Line<'static>]> {
        let rows = self.transcript.rendered();
        let panned: Vec<(DiagramSpot, usize)> = self
            .transcript
            .wide_diagrams()
            .iter()
            .map(|spot| (*spot, self.transcript.pan_of(spot)))
            .filter(|(_, pan)| *pan > 0)
            .collect();
        if panned.is_empty() {
            return Cow::Borrowed(rows);
        }
        let mut rows = rows.to_vec();
        for (spot, pan) in panned {
            for line in spot.line..spot.track() {
                if let Some(row) = rows.get_mut(line) {
                    *row = pan_line(row, usize::from(spot.col), pan);
                }
            }
        }
        Cow::Owned(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn panning_shifts_the_content_but_not_the_bar() {
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled("┊ ", Style::default()),
            Span::raw("┌──┐ abcdef"),
        ]);
        assert_eq!(text(&pan_line(&line, 4, 0)), "  ┊ ┌──┐ abcdef");
        assert_eq!(text(&pan_line(&line, 4, 3)), "  ┊ ┐ abcdef");
        assert_eq!(text(&pan_line(&line, 4, 7)), "  ┊ cdef");
        assert_eq!(pan_line(&line, 4, 3).spans.len(), 3);
    }

    #[test]
    fn a_wide_character_cut_by_the_edge_leaves_a_blank() {
        let line = Line::from(vec![Span::raw("┊ "), Span::raw("a界b")]);
        assert_eq!(text(&pan_line(&line, 2, 2)), "┊  b");
        assert_eq!(text(&pan_line(&line, 2, 3)), "┊ b");
    }

    #[test]
    fn a_spot_clamps_to_its_width() {
        let spot = DiagramSpot {
            line: 5,
            rows: 3,
            col: 4,
            cols: 50,
            view: 30,
            item: 1,
            index: 0,
        };
        assert_eq!(spot.max_pan(), 20);
        assert_eq!(spot.track(), 8);
        assert!(spot.covers(8) && !spot.is_row(8) && spot.is_row(5));
        assert!(!spot.covers(4) && !spot.covers(9));
    }
}
