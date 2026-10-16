//! Marks anchored to lines (breakpoints) that follow edits of the text.

use std::collections::BTreeSet;

/// A replaced range, stated in the pre-edit document by line and by whether
/// each end sits at the very start of its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineEdit {
    pub start_line: usize,
    pub start_at_line_start: bool,
    pub end_line: usize,
    pub end_at_line_start: bool,
    /// Line breaks in the inserted text.
    pub inserted_breaks: usize,
}

/// Move `marks` so that each stays on the text it was set on after `edit`.
///
/// When both ends of the range sit at line starts, whole lines are replaced:
/// marks on them go, and the marks below move by the change in line count —
/// a line break typed at the start of a line pushes its mark down with it.
/// Otherwise the first line keeps its head, so its mark stays; marks on the
/// lines merged into it join it, and the marks below move.
pub(crate) fn shift_marks(marks: &mut BTreeSet<usize>, edit: LineEdit) -> bool {
    let removed = edit.end_line - edit.start_line;
    if removed == 0 && edit.inserted_breaks == 0 {
        return false;
    }
    let whole_lines = edit.start_at_line_start && edit.end_at_line_start;
    let first_moved = if whole_lines {
        edit.end_line
    } else {
        edit.end_line + 1
    };
    if marks.range(edit.start_line..).next().is_none() {
        return false;
    }

    let before = marks.clone();
    let mut shifted = BTreeSet::new();
    for &line in &before {
        if line < edit.start_line {
            shifted.insert(line);
        } else if line >= first_moved {
            shifted.insert(line - removed + edit.inserted_breaks);
        } else if !whole_lines {
            // The first line, or a line merged into it.
            shifted.insert(edit.start_line);
        }
        // Whole lines replaced: the mark goes with its line.
    }
    let changed = shifted != before;
    *marks = shifted;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(lines: &[usize]) -> BTreeSet<usize> {
        lines.iter().copied().collect()
    }

    fn edit(start: (usize, bool), end: (usize, bool), breaks: usize) -> LineEdit {
        LineEdit {
            start_line: start.0,
            start_at_line_start: start.1,
            end_line: end.0,
            end_at_line_start: end.1,
            inserted_breaks: breaks,
        }
    }

    #[test]
    fn a_break_at_a_line_start_pushes_its_mark_down() {
        let mut m = marks(&[2, 5]);
        assert!(shift_marks(&mut m, edit((2, true), (2, true), 1)));
        assert_eq!(m, marks(&[3, 6]));
    }

    #[test]
    fn a_break_at_a_line_end_keeps_its_mark() {
        let mut m = marks(&[2, 5]);
        assert!(shift_marks(&mut m, edit((2, false), (2, false), 1)));
        assert_eq!(m, marks(&[2, 6]));
    }

    #[test]
    fn deleting_a_whole_line_drops_its_mark_and_lifts_the_rest() {
        let mut m = marks(&[1, 2, 5]);
        assert!(shift_marks(&mut m, edit((2, true), (3, true), 0)));
        assert_eq!(m, marks(&[1, 4]));
    }

    #[test]
    fn joining_lines_moves_the_mark_onto_the_joined_line() {
        let mut m = marks(&[3, 7]);
        assert!(shift_marks(&mut m, edit((2, false), (3, true), 0)));
        assert_eq!(m, marks(&[2, 6]));
    }

    #[test]
    fn an_edit_inside_one_line_moves_nothing() {
        let mut m = marks(&[2]);
        assert!(!shift_marks(&mut m, edit((2, false), (2, false), 0)));
        assert_eq!(m, marks(&[2]));
    }

    #[test]
    fn marks_above_the_edit_stay() {
        let mut m = marks(&[0, 1]);
        assert!(!shift_marks(&mut m, edit((4, true), (4, true), 3)));
        assert_eq!(m, marks(&[0, 1]));
    }

    #[test]
    fn pasting_lines_above_a_mark_moves_it_by_their_count() {
        let mut m = marks(&[10]);
        assert!(shift_marks(&mut m, edit((4, false), (4, false), 3)));
        assert_eq!(m, marks(&[13]));
    }
}
