//! Edits stated in byte/point terms for incremental re-parsing.
//!
//! Every mutation of a [`crate::TextBuffer`] is appended here, so a consumer
//! that keeps a syntax tree can bring it up to date with the edits since the
//! revision it last saw instead of re-parsing the whole text.

use ropey::Rope;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// Edits kept for consumers that fall behind. Past this, a consumer that has
/// not synced re-parses from scratch, which is cheaper than replaying more.
const MAX_LOGGED_EDITS: usize = 1024;

/// Source of document identities: every buffer (and every clone of one) is a
/// different document as far as edit consumers are concerned.
static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

/// A position as tree-sitter states it: a row and a byte column.
///
/// The row is the rope's line index. The column counts bytes since the last
/// `\n`, which is tree-sitter's own convention; the two only diverge from a
/// tree-sitter row count in text using rarer line breaks (lone CR, form
/// feed, U+2028), where rows are informational and bytes stay exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EditPoint {
    pub row: usize,
    pub column: usize,
}

/// One replaced range, in the coordinates of the text right before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_position: EditPoint,
    pub old_end_position: EditPoint,
    pub new_end_position: EditPoint,
}

/// Where a buffer's text stands: which document, and how many edits into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditRevision {
    pub document: u64,
    pub seq: u64,
}

/// Recent edits of one document.
#[derive(Debug)]
pub(crate) struct EditLog {
    document: u64,
    /// Number of edits ever recorded; the last entry of `edits` has this seq.
    seq: u64,
    edits: VecDeque<SyntaxEdit>,
}

impl EditLog {
    pub(crate) fn new() -> Self {
        Self {
            document: NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed),
            seq: 0,
            edits: VecDeque::new(),
        }
    }

    pub(crate) fn revision(&self) -> EditRevision {
        EditRevision {
            document: self.document,
            seq: self.seq,
        }
    }

    /// Edits that lead from `since` to the current revision, oldest first.
    /// `None` when they are not all known — another document, or a revision
    /// older than the log keeps.
    pub(crate) fn since(&self, since: EditRevision) -> Option<impl Iterator<Item = &SyntaxEdit>> {
        if since.document != self.document || since.seq > self.seq {
            return None;
        }
        let behind = usize::try_from(self.seq - since.seq).ok()?;
        if behind > self.edits.len() {
            return None;
        }
        Some(self.edits.range(self.edits.len() - behind..))
    }

    /// Record that `chars` of `rope` is about to be replaced with `text`.
    /// Must run before the rope is modified.
    pub(crate) fn record(&mut self, rope: &Rope, chars: std::ops::Range<usize>, text: &str) {
        let start_byte = rope.char_to_byte(chars.start);
        let old_end_byte = rope.char_to_byte(chars.end);
        let start_position = point_at(rope, start_byte);
        let old_end_position = point_at(rope, old_end_byte);
        let new_end_position = advance(start_position, text);
        self.push(SyntaxEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte + text.len(),
            start_position,
            old_end_position,
            new_end_position,
        });
    }

    fn push(&mut self, edit: SyntaxEdit) {
        if self.edits.len() >= MAX_LOGGED_EDITS {
            self.edits.pop_front();
        }
        self.edits.push_back(edit);
        self.seq += 1;
    }
}

impl Clone for EditLog {
    /// A clone diverges from the original as soon as either is edited, so it
    /// is a new document: consumers synced to the original re-parse it.
    fn clone(&self) -> Self {
        Self::new()
    }
}

/// The [`EditPoint`] of `byte` in `rope`.
fn point_at(rope: &Rope, byte: usize) -> EditPoint {
    let row = rope.byte_to_line(byte);
    let mut line = row;
    let mut column = byte - rope.line_to_byte(line);
    // The rope also breaks lines at CR, form feed and the Unicode separators;
    // tree-sitter's column only restarts after `\n`, so keep counting back
    // across those breaks.
    while line > 0 {
        let line_start = rope.line_to_byte(line);
        if rope.byte(line_start - 1) == b'\n' {
            break;
        }
        line -= 1;
        column += line_start - rope.line_to_byte(line);
    }
    EditPoint { row, column }
}

/// The point reached by writing `text` at `start`.
fn advance(start: EditPoint, text: &str) -> EditPoint {
    let breaks = ropey::str_utils::byte_to_line_idx(text, text.len());
    let column = match text.rfind('\n') {
        Some(nl) => text.len() - nl - 1,
        None => start.column + text.len(),
    };
    EditPoint {
        row: start.row + breaks,
        column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_byte_and_point_ranges() {
        let rope = Rope::from_str("ab\nцд\nx");
        let mut log = EditLog::new();
        let start = log.revision();
        // Replace "д\nx" (chars 4..7) with "й\n\nzz".
        log.record(&rope, 4..7, "й\n\nzz");
        let edits: Vec<_> = log.since(start).unwrap().copied().collect();
        assert_eq!(
            edits,
            [SyntaxEdit {
                start_byte: 5,
                old_end_byte: 9,
                new_end_byte: 5 + "й\n\nzz".len(),
                start_position: EditPoint { row: 1, column: 2 },
                old_end_position: EditPoint { row: 2, column: 1 },
                new_end_position: EditPoint { row: 3, column: 2 },
            }]
        );
    }

    #[test]
    fn since_reports_only_newer_edits_of_the_same_document() {
        let rope = Rope::from_str("abc");
        let mut log = EditLog::new();
        log.record(&rope, 0..0, "x");
        let mid = log.revision();
        log.record(&rope, 1..1, "y");
        assert_eq!(log.since(mid).unwrap().count(), 1);
        assert_eq!(log.since(log.revision()).unwrap().count(), 0);
        let other = EditLog::new();
        assert!(log.since(other.revision()).is_none());
        assert!(
            log.clone().since(log.revision()).is_none(),
            "a clone is a different document"
        );
    }

    #[test]
    fn a_consumer_too_far_behind_gets_none() {
        let rope = Rope::from_str("a");
        let mut log = EditLog::new();
        let start = log.revision();
        for _ in 0..=MAX_LOGGED_EDITS {
            log.record(&rope, 0..0, "x");
        }
        assert!(log.since(start).is_none());
    }

    #[test]
    fn column_counts_back_across_non_newline_breaks() {
        // "a\rb" is two rope lines but one tree-sitter row.
        let rope = Rope::from_str("a\rbc");
        assert_eq!(point_at(&rope, 3), EditPoint { row: 1, column: 3 });
    }
}
