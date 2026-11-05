//! The raw output of a terminal's program, as it came, for a reader other
//! than the screen: the agent reads what a command printed in its shell
//! session from here. The latest bytes stay in memory, each reader keeping
//! its own place by offset; all of them go to a log file, up to a cap.
//!
//! The tap also notes where the shell marks its prompt and a command's start
//! and end (OSC 133 `A`, `C` and `D`), as the bytes arrive: a mark is found even when the
//! bytes around it are long gone from memory.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What [`OutputTap::read_since`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapChunk {
    /// The bytes from `from` to `end`.
    pub data: Vec<u8>,
    /// Where `data` starts: the offset asked for, or later when the bytes
    /// before it are no longer held in memory.
    pub from: u64,
    /// The offset just past `data`: where the next read goes on.
    pub end: u64,
    /// The program closed its terminal: nothing comes after `end`.
    pub closed: bool,
}

/// What [`OutputTap::read_marked`] found, all of it as of one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedChunk {
    pub chunk: TapChunk,
    /// The marks at or past the offset asked for, oldest first: every mark
    /// that ends within `chunk`.
    pub marks: Vec<Mark>,
    /// Where the bytes of `chunk` stop being settled: the first byte of an
    /// escape sequence still arriving, which may yet turn out a mark, or
    /// `chunk.end` when none is.
    pub settled: u64,
}

/// A shell's mark of where a command's output starts or ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// OSC 133 `A`: the shell shows its prompt, ready for a command.
    Prompt,
    /// OSC 133 `C`: the command starts; its output follows.
    Start,
    /// OSC 133 `D`: the command is over, with its exit code when given.
    End(Option<i32>),
}

/// A [`MarkKind`] and where it stands in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    /// The offset of the mark's first byte.
    pub at: u64,
    /// The offset just past it.
    pub after: u64,
}

/// Marks kept; a reader looks for the latest few, far fewer than this.
const MARKS_KEPT: usize = 256;
/// The longest OSC the scanner holds on to; a mark is far shorter.
const OSC_MAX: usize = 32;

/// Finds marks in output that arrives in pieces.
#[derive(Default)]
struct Scanner {
    /// An escape sequence under way: its offset and bytes so far.
    pending: Option<(u64, Vec<u8>)>,
}

impl Scanner {
    /// Scan `data`, which starts at offset `start`.
    fn feed(&mut self, start: u64, data: &[u8], marks: &mut VecDeque<Mark>) {
        for (i, &byte) in data.iter().enumerate() {
            let at = start + i as u64;
            if let Some((begin, bytes)) = &mut self.pending {
                let terminated = byte == 0x07 || (byte == b'\\' && bytes.last() == Some(&0x1b));
                if terminated {
                    let body = if byte == 0x07 {
                        &bytes[..]
                    } else {
                        &bytes[..bytes.len() - 1]
                    };
                    if let Some(kind) = parse_mark(body) {
                        if marks.len() == MARKS_KEPT {
                            marks.pop_front();
                        }
                        marks.push_back(Mark {
                            kind,
                            at: *begin,
                            after: at + 1,
                        });
                    }
                    self.pending = None;
                    continue;
                }
                let valid = match bytes.len() {
                    1 => byte == b']',
                    _ => bytes.len() < OSC_MAX && (byte != 0x1b || bytes.last() != Some(&0x1b)),
                };
                if valid {
                    bytes.push(byte);
                    continue;
                }
                self.pending = None;
            }
            if byte == 0x1b {
                self.pending = Some((at, vec![byte]));
            }
        }
    }
}

/// The mark an OSC body (`ESC ] …`, without its terminator) stands for.
fn parse_mark(body: &[u8]) -> Option<MarkKind> {
    let rest = body.strip_prefix(b"\x1b]133;")?;
    match rest {
        b"A" => Some(MarkKind::Prompt),
        [b'A', b';', ..] => Some(MarkKind::Prompt),
        b"C" => Some(MarkKind::Start),
        [b'D', code @ ..] => {
            let code = code
                .strip_prefix(b";")
                .and_then(|code| std::str::from_utf8(code).ok())
                .and_then(|code| code.parse().ok());
            Some(MarkKind::End(code))
        }
        _ => None,
    }
}

