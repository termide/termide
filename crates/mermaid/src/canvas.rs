//! A growable character grid used to compose diagram pseudographics.

use unicode_width::UnicodeWidthChar;

/// Fills the cell a double-width character spills into; dropped when the
/// grid becomes text, so the character's two columns line up with the rest.
pub(crate) const WIDE_TAIL: char = '\0';

/// A growable grid of characters addressed by `(x, y)`.
pub(crate) struct Canvas {
    rows: Vec<Vec<char>>,
}

impl Canvas {
    pub(crate) fn new() -> Self {
        Self { rows: Vec::new() }
    }

    fn ensure(&mut self, x: usize, y: usize) {
        if y >= self.rows.len() {
            self.rows.resize(y + 1, Vec::new());
        }
        if x >= self.rows[y].len() {
            self.rows[y].resize(x + 1, ' ');
        }
    }

    pub(crate) fn put(&mut self, x: usize, y: usize, ch: char) {
        let width = cell_width(ch);
        self.ensure(x + width - 1, y);
        put_cells(&mut self.rows[y], x, ch);
    }

    /// Write text left-to-right starting at `(x, y)`, a double-width
    /// character taking two cells.
    pub(crate) fn text(&mut self, x: usize, y: usize, s: &str) {
        let mut x = x;
        for ch in s.chars() {
            self.put(x, y, ch);
            x += cell_width(ch);
        }
    }

    pub(crate) fn hline(&mut self, x0: usize, x1: usize, y: usize, ch: char) {
        for x in x0..=x1 {
            self.put(x, y, ch);
        }
    }

    pub(crate) fn vline(&mut self, x: usize, y0: usize, y1: usize, ch: char) {
        for y in y0..=y1 {
            self.put(x, y, ch);
        }
    }

    /// Draw a single-line box with top-left at `(x, y)` and the given inner
    /// width, with the label centered. `corners` selects the corner glyphs.
    pub(crate) fn draw_box(&mut self, x: usize, y: usize, inner_w: usize, label: &str) {
        self.draw_box_styled(x, y, inner_w, label, ['┌', '┐', '└', '┘']);
    }

    /// Like [`Self::draw_box`] but with custom corner glyphs (shape hint).
    pub(crate) fn draw_box_styled(
        &mut self,
        x: usize,
        y: usize,
        inner_w: usize,
        label: &str,
        corners: [char; 4],
    ) {
        let w = inner_w + 2;
        self.put(x, y, corners[0]);
        self.put(x + w - 1, y, corners[1]);
        self.hline(x + 1, x + w - 2, y, '─');
        self.put(x, y + 1, '│');
        self.put(x + w - 1, y + 1, '│');
        let pad = inner_w.saturating_sub(label_width(label)) / 2;
        self.text(x + 1 + pad, y + 1, label);
        self.put(x, y + 2, corners[2]);
        self.put(x + w - 1, y + 2, corners[3]);
        self.hline(x + 1, x + w - 2, y + 2, '─');
    }

    /// Draw a box `h` rows tall with the label centred both ways — a plain
    /// node grown to seat the edges meeting its sides.
    pub(crate) fn draw_tall_box(
        &mut self,
        x: usize,
        y: usize,
        inner_w: usize,
        h: usize,
        label: &str,
        corners: [char; 4],
    ) {
        let w = inner_w + 2;
        self.put(x, y, corners[0]);
        self.put(x + w - 1, y, corners[1]);
        self.hline(x + 1, x + w - 2, y, '─');
        for row in y + 1..y + h - 1 {
            self.put(x, row, '│');
            self.put(x + w - 1, row, '│');
        }
        let pad = inner_w.saturating_sub(label_width(label)) / 2;
        self.text(x + 1 + pad, y + h / 2, label);
        self.put(x, y + h - 1, corners[2]);
        self.put(x + w - 1, y + h - 1, corners[3]);
        self.hline(x + 1, x + w - 2, y + h - 1, '─');
    }

