//! Reading a window of lines from a file without loading the whole of it,
//! shared by the `read` tool and the ACP client's `fs/read_text_file`.

use std::io::{self, BufRead, BufReader, Read};

/// Lines `first..` of a file, read without holding more than the window.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LineWindow {
    /// The lines in the window, decoded as lossy UTF-8, without their line
    /// ending (`\n` or `\r\n`). A line longer than the per-line cap keeps
    /// only its beginning.
    pub lines: Vec<WindowLine>,
    /// Lines in the whole file, when they were counted.
    pub total_lines: Option<usize>,
    /// The window stopped at its byte budget before its line count.
    pub hit_byte_budget: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WindowLine {
    pub text: String,
    /// The line was longer than the per-line cap and was cut.
    pub cut: bool,
}

/// What to read with [`read_line_window`].
#[derive(Debug, Clone, Copy)]
pub struct WindowSpec {
    /// 0-based index of the first line.
    pub first: usize,
    pub max_lines: usize,
    /// Bytes of line text the window collects at most; the line that would
    /// pass it is left out, unless it is the first one.
    pub max_bytes: usize,
    /// Bytes kept of any one line.
    pub max_line_bytes: usize,
    /// Read on to the end to count every line of the file.
    pub count_total: bool,
}

/// Read a window of lines from `reader` in one pass, keeping only the
/// window in memory however large the input or any line in it. `stop` is
/// polled between chunks and ends the read early, with `total_lines` left
/// as `None`.
pub fn read_line_window(
    reader: impl Read,
    spec: WindowSpec,
    stop: &dyn Fn() -> bool,
) -> io::Result<LineWindow> {
    let mut reader = BufReader::with_capacity(64 * 1024, reader);
    let mut window = LineWindow::default();
    let mut collected = 0usize;
    let mut collecting = spec.max_lines > 0;
    // The line being read: its index, the bytes kept so far, whether it
    // outgrew the cap, whether it has any byte at all.
    let mut index = 0usize;
    let mut current: Vec<u8> = Vec::new();
    let mut cut = false;
    let mut started = false;

    loop {
        if stop() {
            return Ok(window);
        }
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let in_window = collecting && index >= spec.first;
        let (piece, ends_line) = match chunk.iter().position(|&b| b == b'\n') {
            Some(at) => (&chunk[..at], true),
            None => (chunk, false),
        };
        let consumed = piece.len() + usize::from(ends_line);
        started |= !piece.is_empty();
        if in_window {
            let room = spec.max_line_bytes.saturating_sub(current.len());
            current.extend_from_slice(&piece[..room.min(piece.len())]);
            cut |= piece.len() > room;
        }
        reader.consume(consumed);
        if !ends_line {
            continue;
        }
        if in_window {
            let line = finish_line(&mut current, cut);
            if !window.lines.is_empty() && collected + line.text.len() > spec.max_bytes {
                window.hit_byte_budget = true;
                collecting = false;
            } else {
                collected += line.text.len();
                window.lines.push(line);
                collecting = window.lines.len() < spec.max_lines;
            }
        }
        index += 1;
        cut = false;
        started = false;
        if !collecting && !spec.count_total {
            return Ok(window);
        }
    }
    // A last line without a trailing newline still counts.
    if started {
        if collecting && index >= spec.first {
            let line = finish_line(&mut current, cut);
            if window.lines.is_empty() || collected + line.text.len() <= spec.max_bytes {
                window.lines.push(line);
            } else {
                window.hit_byte_budget = true;
            }
        }
        index += 1;
    }
    if spec.count_total {
        window.total_lines = Some(index);
    }
    Ok(window)
}

fn finish_line(bytes: &mut Vec<u8>, cut: bool) -> WindowLine {
    if !cut && bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let text = String::from_utf8_lossy(bytes).into_owned();
    bytes.clear();
    WindowLine { text, cut }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(first: usize, max_lines: usize) -> WindowSpec {
        WindowSpec {
            first,
            max_lines,
            max_bytes: usize::MAX,
            max_line_bytes: usize::MAX,
            count_total: true,
        }
    }

    fn texts(window: &LineWindow) -> Vec<&str> {
        window.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn a_window_is_cut_from_the_middle_and_the_rest_counted() {
        let body: String = (1..=10).map(|n| format!("l{n}\r\n")).collect();
        let window = read_line_window(body.as_bytes(), spec(3, 2), &|| false).unwrap();
        assert_eq!(texts(&window), ["l4", "l5"]);
        assert_eq!(window.total_lines, Some(10));

        let without_count = WindowSpec {
            count_total: false,
            ..spec(3, 2)
        };
        let window = read_line_window(body.as_bytes(), without_count, &|| false).unwrap();
        assert_eq!(window.total_lines, None);
    }

    #[test]
    fn a_last_line_without_newline_counts() {
        let window = read_line_window(&b"a\nb"[..], spec(0, 10), &|| false).unwrap();
        assert_eq!(texts(&window), ["a", "b"]);
        assert_eq!(window.total_lines, Some(2));
        let empty = read_line_window(&b""[..], spec(0, 10), &|| false).unwrap();
        assert_eq!(empty.total_lines, Some(0));
    }

    #[test]
    fn a_huge_line_keeps_only_its_beginning() {
        let mut body = vec![b'x'; 1_000_000];
        body.extend_from_slice(b"\nnext\n");
        let spec = WindowSpec {
            max_line_bytes: 100,
            ..spec(0, 10)
        };
        let window = read_line_window(&body[..], spec, &|| false).unwrap();
        assert_eq!(window.lines[0].text.len(), 100);
        assert!(window.lines[0].cut);
        assert_eq!(window.lines[1].text, "next");
        assert!(!window.lines[1].cut);
    }

    #[test]
    fn the_byte_budget_ends_the_window_on_a_whole_line() {
        let spec = WindowSpec {
            max_bytes: 5,
            ..spec(0, 10)
        };
        let window = read_line_window(&b"abc\ndef\nghi\n"[..], spec, &|| false).unwrap();
        assert_eq!(texts(&window), ["abc"]);
        assert!(window.hit_byte_budget);
        assert_eq!(window.total_lines, Some(3));
    }
}
