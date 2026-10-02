//! A mouse selection over the transcript's rendered rows, as a terminal makes
//! one: from the cell the drag started on to the cell it is over, running on
//! through whole rows in between. Positions are in flattened-line
//! coordinates, so scrolling does not move the selection off its text.

use ratatui::text::Line;

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

    /// The selected text of `lines`: each row's selected cells, trimmed of the
    /// padding a row ends with, joined by newlines.
    #[must_use]
    pub fn text(&self, lines: &[Line<'_>], width: usize) -> String {
        let (first, last) = self.bounds();
        let mut rows = Vec::new();
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(last.line + 1)
            .skip(first.line)
        {
            let Some((start, end)) = self.columns_on(index, width) else {
                rows.push(String::new());
                continue;
            };
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
            rows.push(row.trim_end().to_string());
        }
        rows.join("\n")
    }
}

/// The `http(s)://` URL of `line` that covers display column `col`, if any:
/// it runs from its scheme to the first space or closing delimiter, as the
/// terminal panel reads one, less any sentence punctuation it ends with.
#[must_use]
pub fn url_at(line: &Line<'_>, col: usize) -> Option<String> {
    let mut chars = Vec::new();
    let mut at = 0;
    for ch in line.spans.iter().flat_map(|span| span.content.chars()) {
        chars.push((ch, at));
        at += termide_ui::str_display_width(ch.encode_utf8(&mut [0; 4]));
    }
    let text: String = chars.iter().map(|(ch, _)| ch).collect();
    let mut from = 0;
    while let Some(found) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| text[from..].find(scheme))
        .min()
    {
        let start = from + found;
        let end = text[start..]
            .find(|ch: char| ch.is_whitespace() || ")>]}\"'`<".contains(ch))
            .map_or(text.len(), |len| start + len);
        let url = text[start..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let first = text[..start].chars().count();
        let last = first + url.chars().count();
        let cols = chars[first].1..chars.get(last).map_or(at, |(_, c)| *c);
        if cols.contains(&col) {
            return Some(url.to_string());
        }
        from = end.max(start + 1);
    }
    None
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
        assert_eq!(selection.text(&lines, 20), "row\nsecond row\nthird");
    }

    #[test]
    fn wide_characters_count_by_their_cells() {
        let lines = vec![Line::from("🕒 3s done")];
        let selection = TextSelection {
            anchor: cell(0, 3),
            head: cell(0, 4),
        };
        assert_eq!(selection.text(&lines, 20), "3s");
        assert!(!selection.is_empty());
    }

    #[test]
    fn a_url_is_found_under_its_cells_only() {
        let line = Line::from(vec![
            Span::raw("↓ Fetching "),
            Span::raw("https://docs.rs/x, and (http://a.b)."),
        ]);
        // `↓` is one cell wide, so the URL starts at column 11.
        assert_eq!(url_at(&line, 10), None);
        assert_eq!(url_at(&line, 11).as_deref(), Some("https://docs.rs/x"));
        assert_eq!(url_at(&line, 27).as_deref(), Some("https://docs.rs/x"));
        // The comma after it is sentence punctuation, not the URL.
        assert_eq!(url_at(&line, 28), None);
        assert_eq!(url_at(&line, 35).as_deref(), Some("http://a.b"));
        assert_eq!(url_at(&line, 45), None);
    }
}
