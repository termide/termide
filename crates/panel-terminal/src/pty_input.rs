//! Bytes on their way into a terminal panel's PTY.
//!
//! A write to a PTY blocks once the program on the other side stops reading
//! and the kernel buffer (about 1 KB in raw mode) fills up — a program that
//! hung, a stalled ssh. Neither the UI thread (keys, pastes) nor the reader
//! thread (replies to queries such as `\e[6n`) may ever wait for that, so all
//! writes go through a queue drained by one writer thread per panel. Only
//! that thread blocks; when the panel closes, the queue is dropped and the
//! thread exits as soon as its write returns (killing the program hangs the
//! PTY up, which fails the write).

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Most user input (keys, pastes, typed commands) held for a program that is
/// not reading. Beyond it new input is refused rather than buffered.
pub const MAX_PENDING_INPUT: usize = 16 * 1024 * 1024;

/// Most query replies held for a program that is not reading. A program
/// that keeps asking without reading the answers gets no further ones.
pub const MAX_PENDING_REPLIES: usize = 4 * 1024;

/// Why input was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    /// The program has left too much input unread.
    Full,
    /// The panel closed or the program exited.
    Closed,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Full => "the program in the terminal is not reading its input",
            Self::Closed => "the terminal is closed",
        })
    }
}

impl std::error::Error for InputError {}

