//! Hex-dump and lossy-text row rendering with cursor/selection/match styling.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{BinaryPanel, Zone};

impl BinaryPanel {
    /// Style for a byte cell, in the hex or ASCII representation.
    fn cell_style(&self, gi: u64, byte: u8, repr: Zone) -> Style {
        let mut st = Style::default().fg(if byte == 0 {
            self.theme.disabled
        } else {
            self.theme.fg
        });
        // Search-match highlight (same colours as the editor: current match in
        // the accent colour, other matches in the warning colour).
        match self.match_at(gi) {
            Some(true) => {
                st = st
                    .bg(self.theme.info)
                    .fg(self.theme.bg)
                    .add_modifier(Modifier::BOLD)
            }
            Some(false) => st = st.bg(self.theme.warning).fg(self.theme.bg),
            None => {}
        }
        let (s, e) = self.sel_range();
        let selected = self.anchor.is_some() && gi >= s && gi <= e;
        if selected {
            st = st.fg(self.theme.selection_fg).bg(self.theme.selection_bg);
        }
        // The cursor byte is shown inverse in both zones (consistent with the
        // editor's cursor); the active zone also gets BOLD so it's clear which
        // zone Tab will edit/navigate. Hidden when the panel isn't focused.
        if gi == self.cursor && self.focused {
            st = st.add_modifier(Modifier::REVERSED);
            if repr == self.zone {
                st = st.add_modifier(Modifier::BOLD);
            }
        }
        st
    }

    /// Build one hex-dump row (`offset │ hex │ ASCII`) over `cols` columns.
    pub(crate) fn hex_row<'a>(&self, off: u64, bytes: &[u8], cols: u64) -> Line<'a> {
        let dim = Style::default().fg(self.theme.disabled);
        let off_style = Style::default().fg(self.theme.line_numbers);

        let mut spans: Vec<Span<'a>> = Vec::with_capacity(cols as usize * 2 + 4);
        spans.push(Span::styled(format!("{off:08X}"), off_style));
        spans.push(Span::styled("  ", dim));

        for i in 0..cols as usize {
            // Separator after the byte: a dim `│` every 8 bytes for orientation
            // (width-neutral — it replaces the inter-byte space), else a space.
            let last = i + 1 == cols as usize;
            let sep = if i % 8 == 7 && !last { "│" } else { " " };
            match bytes.get(i) {
                Some(&b) => {
                    let gi = off + i as u64;
                    spans.push(Span::styled(
                        format!("{b:02x}"),
                        self.cell_style(gi, b, Zone::Hex),
                    ));
                    spans.push(Span::styled(sep, dim));
                }
                None => {
                    spans.push(Span::styled("  ", dim));
                    spans.push(Span::styled(sep, dim));
                }
            }
        }

        spans.push(Span::styled(" ", dim));
        for (i, &b) in bytes.iter().enumerate() {
            let gi = off + i as u64;
            let ch = if (0x20..=0x7e).contains(&b) {
                (b as char).to_string()
            } else {
                "·".to_string()
            };
            spans.push(Span::styled(ch, self.cell_style(gi, b, Zone::Ascii)));
        }

        Line::from(spans)
    }

    /// Build one lossy plain-text row (non-printable bytes shown as `·`), one
    /// char per byte, with the same cursor/selection/match highlighting.
    pub(crate) fn text_row<'a>(&self, off: u64, bytes: &[u8]) -> Line<'a> {
        let mut spans: Vec<Span<'a>> = Vec::with_capacity(bytes.len());
        for (i, &b) in bytes.iter().enumerate() {
            let gi = off + i as u64;
            let ch = if (0x20..=0x7e).contains(&b) {
                (b as char).to_string()
            } else {
                "·".to_string()
            };
            spans.push(Span::styled(ch, self.cell_style(gi, b, Zone::Ascii)));
        }
        Line::from(spans)
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
    fn hex_row_formats_offset_bytes_and_ascii() {
        let p = panel_with(0, 80, 10);
        let line = p.hex_row(0x10, b"Hi\x00\xff", 16);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("00000010"), "offset: {text:?}");
        assert!(text.contains("48 69 00 ff"), "hex: {text:?}");
        assert!(text.trim_end().ends_with("Hi··"), "ascii: {text:?}");
    }
}
