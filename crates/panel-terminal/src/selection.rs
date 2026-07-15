//! Mouse double/triple-click selection geometry: mapping a clicked cell to a
//! word or line range on the terminal screen. Pure helpers over
//! [`TerminalScreen`], independent of panel state.

use super::terminal::TerminalScreen;

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Word around `col` on `abs_row` as an inclusive (start, end) selection range,
/// or `None` when the clicked cell is not part of a word.
pub(crate) fn word_selection(
    screen: &TerminalScreen,
    abs_row: usize,
    col: usize,
) -> Option<((usize, usize), (usize, usize))> {
    let row = screen.get_line_by_absolute(abs_row)?;
    if col >= row.len() || !is_word_char(row[col].ch) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word_char(row[start - 1].ch) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < row.len() && is_word_char(row[end + 1].ch) {
        end += 1;
    }
    Some(((abs_row, start), (abs_row, end)))
}

/// Whole line on `abs_row` as an inclusive (start, end) selection range.
/// Trailing whitespace is trimmed at copy time.
pub(crate) fn line_selection(
    screen: &TerminalScreen,
    abs_row: usize,
) -> Option<((usize, usize), (usize, usize))> {
    let row = screen.get_line_by_absolute(abs_row)?;
    let last = row.len().saturating_sub(1);
    Some(((abs_row, 0), (abs_row, last)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_selection_covers_word_and_skips_boundaries() {
        let mut screen = TerminalScreen::new(3, 20);
        for ch in "foo bar_baz".chars() {
            screen.put_char(ch);
        }
        // Click inside "bar_baz" (cols 4..=10) -> whole word selected.
        assert_eq!(word_selection(&screen, 0, 5), Some(((0, 4), (0, 10))));
        // Click on the space -> no word selection.
        assert_eq!(word_selection(&screen, 0, 3), None);
    }

    #[test]
    fn line_selection_starts_at_column_zero() {
        let mut screen = TerminalScreen::new(3, 20);
        for ch in "hi".chars() {
            screen.put_char(ch);
        }
        let sel = line_selection(&screen, 0).unwrap();
        assert_eq!(sel.0, (0, 0));
    }
}