#[derive(Default)]
struct Queue {
    /// Pending writes, in the order they were made; `true` marks a reply.
    chunks: VecDeque<(bool, Vec<u8>)>,
    input_bytes: usize,
    reply_bytes: usize,
    /// The panel closed or a write failed: nothing more is written.
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        let mut queue = self.lock();
        queue.closed = true;
        queue.chunks.clear();
        queue.input_bytes = 0;
        queue.reply_bytes = 0;
        drop(queue);
        self.ready.notify_all();
    }

    /// Next chunk to write, waiting for one; `None` once closed.
    fn next(&self) -> Option<Vec<u8>> {
        let mut queue = self.lock();
        loop {
            if queue.closed {
                return None;
            }
            if let Some((reply, chunk)) = queue.chunks.pop_front() {
                if reply {
                    queue.reply_bytes -= chunk.len();
                } else {
                    queue.input_bytes -= chunk.len();
                }
                return Some(chunk);
            }
            queue = self.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn push(&self, reply: bool, data: Vec<u8>) -> Result<(), InputError> {
        if data.is_empty() {
            return Ok(());
        }
        let mut queue = self.lock();
        if queue.closed {
            return Err(InputError::Closed);
        }
        let (used, limit) = if reply {
            (queue.reply_bytes, MAX_PENDING_REPLIES)
        } else {
            (queue.input_bytes, MAX_PENDING_INPUT)
        };
        if used + data.len() > limit {
            return Err(InputError::Full);
        }
        if reply {
            queue.reply_bytes += data.len();
        } else {
            queue.input_bytes += data.len();
        }
        queue.chunks.push_back((reply, data));
        drop(queue);
        self.ready.notify_one();
        Ok(())
    }
}

/// The panel's end of the queue. Dropping it closes the queue: what is
/// still pending is discarded and the writer thread ends.
pub struct PtyInput {
    shared: Arc<Shared>,
}

impl PtyInput {
    /// Start the writer thread that feeds `writer`.
    pub fn spawn(mut writer: Box<dyn Write + Send>) -> Self {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || {
                while let Some(chunk) = thread_shared.next() {
                    if writer
                        .write_all(&chunk)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        // The program is gone; later input has nowhere to go.
                        thread_shared.close();
                    }
                }
            });
        if spawned.is_err() {
            shared.close();
        }
        Self { shared }
    }

    /// A queue nothing drains, for tests that inspect what was written.
    #[cfg(test)]
    pub fn unspawned() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
        }
    }

    /// Queue `data` (keys, a paste) for the program. Never blocks; refused
    /// when the program has left too much unread.
    pub fn send(&self, data: Vec<u8>) -> Result<(), InputError> {
        self.shared.push(false, data)
    }

    /// The handle the reader thread answers queries through.
    pub fn replies(&self) -> ReplySink {
        ReplySink {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Everything queued and not yet written, emptied out.
    #[cfg(test)]
    pub fn take_queued(&self) -> Vec<u8> {
        let mut queue = self.shared.lock();
        queue.input_bytes = 0;
        queue.reply_bytes = 0;
        queue.chunks.drain(..).flat_map(|(_, c)| c).collect()
    }
}

impl Drop for PtyInput {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// Where the parser sends its replies to terminal queries.
#[derive(Clone)]
pub struct ReplySink {
    shared: Arc<Shared>,
}

impl ReplySink {
    /// Queue a reply. Never blocks; dropped when the program has not read
    /// the earlier ones, or the panel is closed.
    pub fn reply(&self, data: &[u8]) {
        let _ = self.shared.push(true, data.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A writer stuck the way a PTY is when its program stops reading:
    /// each write waits until the test lets it through.
    struct Stuck {
        gate: mpsc::Receiver<()>,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Stuck {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.gate
                .recv()
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn stuck() -> (PtyInput, mpsc::Sender<()>, Arc<Mutex<Vec<u8>>>) {
        let (gate_tx, gate) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let input = PtyInput::spawn(Box::new(Stuck {
            gate,
            written: Arc::clone(&written),
        }));
        (input, gate_tx, written)
    }

    fn wait_for(written: &Mutex<Vec<u8>>, expected: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while written.lock().unwrap().as_slice() != expected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(written.lock().unwrap().as_slice(), expected);
    }

    /// Input and replies reach the program in the order they were made.
    #[test]
    fn writes_keep_their_order() {
        let (input, gate, written) = stuck();
        input.send(b"ab".to_vec()).unwrap();
        input.replies().reply(b"\x1b[1;1R");
        input.send(b"c".to_vec()).unwrap();
        for _ in 0..3 {
            gate.send(()).unwrap();
        }
        wait_for(&written, b"ab\x1b[1;1Rc");
    }

    /// A program that stops reading never holds up the sender: input piles
    /// up to its limit and is then refused, still without blocking.
    #[test]
    fn a_program_that_stops_reading_never_blocks_the_sender() {
        let (input, _gate, _written) = stuck();
        let start = Instant::now();
        let chunk = vec![b'x'; 1024 * 1024];
        let mut refused = false;
        for _ in 0..=MAX_PENDING_INPUT / chunk.len() + 1 {
            if let Err(InputError::Full) = input.send(chunk.clone()) {
                refused = true;
            }
        }
        assert!(refused, "input past the limit was buffered");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    /// Replies to a program flooding queries without reading are dropped
    /// once their budget is used, and leave user input unaffected.
    #[test]
    fn replies_past_their_budget_are_dropped() {
        let input = PtyInput::unspawned();
        let replies = input.replies();
        for _ in 0..10_000 {
            replies.reply(b"\x1b[24;80R");
        }
        let queued = input.take_queued();
        assert!(queued.len() <= MAX_PENDING_REPLIES, "{}", queued.len());
        assert!(!queued.is_empty());
        input.send(b"typed".to_vec()).unwrap();
        assert_eq!(input.take_queued(), b"typed");
    }

    /// Closing the panel while the writer is stuck returns at once, and the
    /// writer stops once its write returns.
    #[test]
    fn closing_while_stuck_returns_at_once() {
        let (input, gate, written) = stuck();
        input.send(b"first".to_vec()).unwrap();
        input.send(b"second".to_vec()).unwrap();
        let replies = input.replies();
        let start = Instant::now();
        drop(input);
        assert!(start.elapsed() < Duration::from_millis(500));
        replies.reply(b"late");
        // The write in flight completes; nothing queued after it is written.
        let _ = gate.send(());
        let _ = gate.send(());
        std::thread::sleep(Duration::from_millis(50));
        let written = written.lock().unwrap().clone();
        assert!(written.is_empty() || written == b"first", "{written:?}");
    }

    /// A failed write (the program exited) closes the queue.
    #[test]
    fn a_failed_write_closes_the_queue() {
        let (input, gate, _written) = stuck();
        drop(gate);
        input.send(b"x".to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match input.send(b"y".to_vec()) {
                Err(InputError::Closed) => break,
                _ if Instant::now() > deadline => panic!("queue never closed"),
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}