    /// Draw a box with a centered title and, if `body` is non-empty, a second
    /// compartment of left-aligned lines below a separator rule. Used for class
    /// members / ER attributes. `corners` selects the corner glyphs.
    pub(crate) fn draw_panel(
        &mut self,
        x: usize,
        y: usize,
        inner_w: usize,
        title: &str,
        body: &[String],
        corners: [char; 4],
    ) {
        let w = inner_w + 2;
        self.put(x, y, corners[0]);
        self.put(x + w - 1, y, corners[1]);
        self.hline(x + 1, x + w - 2, y, '─');
        // Title (centered).
        self.put(x, y + 1, '│');
        self.put(x + w - 1, y + 1, '│');
        let pad = inner_w.saturating_sub(label_width(title)) / 2;
        self.text(x + 1 + pad, y + 1, title);
        let mut row = y + 2;
        if !body.is_empty() {
            // Separator rule between compartments.
            self.put(x, row, '├');
            self.put(x + w - 1, row, '┤');
            self.hline(x + 1, x + w - 2, row, '─');
            row += 1;
            for line in body {
                self.put(x, row, '│');
                self.put(x + w - 1, row, '│');
                self.text(x + 1, row, line);
                row += 1;
            }
        }
        self.put(x, row, corners[2]);
        self.put(x + w - 1, row, corners[3]);
        self.hline(x + 1, x + w - 2, row, '─');
    }

    /// Fill `│` on empty cells of a column between rows (e.g. a lifeline behind
    /// other content).
    pub(crate) fn lifeline(&mut self, x: usize, y0: usize, y1: usize) {
        for y in y0..=y1 {
            self.ensure(x, y);
            if self.rows[y][x] == ' ' {
                self.rows[y][x] = '│';
            }
        }
    }

    pub(crate) fn into_lines(self) -> Vec<String> {
        self.rows
            .into_iter()
            .map(|r| {
                let mut s: String = r.into_iter().filter(|&ch| ch != WIDE_TAIL).collect();
                let trimmed = s.trim_end();
                s.truncate(trimmed.len());
                s
            })
            .collect()
    }
}

/// Cells a character takes on the canvas: two for a double-width one, one
/// for the rest (a zero-width mark still needs a cell of its own here).
pub(crate) fn cell_width(ch: char) -> usize {
    if ch.width().unwrap_or(1) >= 2 {
        2
    } else {
        1
    }
}

/// Display width of a label: the cells [`Canvas::text`] gives it.
pub(crate) fn label_width(s: &str) -> usize {
    s.chars().map(cell_width).sum()
}

/// Put `ch` into `row` at `x` (which must have room for its width), clearing
/// the other half of any double-width character it overwrites.
pub(crate) fn put_cells(row: &mut [char], x: usize, ch: char) {
    let width = cell_width(ch);
    if row[x] == WIDE_TAIL && x > 0 {
        row[x - 1] = ' ';
    }
    let last = x + width - 1;
    if row.get(last + 1) == Some(&WIDE_TAIL) {
        row[last + 1] = ' ';
    }
    row[x] = ch;
    if width == 2 {
        row[x + 1] = WIDE_TAIL;
    }
}

/// Write `s` into `row` from `x`, stopping at the row's end; a double-width
/// character that does not fit whole is left out.
pub(crate) fn write_cells(row: &mut [char], x: usize, s: &str) {
    let mut x = x;
    for ch in s.chars() {
        let width = cell_width(ch);
        if x + width > row.len() {
            break;
        }
        put_cells(row, x, ch);
        x += width;
    }
}

/// The longest prefix of `s` that fits in `max` cells.
pub(crate) fn clip_to_width(s: &str, max: usize) -> &str {
    let mut used = 0;
    for (i, ch) in s.char_indices() {
        used += cell_width(ch);
        if used > max {
            return &s[..i];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_label_takes_two_cells_per_character() {
        let mut c = Canvas::new();
        c.draw_box(0, 0, label_width("日本"), "日本");
        let lines = c.into_lines();
        assert_eq!(lines, vec!["┌────┐", "│日本│", "└────┘"]);
    }

    #[test]
    fn overwriting_half_of_a_wide_character_clears_the_other_half() {
        let mut c = Canvas::new();
        c.text(0, 0, "日本x");
        c.put(1, 0, '│');
        c.put(2, 0, '-');
        assert_eq!(c.into_lines(), vec![" │- x"]);
    }

    #[test]
    fn clipping_never_splits_a_wide_character() {
        assert_eq!(clip_to_width("ab日本", 3), "ab");
        assert_eq!(clip_to_width("ab日本", 4), "ab日");
    }
}
