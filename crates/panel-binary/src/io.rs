//! Windowed byte reads (with pending-edit overlay) and clipboard copy.

use std::io::{Read, Seek, SeekFrom};

use termide_core::PanelEvent;

use crate::{BinaryPanel, Zone};

/// Upper bound on a single clipboard copy, so a huge selection can't allocate
/// without limit.
const MAX_COPY: u64 = 1 << 20;

impl BinaryPanel {
    /// Read up to `count` bytes starting at `start` from the file.
    pub(crate) fn read_window(&mut self, start: u64, count: usize) -> Vec<u8> {
        let Some(file) = self.file.as_mut() else {
            return Vec::new();
        };
        if start >= self.len {
            return Vec::new();
        }
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let want = count.min((self.len - start) as usize);
        let mut buf = vec![0u8; want];
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        buf.truncate(n);
        // Overlay unsaved edits that fall in this window.
        if !self.edits.is_empty() {
            let end = start + buf.len() as u64;
            for (&off, &b) in self.edits.range(start..end) {
                buf[(off - start) as usize] = b;
            }
        }
        buf
    }

    /// Copy the selection (or cursor byte) to the clipboard — as a hex string
    /// in the hex zone, as text in the ASCII zone.
    pub(crate) fn copy_selection(&mut self) -> Vec<PanelEvent> {
        let (s, e) = self.sel_range();
        let count = (e - s + 1).min(MAX_COPY) as usize;
        let bytes = self.read_window(s, count);
        let text = match self.zone {
            Zone::Hex => bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            Zone::Ascii => String::from_utf8_lossy(&bytes).to_string(),
        };
        let _ = termide_clipboard::copy(&text);
        vec![PanelEvent::NeedsRedraw]
    }
}
