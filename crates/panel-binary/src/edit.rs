//! Overwrite-in-place byte editing (hex nibbles and ASCII characters).

use crossterm::event::{KeyCode, KeyModifiers};

use crate::{BinaryPanel, Zone};

impl BinaryPanel {
    /// Current value of the byte at `off` (pending edit or on-disk).
    fn byte_value(&mut self, off: u64) -> u8 {
        if let Some(&b) = self.edits.get(&off) {
            return b;
        }
        self.read_window(off, 1).first().copied().unwrap_or(0)
    }

    /// Record an overwrite of the byte at `off`.
    fn set_byte(&mut self, off: u64, b: u8) {
        self.edits.insert(off, b);
    }

    /// Move the cursor one byte forward after entering a value (no selection).
    fn advance_cursor(&mut self) {
        self.pending_nibble = None;
        self.anchor = None;
        if self.cursor + 1 < self.len {
            self.cursor += 1;
            self.ensure_cursor_visible();
        }
    }

    /// Apply a hex digit in the hex zone (two nibbles per byte).
    fn edit_hex_nibble(&mut self, d: u8) {
        let cur = self.byte_value(self.cursor);
        self.anchor = None;
        match self.pending_nibble.take() {
            None => {
                self.set_byte(self.cursor, (d << 4) | (cur & 0x0f));
                self.pending_nibble = Some(d);
            }
            Some(hi) => {
                self.set_byte(self.cursor, (hi << 4) | d);
                self.advance_cursor();
            }
        }
    }

    /// Try to interpret `key` as an edit (hex digit in the hex zone, printable
    /// char in the ASCII zone). Returns true if it was consumed.
    pub(crate) fn try_edit(&mut self, key: crossterm::event::KeyEvent) -> bool {
        // Only plain (or Shift) character keys edit; Ctrl/Alt fall through.
        if !(key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) {
            return false;
        }
        let KeyCode::Char(c) = key.code else {
            return false;
        };
        match self.zone {
            Zone::Hex => match c.to_digit(16) {
                Some(d) => {
                    self.edit_hex_nibble(d as u8);
                    true
                }
                None => false,
            },
            Zone::Ascii => {
                if c.is_ascii() && (' '..='~').contains(&c) {
                    self.set_byte(self.cursor, c as u8);
                    self.advance_cursor();
                    true
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    fn panel_with(len: u64, w: u16, h: u16) -> BinaryPanel {
        let mut p = BinaryPanel::new(PathBuf::from("/dev/null")).unwrap();
        p.len = len;
        p.last_area = Rect::new(0, 0, w, h);
        p
    }

    #[test]
    fn hex_nibble_editing_overwrites_byte_keeping_length() {
        let mut p = panel_with(100, 80, 10);
        p.editable = true;
        p.zone = Zone::Hex;
        p.cursor = 5;
        p.edit_hex_nibble(0xa); // high nibble
        p.edit_hex_nibble(0x3); // low nibble → 0xa3, advance
        assert_eq!(p.edits.get(&5), Some(&0xa3));
        assert_eq!(p.cursor, 6);
        assert_eq!(p.len, 100, "overwrite never changes length");
    }

    #[test]
    fn ascii_editing_sets_byte_and_advances() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let mut p = panel_with(100, 80, 10);
        p.editable = true;
        p.zone = Zone::Ascii;
        p.cursor = 2;
        assert!(p.try_edit(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE)));
        assert_eq!(p.edits.get(&2), Some(&b'Z'));
        assert_eq!(p.cursor, 3);
    }

    #[test]
    fn save_writes_edits_in_place_and_backs_up() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let path = std::env::temp_dir().join(format!("termide_hex_{}.bin", std::process::id()));
        std::fs::write(&path, b"ABCDE").unwrap();

        let mut p = BinaryPanel::new_editable(path.clone()).unwrap();
        p.zone = Zone::Ascii;
        p.cursor = 1;
        p.try_edit(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE)); // B -> X
        assert!(p.is_modified());
        p.save().unwrap();
        assert!(!p.is_modified());

        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"AXCDE",
            "edit written, length kept"
        );
        let mut bak = path.clone().into_os_string();
        bak.push(".bak");
        let bak = std::path::PathBuf::from(bak);
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            b"ABCDE",
            "backup holds the original"
        );

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&bak).ok();
    }
}
