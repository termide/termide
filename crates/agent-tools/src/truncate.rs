//! Output limits shared by the tools.

/// Default number of lines `read` returns per call.
pub const READ_MAX_LINES: usize = 2000;
/// Byte budget of one `read` result, whichever limit is hit first.
pub const READ_MAX_BYTES: usize = 64 * 1024;
/// Byte budget of shell output kept inline; the rest goes to a file.
pub const SHELL_MAX_BYTES: usize = 48 * 1024;

/// Split `text` into a head and a tail that together fit `max_bytes`, cut on
/// line boundaries with the head taking about 60% of the budget. Returns
/// `None` when the text already fits. The middle part is what the caller
/// replaces with an omission marker. A line too long for a line cut to keep
/// half of a part's budget is cut inside, at a character boundary.
#[must_use]
pub fn head_tail(text: &str, max_bytes: usize) -> Option<HeadTail<'_>> {
    if text.len() <= max_bytes {
        return None;
    }
    let head_budget = max_bytes * 6 / 10;
    let head_end = head_end(text, head_budget);
    let tail_start = tail_start(text, max_bytes - head_budget);
    if tail_start <= head_end {
        return None;
    }
    let middle = &text[head_end..tail_start];
    Some(HeadTail {
        head: &text[..head_end],
        tail: &text[tail_start..],
        omitted_lines: middle.matches('\n').count(),
        omitted_bytes: middle.len(),
        whole_lines: at_line_start(text, head_end) && at_line_start(text, tail_start),
    })
}

/// [`head_tail`] for output whose middle is already gone: the head comes
/// from `head`, the tail from `tail`, with the same budgets and cuts.
/// `omitted_lines` and `omitted_bytes` count only what is cut from these two
/// texts.
#[must_use]
pub fn head_tail_split<'a>(head: &'a str, tail: &'a str, max_bytes: usize) -> HeadTail<'a> {
    let head_budget = max_bytes * 6 / 10;
    let head_end = if head.len() <= head_budget {
        head.len()
    } else {
        head_end(head, head_budget)
    };
    let tail_start = if tail.len() <= max_bytes - head_budget {
        0
    } else {
        tail_start(tail, max_bytes - head_budget)
    };
    let cut_head = &head[head_end..];
    let cut_tail = &tail[..tail_start];
    HeadTail {
        head: &head[..head_end],
        tail: &tail[tail_start..],
        omitted_lines: cut_head.matches('\n').count() + cut_tail.matches('\n').count(),
        omitted_bytes: cut_head.len() + cut_tail.len(),
        whole_lines: (head_end == head.len() || at_line_start(head, head_end))
            && at_line_start(tail, tail_start),
    }
}

/// End of the longest run of whole lines at the start of `text` within
/// `budget`; when that keeps less than half the budget, the last character
/// boundary within it.
fn head_end(text: &str, budget: usize) -> usize {
    let mut end = 0;
    for (index, _) in text.match_indices('\n') {
        if index + 1 > budget {
            break;
        }
        end = index + 1;
    }
    if end < budget / 2 {
        end = text.floor_char_boundary(budget);
    }
    end
}

/// Start of the longest run of whole lines at the end of `text` within
/// `budget`; when that keeps less than half the budget, the first character
/// boundary within it.
fn tail_start(text: &str, budget: usize) -> usize {
    let mut start = text.len();
    for (index, _) in text.rmatch_indices('\n') {
        if text.len() - (index + 1) > budget {
            break;
        }
        start = index + 1;
    }
    if text.len() - start < budget / 2 {
        start = text.ceil_char_boundary(text.len().saturating_sub(budget));
    }
    start
}

/// Whether `index` starts a line of `text`: its start, its end, or just
/// after a line break.
fn at_line_start(text: &str, index: usize) -> bool {
    index == 0 || index == text.len() || text.as_bytes()[index - 1] == b'\n'
}

#[derive(Debug, PartialEq, Eq)]
pub struct HeadTail<'a> {
    pub head: &'a str,
    pub tail: &'a str,
    /// Line breaks in what was cut out.
    pub omitted_lines: usize,
    pub omitted_bytes: usize,
    /// Whether the cuts fell on line breaks, so what was cut out is whole
    /// lines and `omitted_lines` counts them.
    pub whole_lines: bool,
}

/// What an omission marker says was cut: whole lines when the cuts fell on
/// line breaks, bytes when a long line was cut inside. `extra_lines` and
/// `extra_bytes` are dropped before the cut, counted along.
#[must_use]
pub fn omitted_text(cut: &HeadTail<'_>, extra_lines: usize, extra_bytes: usize) -> String {
    if cut.whole_lines {
        format!("{} lines", cut.omitted_lines + extra_lines)
    } else {
        format!("{} bytes", cut.omitted_bytes + extra_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_not_truncated() {
        assert!(head_tail("a\nb\n", 100).is_none());
    }

    #[test]
    fn head_and_tail_cut_on_line_boundaries() {
        let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let cut = head_tail(&text, 200).expect("truncated");
        assert!(cut.head.starts_with("line 1\n"));
        assert!(cut.head.ends_with('\n'));
        assert!(cut.tail.ends_with("line 100\n"));
        assert!(cut.tail.starts_with("line "));
        assert!(cut.head.len() + cut.tail.len() <= 200);
        assert!(cut.omitted_lines > 50);
    }

    #[test]
    fn split_head_and_tail_are_cut_separately() {
        let head: String = (1..=50).map(|n| format!("h{n}\n")).collect();
        let tail: String = (1..=50).map(|n| format!("t{n}\n")).collect();
        let cut = head_tail_split(&head, &tail, 100);
        assert!(cut.head.starts_with("h1\n") && cut.head.len() <= 60);
        assert!(cut.tail.ends_with("t50\n") && cut.tail.len() <= 40);
        assert_eq!(
            cut.omitted_lines,
            100 - cut.head.lines().count() - cut.tail.lines().count()
        );
        // Short parts are kept whole.
        let cut = head_tail_split("a\n", "b\n", 100);
        assert_eq!((cut.head, cut.tail, cut.omitted_lines), ("a\n", "b\n", 0));
        assert_eq!(omitted_text(&cut, 3, 0), "3 lines");
    }

    #[test]
    fn a_line_with_no_break_in_reach_is_cut_inside_and_counted_in_bytes() {
        // One long line of multibyte characters: nothing to cut on.
        let text = "é".repeat(500);
        let cut = head_tail(&text, 200).expect("truncated");
        assert!(!cut.head.is_empty() && !cut.tail.is_empty());
        assert!(cut.head.len() + cut.tail.len() <= 200);
        assert_eq!(
            cut.head.len() + cut.omitted_bytes + cut.tail.len(),
            text.len()
        );
        assert!(!cut.whole_lines);
        assert_eq!(
            omitted_text(&cut, 0, 0),
            format!("{} bytes", cut.omitted_bytes)
        );
        // Short lines around one huge line: the head still keeps its lines.
        let text = format!("a\nb\n{}\nz\n", "x".repeat(1000));
        let cut = head_tail(&text, 100).expect("truncated");
        assert!(cut.head.starts_with("a\nb\n") && cut.head.len() > 30);
        assert!(cut.tail.ends_with("z\n") && cut.tail.len() > 20);
        // The split form cuts inside a long line the same way.
        let long = "y".repeat(300);
        let cut = head_tail_split(&long, &long, 100);
        assert_eq!((cut.head.len(), cut.tail.len()), (60, 40));
        assert_eq!(cut.omitted_bytes, 500);
        assert_eq!(omitted_text(&cut, 7, 1000), "1500 bytes");
    }
}
