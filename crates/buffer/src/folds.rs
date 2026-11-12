//! Folded line ranges: the lines a fold hides, kept on their text as it is
//! edited.

use crate::line_marks::LineEdit;

/// A folded range of lines: `start` stays on screen as the fold's header and
/// the lines after it, through `end`, are hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fold {
    /// The header line, 0-based; it stays visible.
    pub start: usize,
    /// The last hidden line, 0-based and inclusive; always after `start`.
    pub end: usize,
}

impl Fold {
    /// Lines the fold hides.
    pub fn hidden_lines(&self) -> usize {
        self.end - self.start
    }

    fn contains_hidden(&self, line: usize) -> bool {
        line > self.start && line <= self.end
    }
}

/// The folds of a buffer, nested or apart, and the line runs they hide.
#[derive(Debug, Clone, Default)]
pub(crate) struct Folds {
    /// Sorted by start; at most one fold per header line.
    folds: Vec<Fold>,
    /// The hidden lines as sorted, disjoint, inclusive runs: the union of
    /// every fold's hidden lines, for lookups by line.
    hidden: Vec<(usize, usize)>,
}

impl Folds {
    pub fn all(&self) -> &[Fold] {
        &self.folds
    }

    pub fn is_empty(&self) -> bool {
        self.folds.is_empty()
    }

    /// Add `fold`, replacing one with the same header. `false` when there was
    /// nothing to add: an empty range, or that very fold.
    pub fn add(&mut self, fold: Fold) -> bool {
        if fold.end <= fold.start {
            return false;
        }
        match self.folds.binary_search_by_key(&fold.start, |f| f.start) {
            Ok(i) if self.folds[i] == fold => return false,
            Ok(i) => self.folds[i] = fold,
            Err(i) => self.folds.insert(i, fold),
        }
        self.rebuild();
        true
    }

    /// Remove the fold headed by `start`; `false` when there is none.
    pub fn remove(&mut self, start: usize) -> bool {
        match self.folds.binary_search_by_key(&start, |f| f.start) {
            Ok(i) => {
                self.folds.remove(i);
                self.rebuild();
                true
            }
            Err(_) => false,
        }
    }

    /// Remove every fold that hides `line`, so it shows; `false` when it
    /// already did.
    pub fn reveal(&mut self, line: usize) -> bool {
        let before = self.folds.len();
        self.folds.retain(|f| !f.contains_hidden(line));
        if self.folds.len() == before {
            return false;
        }
        self.rebuild();
        true
    }

    pub fn clear(&mut self) -> bool {
        if self.folds.is_empty() {
            return false;
        }
        self.folds.clear();
        self.hidden.clear();
        true
    }

    /// The fold headed by `line`.
    pub fn at(&self, line: usize) -> Option<Fold> {
        self.folds
            .binary_search_by_key(&line, |f| f.start)
            .ok()
            .map(|i| self.folds[i])
    }

    /// The hidden run holding `line`, when a fold hides it.
    fn run_of(&self, line: usize) -> Option<(usize, usize)> {
        let i = self.hidden.partition_point(|&(_, end)| end < line);
        self.hidden.get(i).copied().filter(|&(s, _)| s <= line)
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        !self.hidden.is_empty() && self.run_of(line).is_some()
    }

    /// `line` when it shows, else the first line after the run hiding it.
    pub fn visible_at_or_after(&self, line: usize) -> usize {
        self.run_of(line).map_or(line, |(_, end)| end + 1)
    }

    /// `line` when it shows, else the header of the fold hiding it.
    pub fn visible_at_or_before(&self, line: usize) -> usize {
        self.run_of(line).map_or(line, |(start, _)| start - 1)
    }

    /// Hidden lines before `line`.
    pub fn hidden_before(&self, line: usize) -> usize {
        self.hidden
            .iter()
            .take_while(|&&(start, _)| start < line)
            .map(|&(start, end)| end.min(line - 1) - start + 1)
            .sum()
    }

    /// The `index`-th visible line (0-based), counting only lines that show.
    pub fn line_at_visible_index(&self, index: usize) -> usize {
        let mut line = index;
        for &(start, end) in &self.hidden {
            if start > line {
                break;
            }
            line += end - start + 1;
        }
        line
    }

