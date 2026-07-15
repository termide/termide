//! Inline find bar: ASCII / hex byte-sequence search and match navigation.

use termide_core::PanelEvent;
use termide_modal::{FindBar, FindBarAction, FindBarBtn, FindBarConfig, FindField};

use crate::BinaryPanel;

/// Search scans at most this many bytes (from the start); larger files report
/// "file too large to search" rather than scanning unbounded.
const MAX_SEARCH_BYTES: u64 = 64 << 20;

/// Cap on collected match offsets.
const MAX_MATCHES: usize = 10_000;

/// Parse a hex query (`"ff fe"` / `"fffe"`) into bytes; `None` if it has odd
/// digits or a non-hex character.
fn parse_hex(q: &str) -> Option<Vec<u8>> {
    let digits: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok())
        .collect()
}

/// All start offsets where `needle` occurs in `hay` (capped at [`MAX_MATCHES`]).
/// `ci` does ASCII-case-insensitive matching.
fn find_all(hay: &[u8], needle: &[u8], ci: bool) -> Vec<u64> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let eq = |a: &u8, b: &u8| {
        if ci {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    for i in 0..=hay.len() - needle.len() {
        if hay[i..i + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| eq(a, b))
        {
            out.push(i as u64);
            if out.len() >= MAX_MATCHES {
                break;
            }
        }
    }
    out
}

impl BinaryPanel {
    /// Whether byte `gi` falls inside any search match.
    pub(crate) fn match_at(&self, gi: u64) -> Option<bool> {
        if self.match_len == 0 || self.matches.is_empty() {
            return None;
        }
        let mlen = self.match_len as u64;
        // Last match whose start is <= gi.
        let count = self.matches.partition_point(|&m| m <= gi);
        if count == 0 {
            return None;
        }
        let start = self.matches[count - 1];
        if gi < start + mlen {
            Some(count - 1 == self.match_idx)
        } else {
            None
        }
    }

    /// Open the inline find bar (ASCII substring or hex byte sequence).
    pub(crate) fn open_find(&mut self) {
        let mut bar = FindBar::new(FindBarConfig {
            fields: vec![FindField::Find],
            // Prev/Next navigation, ASCII case toggle, and the hex-mode toggle.
            buttons: vec![
                FindBarBtn::Prev,
                FindBarBtn::Next,
                FindBarBtn::Case,
                FindBarBtn::Hex,
            ],
        });
        bar.focus_first();
        self.find_bar = Some(bar);
        self.matches.clear();
        self.match_idx = 0;
    }

    pub(crate) fn close_find(&mut self) {
        self.find_bar = None;
        self.matches.clear();
    }

    /// Parse the query into a search needle: hex bytes when the `[hex]` toggle
    /// is on, otherwise the raw ASCII bytes. Returns `(needle, case_insensitive)`.
    fn needle(&self) -> Option<(Vec<u8>, bool)> {
        let bar = self.find_bar.as_ref()?;
        let q = bar.find_text();
        if q.is_empty() {
            return None;
        }
        if bar.hex_mode() {
            parse_hex(q).map(|n| (n, false))
        } else {
            Some((q.as_bytes().to_vec(), !bar.case_sensitive()))
        }
    }

    /// Re-run the search and jump to the first match at/after the cursor.
    fn run_search(&mut self) {
        let parsed = self.needle();
        let too_large = self.len > MAX_SEARCH_BYTES;

        let Some((needle, ci)) = parsed else {
            self.matches.clear();
            if let Some(bar) = self.find_bar.as_mut() {
                bar.clear_match_info();
                let hex_bad = bar.hex_mode() && !bar.find_text().is_empty();
                bar.set_info_text(hex_bad.then(|| "invalid hex".to_string()));
            }
            return;
        };

        if too_large {
            self.matches.clear();
            if let Some(bar) = self.find_bar.as_mut() {
                bar.clear_match_info();
                bar.set_info_text(Some("file too large to search".to_string()));
            }
            return;
        }

        let cap = self.len.min(MAX_SEARCH_BYTES) as usize;
        let hay = self.read_window(0, cap);
        self.matches = find_all(&hay, &needle, ci);
        self.match_len = needle.len();
        self.match_idx = 0;

        if let Some(bar) = self.find_bar.as_mut() {
            bar.set_info_text(None);
            if self.matches.is_empty() {
                bar.set_match_info(0, 0);
            } else {
                bar.set_match_info(1, self.matches.len());
            }
        }
        if let Some(&off) = self.matches.first() {
            self.set_cursor(off, false);
        }
    }

    /// Step to the next/previous match and move the cursor there.
    fn step_match(&mut self, forward: bool) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len();
        self.match_idx = if forward {
            (self.match_idx + 1) % n
        } else {
            (self.match_idx + n - 1) % n
        };
        let off = self.matches[self.match_idx];
        if let Some(bar) = self.find_bar.as_mut() {
            bar.set_match_info(self.match_idx + 1, n);
        }
        self.set_cursor(off, false);
    }

    pub(crate) fn handle_find_action(&mut self, action: FindBarAction) -> Vec<PanelEvent> {
        match action {
            FindBarAction::QueryChanged | FindBarAction::Refresh => self.run_search(),
            FindBarAction::Next | FindBarAction::Submit => self.step_match(true),
            FindBarAction::Previous => self.step_match(false),
            FindBarAction::Close => self.close_find(),
            _ => {}
        }
        vec![PanelEvent::NeedsRedraw]
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
    fn parse_hex_accepts_pairs_rejects_garbage() {
        assert_eq!(parse_hex("ff fe"), Some(vec![0xff, 0xfe]));
        assert_eq!(parse_hex("FFFE"), Some(vec![0xff, 0xfe]));
        assert_eq!(parse_hex("f"), None); // odd
        assert_eq!(parse_hex("zz"), None); // non-hex
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn find_all_locates_matches() {
        let hay = b"abXYabXY";
        assert_eq!(find_all(hay, b"XY", false), vec![2, 6]);
        // case-insensitive ASCII
        assert_eq!(find_all(b"AbcaBC", b"abc", true), vec![0, 3]);
        // hex bytes (exact)
        assert_eq!(
            find_all(&[0x00, 0xff, 0x00, 0xff], &[0x00, 0xff], false),
            vec![0, 2]
        );
        assert!(find_all(hay, b"ZZ", false).is_empty());
    }

    #[test]
    fn match_at_detects_current_and_other_matches() {
        let mut p = panel_with(100, 80, 10);
        p.matches = vec![10, 50];
        p.match_len = 3;
        p.match_idx = 1; // current = the match starting at 50
        assert_eq!(p.match_at(11), Some(false)); // inside first match, not current
        assert_eq!(p.match_at(50), Some(true)); // current match
        assert_eq!(p.match_at(52), Some(true));
        assert_eq!(p.match_at(53), None); // past it (len 3: 50,51,52)
        assert_eq!(p.match_at(0), None);
    }
}
