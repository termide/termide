//! Asking the host terminal what it can do, in one round trip.
//!
//! Startup needs two answers: whether the terminal speaks the Kitty keyboard
//! protocol, and whether U+FE0F widens the emoji before it. Asked one at a
//! time through crossterm, each query waits up to two seconds for a reply,
//! so a terminal or relay that answers none of them held startup for six.
//! Here every query goes out at once, followed by a primary device
//! attributes request (`CSI c`), which every terminal worth the name answers
//! and answers last: its reply ends the wait, and a shared deadline bounds it
//! for one that stays silent.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use crate::terminal_modes::variation_selector_width_override;

/// How long to wait for a silent terminal. A terminal that answers ends the
/// wait with its reply to `CSI c`, in milliseconds even over SSH, so this
/// only bounds the one that never does; it leaves room for a slow link.
const PROBE_DEADLINE: Duration = Duration::from_millis(1000);

/// What the host terminal answered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalProbe {
    /// It answered the Kitty keyboard protocol query.
    pub keyboard_enhancement: bool,
    /// Whether `⏱️` took two columns; `None` when the cursor reports did
    /// not come back. [`crate::VS16_WIDTH_ENV`] wins over the probe.
    pub vs16_wide: Option<bool>,
}

/// Ask the terminal on stdin/stdout both questions at once; `ask_keyboard`
/// false leaves the Kitty query out (over SSH, where termide does not use
/// the protocol). Not a terminal: nothing is asked and nothing is known.
pub fn probe_terminal(ask_keyboard: bool) -> TerminalProbe {
    let forced_width = variation_selector_width_override();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return TerminalProbe {
            keyboard_enhancement: false,
            vs16_wide: forced_width,
        };
    }
    let ask_width = forced_width.is_none();
    let mut probe = ask(ask_keyboard, ask_width);
    if forced_width.is_some() {
        probe.vs16_wide = forced_width;
    }
    probe
}

#[cfg(unix)]
fn ask(ask_keyboard: bool, ask_width: bool) -> TerminalProbe {
    use std::os::fd::AsRawFd;

    // The replies come as input, which only raw mode hands over unechoed and
    // byte by byte; the mode the caller had is put back.
    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && crossterm::terminal::enable_raw_mode().is_err() {
        return TerminalProbe::default();
    }
    let mut query = String::new();
    if ask_width {
        // Save the cursor, report it, draw the glyph, report it again, and
        // put the cursor back over a cleared line.
        query.push_str("\x1b7\x1b[6n\u{23F1}\u{FE0F}\x1b[6n\x1b8\x1b[K");
    }
    if ask_keyboard {
        query.push_str("\x1b[?u");
    }
    query.push_str("\x1b[c");
    let mut stdout = io::stdout();
    let sent = stdout
        .write_all(query.as_bytes())
        .and_then(|()| stdout.flush());
    let mut replies = Vec::new();
    if sent.is_ok() {
        let fd = io::stdin().as_raw_fd();
        let deadline = std::time::Instant::now() + PROBE_DEADLINE;
        let mut chunk = [0u8; 256];
        while !parse_replies(&replies).device_attributes {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            let mut pollfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
            // SAFETY: one valid pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if ready <= 0 {
                break;
            }
            // SAFETY: `chunk` is valid for `chunk.len()` bytes.
            let read = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            let Ok(read) = usize::try_from(read) else {
                break;
            };
            if read == 0 {
                break;
            }
            replies.extend_from_slice(&chunk[..read]);
        }
    }
    if !was_raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    let parsed = parse_replies(&replies);
    TerminalProbe {
        keyboard_enhancement: ask_keyboard && parsed.keyboard_flags,
        vs16_wide: match parsed.cursor_columns[..] {
            [first, second, ..] if ask_width => Some(second.saturating_sub(first) >= 2),
            _ => None,
        },
    }
}

/// Elsewhere the queries go through crossterm, one at a time.
#[cfg(not(unix))]
fn ask(ask_keyboard: bool, ask_width: bool) -> TerminalProbe {
    TerminalProbe {
        keyboard_enhancement: ask_keyboard
            && crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false),
        vs16_wide: if ask_width {
            crate::terminal_modes::probe_variation_selector_width()
        } else {
            None
        },
    }
}

/// The replies found in what the terminal sent back.
#[derive(Debug, Default, PartialEq, Eq)]
struct Replies {
    /// The column of each cursor position report (`CSI row ; col R`), in
    /// order.
    cursor_columns: Vec<u16>,
    /// A Kitty keyboard flags report (`CSI ? flags u`) came.
    keyboard_flags: bool,
    /// The primary device attributes (`CSI ? … c`) came: nothing else will.
    device_attributes: bool,
}

/// Read the control sequences out of `bytes`, skipping anything else — a key
/// typed during startup, a reply cut off by the deadline.
fn parse_replies(bytes: &[u8]) -> Replies {
    let mut replies = Replies::default();
    let mut rest = bytes;
    while let Some(start) = rest.windows(2).position(|w| w == b"\x1b[") {
        let body = &rest[start + 2..];
        let Some(end) = body.iter().position(|b| (0x40..=0x7e).contains(b)) else {
            break;
        };
        let (params, last) = (&body[..end], body[end]);
        let private = params.first() == Some(&b'?');
        match last {
            b'R' if !private => {
                let column = std::str::from_utf8(params)
                    .ok()
                    .and_then(|text| text.split_once(';'))
                    .and_then(|(_, column)| column.parse::<u16>().ok());
                if let Some(column) = column {
                    replies.cursor_columns.push(column);
                }
            }
            b'u' if private => replies.keyboard_flags = true,
            b'c' if private => replies.device_attributes = true,
            _ => {}
        }
        rest = &body[end + 1..];
    }
    replies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_replies_are_read_in_any_company() {
        let replies = parse_replies(b"\x1b[5;1R\x1b[5;3R\x1b[?1u\x1b[?62;22c");
        assert_eq!(
            replies,
            Replies {
                cursor_columns: vec![1, 3],
                keyboard_flags: true,
                device_attributes: true,
            }
        );
        // A terminal without the Kitty protocol answers the rest; a key
        // typed meanwhile is skipped.
        let replies = parse_replies(b"x\x1b[2;10R\x1b[2;11R\x1b[?6c");
        assert_eq!(replies.cursor_columns, [10, 11]);
        assert!(!replies.keyboard_flags);
        assert!(replies.device_attributes);
        // Cut off by the deadline mid-reply: what came whole counts.
        let replies = parse_replies(b"\x1b[3;4R\x1b[3;");
        assert_eq!(replies.cursor_columns, [4]);
        assert!(!replies.device_attributes);
        assert_eq!(parse_replies(b""), Replies::default());
    }
}
