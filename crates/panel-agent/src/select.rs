//! A mouse selection over the transcript's rendered rows, as a terminal makes
//! one: from the cell the drag started on to the cell it is over, running on
//! through whole rows in between. Positions are in flattened-line
//! coordinates, so scrolling does not move the selection off its text.

use std::path::Path;

use ratatui::text::Line;
use termide_core::LinkTarget;
use termide_richtext::{Join, RowCopy};

/// A cell of the rendered transcript: the flattened line and the display
/// column on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Cell {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextSelection {
    /// Where the drag started.
    pub anchor: Cell,
    /// Where the pointer is now.
    pub head: Cell,
}

impl TextSelection {
    /// The selection's first and last cell, in reading order.
    fn bounds(&self) -> (Cell, Cell) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether the drag has left the cell it started on.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected columns of flattened line `line`, `start..end`, when any.
    #[must_use]
    pub fn columns_on(&self, line: usize, width: usize) -> Option<(usize, usize)> {
        let (first, last) = self.bounds();
        if line < first.line || line > last.line {
            return None;
        }
        let start = if line == first.line { first.col } else { 0 };
        let end = if line == last.line {
            (last.col + 1).min(width)
        } else {
            width
        };
        (start < end).then_some((start, end))
    }

    /// The selected text of `lines`: each row's selected cells past its
    /// decoration, as `copy` describes the rows (a row it does not cover is
    /// a line of its own). A row ends in a line break, trimmed of the padding
    /// it ends with, unless the row after it carries it on — after a space
    /// when it wrapped at one, at once when it was cut between characters.
    #[must_use]
    pub fn text(&self, lines: &[Line<'_>], copy: &[RowCopy], width: usize) -> String {
        let (first, last) = self.bounds();
        let row_copy = |index: usize| copy.get(index).copied().unwrap_or_default();
        let mut text = String::new();
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(last.line + 1)
            .skip(first.line)
        {
            if index > first.line {
                match row_copy(index).join {
                    Join::Break => text.push('\n'),
                    Join::Space => text.push(' '),
                    Join::Glued => {}
                }
            }
            let Some((start, end)) = self.columns_on(index, width) else {
                continue;
            };
            let start = start.max(usize::from(row_copy(index).lead));
            let mut row = String::new();
            let mut col = 0;
            for ch in line.spans.iter().flat_map(|span| span.content.chars()) {
                let w = termide_ui::str_display_width(ch.encode_utf8(&mut [0; 4]));
                if col >= start && col < end {
                    row.push(ch);
                }
                col += w;
                if col >= end {
                    break;
                }
            }
            // A space the row was cut after belongs to the text.
            let glued = index < last.line && row_copy(index + 1).join == Join::Glued;
            text.push_str(if glued { &row } else { row.trim_end() });
        }
        text
    }
}

/// The link written out on `line` that covers display column `col` — a web
/// address, or a path that exists, a relative one taken from `cwd` — with the
/// columns it spans, by the detection the terminal panel shares.
#[must_use]
pub fn link_at(
    line: &Line<'_>,
    col: usize,
    cwd: &Path,
) -> Option<(std::ops::Range<usize>, LinkTarget)> {
    // The display column each char starts at, and the line's full width.
    let mut cols = Vec::new();
    let mut text = String::new();
    let mut at = 0;
    for ch in line.spans.iter().flat_map(|span| span.content.chars()) {
        cols.push(at);
        text.push(ch);
        at += termide_ui::str_display_width(ch.encode_utf8(&mut [0; 4]));
    }
    let char_at = cols.iter().rposition(|&c| c <= col)?;
    let (range, target) = termide_core::links::link_at(&text, char_at, cwd)?;
    let start = cols[range.start];
    let end = cols.get(range.end).copied().unwrap_or(at);
    (col < end).then_some((start..end, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    fn cell(line: usize, col: usize) -> Cell {
        Cell { line, col }
    }

    #[test]
    fn a_selection_runs_from_cell_to_cell_through_whole_rows() {
        let lines = vec![
            Line::from("first row   "),
            Line::from("second row"),
            Line::from("third row"),
        ];
        // Dragged backwards: the order of the ends does not matter.
        let selection = TextSelection {
            anchor: cell(2, 4),
            head: cell(0, 6),
        };
        assert_eq!(selection.columns_on(0, 20), Some((6, 20)));
        assert_eq!(selection.columns_on(1, 20), Some((0, 20)));
        assert_eq!(selection.columns_on(2, 20), Some((0, 5)));
        assert_eq!(selection.columns_on(3, 20), None);
        assert_eq!(selection.text(&lines, &[], 20), "row\nsecond row\nthird");
    }

    #[test]
    fn a_copy_skips_decoration_and_rejoins_wrapped_rows() {
        let lines = vec![
            Line::from("› the answer"),
            Line::from("  wraps here"),
            Line::from("  ┊ git push "),
            Line::from("  ┊ origin"),
        ];
        let row = |lead, join| RowCopy { lead, join };
        let copy = [
            row(2, Join::Break),
            row(2, Join::Space),
            row(4, Join::Break),
            row(4, Join::Glued),
        ];
        let selection = TextSelection {
            anchor: cell(0, 0),
            head: cell(3, 19),
        };
        assert_eq!(
            selection.text(&lines, &copy, 20),
            "the answer wraps here\ngit push origin"
        );
        // Started inside the text, a row keeps what lies after the start.
        let selection = TextSelection {
            anchor: cell(2, 8),
            head: cell(3, 19),
        };
        assert_eq!(selection.text(&lines, &copy, 20), "push origin");
    }

    #[test]
    fn wide_characters_count_by_their_cells() {
        let lines = vec![Line::from("🕒 3s done")];
        let selection = TextSelection {
            anchor: cell(0, 3),
            head: cell(0, 4),
        };
        assert_eq!(selection.text(&lines, &[], 20), "3s");
        assert!(!selection.is_empty());
    }

    #[test]
    fn a_link_is_found_under_its_cells_only() {
        let line = Line::from(vec![
            Span::raw("↓ Fetching "),
            Span::raw("https://docs.rs/x, and (http://a.b)."),
        ]);
        let cwd = Path::new("/");
        let url = |col| link_at(&line, col, cwd).map(|(cols, target)| (cols, target.text()));
        // `↓` is one cell wide, so the URL starts at column 11.
        assert_eq!(url(10), None);
        assert_eq!(url(11), Some((11..28, "https://docs.rs/x".to_string())));
        assert_eq!(
            url(27).map(|(_, url)| url).as_deref(),
            Some("https://docs.rs/x")
        );
        // The comma after it is sentence punctuation, not the URL.
        assert_eq!(url(28), None);
        assert_eq!(url(35).map(|(_, url)| url).as_deref(), Some("http://a.b"));
        assert_eq!(url(45), None);
    }

    #[test]
    fn a_path_that_exists_is_a_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        let line = Line::from("  error: ./a.rs:3:1 and ./b.rs");
        let (cols, target) = link_at(&line, 12, dir.path()).unwrap();
        assert_eq!(cols, 9..15);
        assert_eq!(target, LinkTarget::Path(dir.path().join("./a.rs")));
        assert_eq!(link_at(&line, 25, dir.path()), None);
    }
}