    /// Move the folds so that each stays on its text after `edit`. A fold
    /// whose lines the edit touches is dropped — its text shows again — save
    /// for an edit inside its header line alone, which keeps it.
    pub fn shift(&mut self, edit: LineEdit) -> bool {
        if self.folds.is_empty() {
            return false;
        }
        let removed = edit.end_line - edit.start_line;
        let in_one_line = removed == 0 && edit.inserted_breaks == 0;
        // The first line whose text moves as a whole: the edit's own end line
        // when the edit ends at its start, the line after it otherwise.
        let first_moved = if edit.end_at_line_start && !in_one_line {
            edit.end_line
        } else {
            edit.end_line + 1
        };
        let before = self.folds.clone();
        self.folds.retain_mut(|fold| {
            if fold.end < edit.start_line {
                return true;
            }
            if fold.start >= first_moved {
                fold.start = fold.start - removed + edit.inserted_breaks;
                fold.end = fold.end - removed + edit.inserted_breaks;
                return true;
            }
            in_one_line && edit.start_line == fold.start
        });
        if self.folds == before {
            return false;
        }
        self.rebuild();
        true
    }

    fn rebuild(&mut self) {
        self.hidden.clear();
        for fold in &self.folds {
            let (start, end) = (fold.start + 1, fold.end);
            match self.hidden.last_mut() {
                Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
                _ => self.hidden.push((start, end)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds(ranges: &[(usize, usize)]) -> Folds {
        let mut folds = Folds::default();
        for &(start, end) in ranges {
            folds.add(Fold { start, end });
        }
        folds
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

    fn ranges(folds: &Folds) -> Vec<(usize, usize)> {
        folds.all().iter().map(|f| (f.start, f.end)).collect()
    }

    #[test]
    fn a_fold_hides_the_lines_after_its_header() {
        let f = folds(&[(2, 5)]);
        assert!(!f.is_hidden(2));
        assert!(f.is_hidden(3) && f.is_hidden(5));
        assert!(!f.is_hidden(6));
        assert_eq!(f.visible_at_or_after(4), 6);
        assert_eq!(f.visible_at_or_before(4), 2);
        assert_eq!(f.visible_at_or_after(1), 1);
    }

    #[test]
    fn nested_and_adjacent_folds_hide_one_run() {
        let f = folds(&[(0, 9), (2, 4), (10, 12)]);
        assert_eq!(f.hidden, vec![(1, 9), (11, 12)]);
        assert_eq!(f.visible_at_or_before(3), 0);
        assert_eq!(f.hidden_before(10), 9);
        assert_eq!(f.hidden_before(13), 11);
        assert_eq!(f.hidden_before(5), 4);
    }

    #[test]
    fn visible_indices_skip_the_hidden_lines() {
        let f = folds(&[(1, 3), (5, 6)]);
        // Visible lines: 0 1 4 5 7 8...
        let visible: Vec<usize> = (0..5).map(|i| f.line_at_visible_index(i)).collect();
        assert_eq!(visible, vec![0, 1, 4, 5, 7]);
        for (i, line) in visible.into_iter().enumerate() {
            assert_eq!(line - f.hidden_before(line), i);
        }
    }

    #[test]
    fn revealing_a_line_drops_every_fold_over_it() {
        let mut f = folds(&[(0, 9), (2, 4), (6, 8)]);
        assert!(f.reveal(3));
        assert_eq!(ranges(&f), vec![(6, 8)]);
        assert!(!f.reveal(0));
    }

    #[test]
    fn lines_added_above_a_fold_move_it_down() {
        let mut f = folds(&[(4, 6)]);
        assert!(f.shift(edit((1, false), (1, false), 2)));
        assert_eq!(ranges(&f), vec![(6, 8)]);
        // A break typed at the very start of the header pushes it down too.
        assert!(f.shift(edit((6, true), (6, true), 1)));
        assert_eq!(ranges(&f), vec![(7, 9)]);
    }

    #[test]
    fn typing_in_the_header_keeps_the_fold() {
        let mut f = folds(&[(4, 6)]);
        assert!(!f.shift(edit((4, false), (4, false), 0)));
        assert_eq!(ranges(&f), vec![(4, 6)]);
    }

    #[test]
    fn an_edit_touching_the_fold_unfolds_it() {
        let mut f = folds(&[(4, 6), (10, 12)]);
        // A break at the end of the header, lines deleted inside.
        assert!(f.shift(edit((4, false), (4, false), 1)));
        assert_eq!(ranges(&f), vec![(11, 13)]);
        assert!(f.shift(edit((12, true), (13, true), 0)));
        assert!(f.is_empty());
    }

    #[test]
    fn edits_below_a_fold_leave_it() {
        let mut f = folds(&[(4, 6)]);
        assert!(!f.shift(edit((7, true), (9, true), 0)));
        assert_eq!(ranges(&f), vec![(4, 6)]);
    }
}
