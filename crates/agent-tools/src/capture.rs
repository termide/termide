//! Bounded capture of a running command's output.
//!
//! A command may print without end until its timeout (`yes`, a runaway log),
//! so the output is not kept whole: memory holds its first and last
//! `side_bytes` and counts what falls in between. Once anything is about to
//! be dropped, the raw stream is spooled to the log file instead, so the full
//! record still exists on disk — up to `max_log_bytes`, past which only the
//! count is kept.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

pub(crate) struct Capture {
    side_bytes: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    /// Every byte the command wrote.
    total: u64,
    /// Bytes between head and tail no longer in memory, and their newlines.
    dropped: u64,
    dropped_lines: u64,
    spool: Spool,
    log_path: Option<PathBuf>,
    max_log_bytes: u64,
}

enum Spool {
    /// Nothing dropped yet: memory holds the whole output.
    Idle,
    Writing {
        file: BufWriter<File>,
        written: u64,
    },
    /// The log reached its cap; `skipped` bytes after it were not saved.
    Capped {
        file: BufWriter<File>,
        skipped: u64,
    },
    /// The log could not be opened or written.
    Failed,
}

/// What the command wrote once it is over.
pub(crate) struct Captured {
    /// The output when nothing was dropped, else its first part.
    pub head: Vec<u8>,
    /// The last part when something was dropped (empty otherwise).
    pub tail: Vec<u8>,
    pub dropped: u64,
    pub dropped_lines: u64,
    /// The spooled log, when the output outgrew memory and could be saved.
    pub log: Option<SavedLog>,
}

pub(crate) struct SavedLog {
    pub path: PathBuf,
    /// Bytes not saved because the log reached its cap.
    pub skipped: u64,
}