struct Log {
    path: PathBuf,
    file: File,
    written: u64,
    cap: u64,
}

struct State {
    /// The latest bytes; `start` is the offset of the first.
    ring: VecDeque<u8>,
    start: u64,
    keep: usize,
    closed: bool,
    log: Option<Log>,
    scanner: Scanner,
    marks: VecDeque<Mark>,
}

impl State {
    fn end(&self) -> u64 {
        self.start + self.ring.len() as u64
    }

    /// The bytes from offset `from` on, as far as memory still holds them.
    fn chunk(&self, from: u64) -> TapChunk {
        let end = self.end();
        let from = from.clamp(self.start, end);
        let skip = usize::try_from(from - self.start).unwrap_or(usize::MAX);
        TapChunk {
            data: self.ring.iter().skip(skip).copied().collect(),
            from,
            end,
            closed: self.closed,
        }
    }
}

/// The tap; shared by the terminal's reader thread, which fills it, and
/// whoever reads it.
pub struct OutputTap {
    state: Mutex<State>,
    changed: Condvar,
}

impl OutputTap {
    /// A tap keeping the latest `keep` bytes in memory, and every byte in
    /// the file at `log` up to `cap` bytes, when one is given and can be
    /// created.
    #[must_use]
    pub fn new(keep: usize, log: Option<(&Path, u64)>) -> Self {
        let log = log.and_then(|(path, cap)| {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match File::create(path) {
                Ok(file) => Some(Log {
                    path: path.to_path_buf(),
                    file,
                    written: 0,
                    cap,
                }),
                Err(error) => {
                    log::warn!("cannot log terminal output to {}: {error}", path.display());
                    None
                }
            }
        });
        Self {
            state: Mutex::new(State {
                ring: VecDeque::new(),
                start: 0,
                keep: keep.max(1),
                closed: false,
                log,
                scanner: Scanner::default(),
                marks: VecDeque::new(),
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take in what the program wrote.
    pub(crate) fn push(&self, data: &[u8]) {
        let mut state = self.lock();
        if let Some(log) = &mut state.log {
            let room = log.cap.saturating_sub(log.written);
            let part = &data[..data.len().min(usize::try_from(room).unwrap_or(usize::MAX))];
            if !part.is_empty() && log.file.write_all(part).is_ok() {
                log.written += part.len() as u64;
            }
        }
        let end = state.end();
        let State { scanner, marks, .. } = &mut *state;
        scanner.feed(end, data, marks);
        state.ring.extend(data);
        let excess = state.ring.len().saturating_sub(state.keep);
        if excess > 0 {
            state.ring.drain(..excess);
            state.start += excess as u64;
        }
        drop(state);
        self.changed.notify_all();
    }

    /// The program closed its terminal.
    pub(crate) fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        if let Some(log) = &mut state.log {
            let _ = log.file.flush();
        }
        drop(state);
        self.changed.notify_all();
    }

    /// The offset just past the last byte taken in.
    pub fn end(&self) -> u64 {
        self.lock().end()
    }

    /// Whether the program closed its terminal.
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Where every byte goes, while it fits the cap.
    pub fn log_path(&self) -> Option<PathBuf> {
        self.lock().log.as_ref().map(|log| log.path.clone())
    }

    /// The bytes from offset `from` on, as far as memory still holds them.
    pub fn read_since(&self, from: u64) -> TapChunk {
        self.lock().chunk(from)
    }

    /// The marks at or past offset `from`, oldest first.
    pub fn marks_since(&self, from: u64) -> Vec<Mark> {
        self.lock()
            .marks
            .iter()
            .filter(|mark| mark.at >= from)
            .copied()
            .collect()
    }

    /// The bytes from offset `from` on with the marks at or past
    /// `marks_from`, taken together: read one after the other, a mark could
    /// arrive between the two and its bytes pass for output.
    pub fn read_marked(&self, from: u64, marks_from: u64) -> MarkedChunk {
        let state = self.lock();
        let chunk = state.chunk(from);
        MarkedChunk {
            settled: (state.scanner.pending.as_ref()).map_or(chunk.end, |(at, _)| *at),
            chunk,
            marks: state
                .marks
                .iter()
                .filter(|mark| mark.at >= marks_from)
                .copied()
                .collect(),
        }
    }

    /// Wait until bytes past `from` come or the tap closes, `timeout` at
    /// most; whether either happened.
    pub fn wait_past(&self, from: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if state.end() > from || state.closed {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn readers_keep_their_own_place() {
        let tap = OutputTap::new(1024, None);
        tap.push(b"one ");
        let first = tap.read_since(0);
        assert_eq!((first.data.as_slice(), first.end), (&b"one "[..], 4));
        tap.push(b"two");
        assert_eq!(tap.read_since(first.end).data, b"two");
        assert_eq!(tap.read_since(0).data, b"one two");
    }

    #[test]
    fn what_memory_no_longer_holds_is_skipped_but_logged_up_to_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        let tap = OutputTap::new(4, Some((&path, 6)));
        tap.push(b"abcdefgh");
        let chunk = tap.read_since(0);
        assert_eq!(
            (chunk.data.as_slice(), chunk.from, chunk.end),
            (&b"efgh"[..], 4, 8)
        );
        tap.close();
        assert!(tap.read_since(8).closed);
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
    }

    #[test]
    fn a_prompt_is_marked_with_or_without_options() {
        let tap = OutputTap::new(64, None);
        tap.push(b"\x1b]133;A\x07$ \x1b]133;A;click_events=1\x1b\\> \x1b]133;B\x07\x1b]133;C;cmdline_url=ls\x07");
        let kinds: Vec<_> = tap.marks_since(0).iter().map(|mark| mark.kind).collect();
        assert_eq!(kinds, [MarkKind::Prompt, MarkKind::Prompt]);
    }

    #[test]
    fn marks_are_found_across_pieces_and_terminators() {
        let tap = OutputTap::new(4, None);
        for piece in [
            &b"$ ls\r\n\x1b]13"[..],
            b"3;C\x07a\r\nb\x1b]133;D;2\x1b\\",
            b"\x1b]0;title\x07\x1b]133;C\x07\x1b]133;D\x07",
        ] {
            tap.push(piece);
        }
        let marks = tap.marks_since(0);
        assert_eq!(
            marks,
            [
                Mark {
                    kind: MarkKind::Start,
                    at: 6,
                    after: 14
                },
                Mark {
                    kind: MarkKind::End(Some(2)),
                    at: 18,
                    after: 29
                },
                Mark {
                    kind: MarkKind::Start,
                    at: 39,
                    after: 47
                },
                Mark {
                    kind: MarkKind::End(None),
                    at: 47,
                    after: 55
                },
            ]
        );
        assert_eq!(tap.marks_since(19).len(), 2);
    }

    #[test]
    fn a_mark_still_arriving_is_not_settled_output() {
        let tap = OutputTap::new(64, None);
        tap.push(b"1\r\n\x1b]133;D");
        let read = tap.read_marked(0, 0);
        assert_eq!((read.chunk.end, read.settled), (10, 3));
        assert!(read.marks.is_empty());

        tap.push(b";0\x07");
        let read = tap.read_marked(0, 0);
        assert_eq!(read.settled, read.chunk.end);
        assert_eq!(
            read.marks,
            [Mark {
                kind: MarkKind::End(Some(0)),
                at: 3,
                after: 13
            }]
        );
    }

    #[test]
    fn a_waiting_reader_wakes_on_output() {
        let tap = Arc::new(OutputTap::new(16, None));
        assert!(!tap.wait_past(0, Duration::from_millis(10)));
        let writer = Arc::clone(&tap);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            writer.push(b"x");
        });
        assert!(tap.wait_past(0, Duration::from_secs(5)));
        thread.join().unwrap();
    }
}
