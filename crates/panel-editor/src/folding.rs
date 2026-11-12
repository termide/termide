//! Where folds can go: the foldable line ranges of a text, from its language
//! server or, without one, from its indentation, and the state of a fold
//! waiting for the server's answer.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use termide_buffer::TextBuffer;

/// How long a fold waits for the language server before falling back to the
/// indentation.
pub(crate) const SERVER_WAIT: Duration = Duration::from_millis(500);

/// How long a late answer is still taken in, for the folds that follow.
pub(crate) const LATE_ANSWER_WAIT: Duration = Duration::from_secs(30);

/// A foldable range: `start` is the header line that stays on screen, `end`
/// the last line the fold hides. Both 0-based, `end > start`.
pub(crate) type FoldRange = (usize, usize);

/// A fold action that needs the foldable ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldAction {
    /// Fold the innermost unfolded block around the cursor.
    Fold,
    /// Fold every block.
    FoldAll,
}

/// The editor's folding state apart from the folds themselves, which the
/// buffer keeps.
#[derive(Default)]
pub(crate) struct FoldingState {
    /// The server's ranges and the `edit_version` they describe.
    pub server_ranges: Option<(u64, Vec<FoldRange>)>,
    /// An action waiting for the server's ranges.
    pub pending: Option<FoldAction>,
    /// The pending action wants the app layer to ask the server.
    pub request_wanted: bool,
    /// The server's answer on its way, the `edit_version` it was asked for
    /// and when.
    pub rx: Option<(ServerAnswer, u64, Instant)>,
}

/// Where the server's folding ranges arrive.
pub(crate) type ServerAnswer = mpsc::Receiver<Option<Vec<lsp_types::FoldingRange>>>;

/// The ranges a server reported, as whole lines, sorted, empty ones dropped.
pub(crate) fn server_ranges(ranges: &[lsp_types::FoldingRange]) -> Vec<FoldRange> {
    let mut out: Vec<FoldRange> = ranges
        .iter()
        .map(|r| (r.start_line as usize, r.end_line as usize))
        .filter(|&(start, end)| end > start)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The blocks the indentation shows: a line followed by lines indented
/// deeper heads a block, which runs to the last of those lines that is not
/// blank. Blank lines inside a block belong to it; blank lines after it do
/// not. Sorted by header.
pub(crate) fn indent_ranges(buffer: &TextBuffer, tab_size: usize) -> Vec<FoldRange> {
    let tab_size = tab_size.max(1);
    let mut ranges = Vec::new();
    // Headers still open: (line, indentation).
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_text_line = 0;
    for line in 0..buffer.line_count() {
        let Some(text) = buffer.line_cow(line) else {
            continue;
        };
        let Some(indent) = indentation(&text, tab_size) else {
            continue;
        };
        while let Some(&(start, header_indent)) = open.last() {
            if header_indent < indent {
                break;
            }
            open.pop();
            if last_text_line > start {
                ranges.push((start, last_text_line));
            }
        }
        open.push((line, indent));
        last_text_line = line;
    }
    for (start, _) in open.into_iter().rev() {
        if last_text_line > start {
            ranges.push((start, last_text_line));
        }
    }
    ranges.sort_unstable();
    ranges
}

/// Screen columns of `text`'s leading whitespace; `None` for a blank line.
fn indentation(text: &str, tab_size: usize) -> Option<usize> {
    let mut columns = 0;
    for ch in text.chars() {
        match ch {
            ' ' => columns += 1,
            '\t' => columns += tab_size - columns % tab_size,
            '\n' | '\r' => return None,
            _ => return Some(columns),
        }
    }
    None
}

/// The range to fold for a cursor on `line`: the innermost one holding it
/// that is not folded yet, a block headed by the line itself first. Folding
/// again from the same place so takes the next enclosing block.
pub(crate) fn range_to_fold(
    ranges: &[FoldRange],
    line: usize,
    is_folded: impl Fn(usize) -> bool,
) -> Option<FoldRange> {
    ranges
        .iter()
        .copied()
        .filter(|&(start, end)| start <= line && line <= end && !is_folded(start))
        .max_by_key(|&(start, end)| (start, std::cmp::Reverse(end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(text: &str) -> Vec<FoldRange> {
        indent_ranges(&TextBuffer::from_text(text), 4)
    }

    #[test]
    fn a_block_ends_before_the_line_that_closes_it() {
        let text = "fn a() {\n    one;\n    two;\n}\n";
        assert_eq!(ranges(text), vec![(0, 2)]);
    }

    #[test]
    fn nested_blocks_and_blank_lines() {
        let text = "a:\n  b:\n    c\n\n    d\n  e\n\n\nf\n";
        // Lines: 0 a, 1 b, 2 c, 3 blank, 4 d, 5 e, 6-7 blank, 8 f.
        assert_eq!(ranges(text), vec![(0, 5), (1, 4)]);
    }

    #[test]
    fn a_block_open_at_the_end_of_the_text_closes_there() {
        assert_eq!(ranges("a\n\tb\n\t\tc"), vec![(0, 2), (1, 2)]);
    }

    #[test]
    fn lines_without_deeper_lines_head_nothing() {
        assert!(ranges("a\nb\n  \nc\n").is_empty());
    }

    #[test]
    fn the_innermost_unfolded_block_is_folded_first() {
        let ranges = [(0, 9), (2, 5), (3, 4)];
        assert_eq!(range_to_fold(&ranges, 4, |_| false), Some((3, 4)));
        assert_eq!(range_to_fold(&ranges, 2, |_| false), Some((2, 5)));
        assert_eq!(range_to_fold(&ranges, 2, |s| s == 2), Some((0, 9)));
        assert_eq!(range_to_fold(&ranges, 12, |_| false), None);
    }

    #[test]
    fn server_ranges_drop_one_line_ranges() {
        let range = |start_line, end_line| lsp_types::FoldingRange {
            start_line,
            end_line,
            ..Default::default()
        };
        let got = server_ranges(&[range(4, 6), range(1, 1), range(0, 9)]);
        assert_eq!(got, vec![(0, 9), (4, 6)]);
    }
}