impl Capture {
    pub fn new(side_bytes: usize, log_path: Option<PathBuf>, max_log_bytes: u64) -> Self {
        Self {
            side_bytes: side_bytes.max(1),
            head: Vec::new(),
            tail: VecDeque::new(),
            total: 0,
            dropped: 0,
            dropped_lines: 0,
            spool: Spool::Idle,
            log_path,
            max_log_bytes,
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn push(&mut self, bytes: &[u8]) {
        let in_memory = (self.head.len() + self.tail.len()) as u64;
        if matches!(self.spool, Spool::Idle)
            && in_memory + bytes.len() as u64 > 2 * self.side_bytes as u64
        {
            self.start_spool();
        }
        self.spool_write(bytes);
        self.total += bytes.len() as u64;

        let room = self.side_bytes.saturating_sub(self.head.len());
        let (to_head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(to_head);
        self.tail.extend(rest);
        if self.tail.len() > self.side_bytes {
            let excess = self.tail.len() - self.side_bytes;
            self.dropped += excess as u64;
            self.dropped_lines += self.tail.range(..excess).filter(|&&b| b == b'\n').count() as u64;
            self.tail.drain(..excess);
        }
    }

    /// Write what memory holds so far to the log and keep appending to it.
    fn start_spool(&mut self) {
        let Some(path) = &self.log_path else {
            self.spool = Spool::Failed;
            return;
        };
        self.spool = match File::create(path) {
            Ok(file) => Spool::Writing {
                file: BufWriter::new(file),
                written: 0,
            },
            Err(error) => {
                log::warn!("cannot save bash output to {}: {error}", path.display());
                Spool::Failed
            }
        };
        let head = std::mem::take(&mut self.head);
        self.spool_write(&head);
        self.head = head;
        let (a, b) = self.tail.as_slices();
        let (a, b) = (a.to_vec(), b.to_vec());
        self.spool_write(&a);
        self.spool_write(&b);
    }

    fn spool_write(&mut self, bytes: &[u8]) {
        let cap = self.max_log_bytes;
        let spool = std::mem::replace(&mut self.spool, Spool::Failed);
        self.spool = match spool {
            Spool::Writing { mut file, written } => {
                let fits = cap.saturating_sub(written).min(bytes.len() as u64) as usize;
                if file.write_all(&bytes[..fits]).is_err() {
                    Spool::Failed
                } else if fits < bytes.len() {
                    Spool::Capped {
                        file,
                        skipped: (bytes.len() - fits) as u64,
                    }
                } else {
                    Spool::Writing {
                        file,
                        written: written + fits as u64,
                    }
                }
            }
            Spool::Capped { file, skipped } => Spool::Capped {
                file,
                skipped: skipped + bytes.len() as u64,
            },
            other => other,
        };
    }

    /// A view of the output so far for the live display, built only from
    /// what changed since `view` was last updated while nothing is dropped.
    pub fn update_view(&self, view: &mut LiveView) {
        if self.dropped == 0 {
            let head_len = self.head.len() as u64;
            let from = view.consumed;
            if from < head_len {
                view.decode(&self.head[from as usize..]);
            }
            let tail_from = from.saturating_sub(head_len) as usize;
            let (a, b) = self.tail.as_slices();
            if tail_from < a.len() {
                view.decode(&a[tail_from..]);
                view.decode(b);
            } else {
                view.decode(&b[(tail_from - a.len()).min(b.len())..]);
            }
            view.consumed = self.total;
            return;
        }
        // Past the first drop the head no longer changes; only the bounded
        // tail is decoded again.
        let head = view
            .head
            .get_or_insert_with(|| String::from_utf8_lossy(&self.head).into_owned());
        let (a, b) = self.tail.as_slices();
        let mut tail = Vec::with_capacity(self.tail.len());
        tail.extend_from_slice(a);
        tail.extend_from_slice(b);
        view.text = format!(
            "{head}\n[... {} bytes omitted ...]\n{}",
            self.dropped,
            String::from_utf8_lossy(&tail)
        );
        view.pending.clear();
        view.consumed = self.total;
    }

    pub fn finish(self) -> Captured {
        let log_path = self.log_path;
        let log = match self.spool {
            Spool::Writing { mut file, .. } => file.flush().ok().map(|()| 0),
            Spool::Capped { mut file, skipped } => {
                let note =
                    format!("\n[termide: log stopped here; {skipped} more bytes were not saved]\n");
                (file.write_all(note.as_bytes()).is_ok() && file.flush().is_ok()).then_some(skipped)
            }
            Spool::Idle | Spool::Failed => None,
        }
        .and_then(|skipped| log_path.map(|path| SavedLog { path, skipped }));
        Captured {
            head: self.head,
            tail: self.tail.into(),
            dropped: self.dropped,
            dropped_lines: self.dropped_lines,
            log,
        }
    }
}

/// The live display's text, kept up to date incrementally.
#[derive(Default)]
pub(crate) struct LiveView {
    pub text: String,
    /// The start of a UTF-8 sequence split across reads.
    pending: Vec<u8>,
    /// How many output bytes `text` reflects.
    pub consumed: u64,
    /// The decoded head, once the output outgrew memory.
    head: Option<String>,
}

impl LiveView {
    /// Append `bytes` as lossy UTF-8, holding back an incomplete trailing
    /// sequence until the next call completes it.
    fn decode(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        loop {
            match std::str::from_utf8(&self.pending[start..]) {
                Ok(valid) => {
                    self.text.push_str(valid);
                    start = self.pending.len();
                    break;
                }
                Err(error) => {
                    let valid_end = start + error.valid_up_to();
                    if let Ok(valid) = std::str::from_utf8(&self.pending[start..valid_end]) {
                        self.text.push_str(valid);
                    }
                    match error.error_len() {
                        Some(len) => {
                            self.text.push(char::REPLACEMENT_CHARACTER);
                            start = valid_end + len;
                        }
                        None => {
                            start = valid_end;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_stays_whole_and_writes_no_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        let mut capture = Capture::new(16, Some(log.clone()), 1024);
        capture.push(b"hello ");
        capture.push(b"world\n");
        let captured = capture.finish();
        assert_eq!(captured.head, b"hello world\n");
        assert!(captured.tail.is_empty());
        assert_eq!(captured.dropped, 0);
        assert!(captured.log.is_none());
        assert!(!log.exists());
    }

    #[test]
    fn long_output_keeps_head_and_tail_and_spools_everything() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        let mut capture = Capture::new(8, Some(log.clone()), 1 << 20);
        let mut all = Vec::new();
        for n in 0..100 {
            let line = format!("{n}\n");
            all.extend_from_slice(line.as_bytes());
            capture.push(line.as_bytes());
        }
        let captured = capture.finish();
        assert_eq!(captured.head, &all[..8]);
        assert_eq!(captured.tail, &all[all.len() - 8..]);
        assert_eq!(captured.dropped as usize, all.len() - 16);
        let middle = &all[8..all.len() - 8];
        assert_eq!(
            captured.dropped_lines as usize,
            middle.iter().filter(|&&b| b == b'\n').count()
        );
        let saved = captured.log.unwrap();
        assert_eq!(saved.skipped, 0);
        assert_eq!(std::fs::read(&saved.path).unwrap(), all);
    }

    #[test]
    fn the_log_stops_at_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        let mut capture = Capture::new(4, Some(log.clone()), 20);
        for _ in 0..10 {
            capture.push(b"abcdefgh");
        }
        let captured = capture.finish();
        assert_eq!(captured.log.unwrap().skipped, 60);
        let saved = std::fs::read_to_string(&log).unwrap();
        assert!(saved.starts_with("abcdefghabcdefghabcd\n[termide: log stopped"));
    }

    #[test]
    fn the_live_view_decodes_incrementally_across_split_characters() {
        let mut capture = Capture::new(64, None, 0);
        let mut view = LiveView::default();
        let text = "añb€c\n".as_bytes();
        for byte in text {
            capture.push(std::slice::from_ref(byte));
            capture.update_view(&mut view);
        }
        assert_eq!(view.text, "añb€c\n");
    }

    #[test]
    fn the_live_view_shows_head_and_tail_once_output_is_dropped() {
        let mut capture = Capture::new(4, None, 0);
        let mut view = LiveView::default();
        capture.push(b"HEAD");
        capture.update_view(&mut view);
        capture.push(b"middle-part");
        capture.push(b"TAIL");
        capture.update_view(&mut view);
        assert_eq!(view.text, "HEAD\n[... 11 bytes omitted ...]\nTAIL");
    }
}
