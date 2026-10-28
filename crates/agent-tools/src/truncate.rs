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
/// replaces with an omission marker.
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

    let omitted_lines = text[head_end..tail_start].matches('\n').count();
    Some(HeadTail {
        head: &text[..head_end],
        tail: &text[tail_start..],
        omitted_lines,
    })
}

/// [`head_tail`] for output whose middle is already gone: the head comes
/// from `head`, the tail from `tail`, with the same budgets and line cuts.
/// `omitted_lines` counts only the lines cut from these two texts.
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
    HeadTail {
        head: &head[..head_end],
        tail: &tail[tail_start..],
        omitted_lines: head[head_end..].matches('\n').count()
            + tail[..tail_start].matches('\n').count(),
    }
}

/// End of the longest run of whole lines at the start of `text` within `budget`.
fn head_end(text: &str, budget: usize) -> usize {
    let mut end = 0;
    for (index, _) in text.match_indices('\n') {
        if index + 1 > budget {
            break;
        }
        end = index + 1;
    }
    end
}

/// Start of the longest run of whole lines at the end of `text` within `budget`.
fn tail_start(text: &str, budget: usize) -> usize {
    let mut start = text.len();
    for (index, _) in text.rmatch_indices('\n') {
        if text.len() - (index + 1) > budget {
            break;
        }
        start = index + 1;
    }
    start
}

#[derive(Debug, PartialEq, Eq)]
pub struct HeadTail<'a> {
    pub head: &'a str,
    pub tail: &'a str,
    pub omitted_lines: usize,
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
    }
}
