//! The keys that may follow a pending Vim prefix (`g`, `z`, `Ctrl+W`, an
//! operator), shown in a box at the bottom right of the editor while the
//! prefix waits — a which-key hint.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use termide_theme::Theme;
use unicode_width::UnicodeWidthStr;

use super::mode::VimMode;
use super::operators::VimOperator;
use super::state::VimState;

/// What a pending prefix may be followed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimHint {
    /// The keys typed so far: `g`, `2d`, `Ctrl+W`.
    pub title: String,
    /// Keys and what they do.
    pub rows: Vec<(String, String)>,
}

impl VimHint {
    /// The hint for what `state` waits on; `None` when it waits on nothing.
    pub fn for_state(state: &VimState) -> Option<Self> {
        if state.mode != VimMode::Normal {
            return None;
        }
        let t = termide_i18n::t();
        let count = state.count.map(|n| n.to_string()).unwrap_or_default();
        let row = |keys: &str, what: &str| (keys.to_string(), what.to_string());
        let (prefix, rows) = match state.partial_keys.first() {
            Some('g') => (
                "g".to_string(),
                vec![
                    row("g", t.vim_hint_first_line()),
                    row("j k", t.vim_hint_screen_lines()),
                ],
            ),
            Some('z') => (
                "z".to_string(),
                vec![
                    row("a", t.vim_hint_toggle_fold()),
                    row("c", t.help_desc_fold()),
                    row("o", t.help_desc_unfold()),
                    row("M", t.help_desc_fold_all()),
                    row("R", t.help_desc_unfold_all()),
                ],
            ),
            Some('\x17') => (
                "Ctrl+W".to_string(),
                vec![row("h j k l", t.vim_hint_panels())],
            ),
            Some(_) => return None,
            None => {
                let letter = match state.pending_operator? {
                    VimOperator::Delete => "d",
                    VimOperator::Yank => "y",
                    VimOperator::Change => "c",
                };
                (
                    letter.to_string(),
                    vec![
                        row(letter, t.vim_hint_whole_line()),
                        row("w b e", t.vim_hint_words()),
                        row("^ $", t.vim_hint_line_ends()),
                        row("h l", t.vim_hint_chars()),
                        row("j k", t.vim_hint_lines()),
                    ],
                )
            }
        };
        Some(Self {
            title: format!("{count}{prefix}"),
            rows,
        })
    }

    /// Draw the hint in the bottom right corner of `area`, as much of it as
    /// fits. Returns the rect drawn, `None` when the area is too small.
    pub fn render(&self, buf: &mut Buffer, area: Rect, theme: &Theme) -> Option<Rect> {
        let keys_width = self.rows.iter().map(|(k, _)| k.width()).max()?;
        let what_width = self.rows.iter().map(|(_, w)| w.width()).max()?;
        let title_width = self.title.width() + 2;
        // Borders, a space either side, two between the columns.
        let inner = (keys_width + 2 + what_width).max(title_width);
        let width = (inner as u16 + 4).min(area.width);
        let height = (self.rows.len() as u16 + 2).min(area.height);
        if width < 8 || height < 3 {
            return None;
        }
        let rect = Rect::new(area.right() - width, area.bottom() - height, width, height);
        let style = Style::default().bg(theme.accented_bg).fg(theme.fg);
        Clear.render(rect, buf);
        Block::default()
            .borders(Borders::ALL)
            .border_style(style.fg(theme.disabled))
            .title(format!(" {} ", self.title))
            .title_style(style.add_modifier(Modifier::BOLD))
            .style(style)
            .render(rect, buf);
        let key_style = style.fg(theme.accented_fg).add_modifier(Modifier::BOLD);
        let right = rect.right().saturating_sub(2);
        for (i, (keys, what)) in self.rows.iter().enumerate() {
            let y = rect.y + 1 + i as u16;
            if y >= rect.bottom() - 1 {
                break;
            }
            let x = rect.x + 2;
            let (end, _) = buf.set_stringn(x, y, keys, right.saturating_sub(x) as usize, key_style);
            let what_x = (x + keys_width as u16 + 2).max(end);
            if what_x < right {
                buf.set_stringn(what_x, y, what, (right - what_x) as usize, style);
            }
        }
        Some(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(hint: &VimHint) -> Vec<&str> {
        hint.rows.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn nothing_pending_shows_nothing() {
        assert_eq!(VimHint::for_state(&VimState::new()), None);
        let mut state = VimState::new();
        state.count = Some(3);
        assert_eq!(VimHint::for_state(&state), None, "a count alone");
    }

    #[test]
    fn each_prefix_lists_its_continuations() {
        let mut state = VimState::new();
        state.push_partial_key('z');
        let hint = VimHint::for_state(&state).unwrap();
        assert_eq!(hint.title, "z");
        assert_eq!(keys(&hint), ["a", "c", "o", "M", "R"]);

        let mut state = VimState::new();
        state.push_partial_key('\x17');
        assert_eq!(VimHint::for_state(&state).unwrap().title, "Ctrl+W");

        let mut state = VimState::new();
        state.count = Some(2);
        state.set_pending_operator(VimOperator::Delete);
        let hint = VimHint::for_state(&state).unwrap();
        assert_eq!(hint.title, "2d");
        assert_eq!(keys(&hint)[0], "d", "dd takes the whole line");
    }

    #[test]
    fn insert_mode_shows_nothing() {
        let mut state = VimState::new();
        state.push_partial_key('g');
        state.mode = VimMode::Insert;
        assert_eq!(VimHint::for_state(&state), None);
    }

    #[test]
    fn the_box_sits_in_the_bottom_right_corner() {
        let mut state = VimState::new();
        state.push_partial_key('g');
        let hint = VimHint::for_state(&state).unwrap();
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        let rect = hint.render(&mut buf, area, &Theme::default()).unwrap();
        assert_eq!((rect.right(), rect.bottom()), (60, 12));
        assert_eq!(rect.height, 4);
        let row: String = (rect.x..rect.right())
            .map(|x| buf[(x, rect.y + 1)].symbol())
            .collect();
        assert!(row.contains("g  "), "{row:?}");
        // Too small an area draws nothing.
        let tiny = Rect::new(0, 0, 6, 2);
        assert_eq!(
            hint.render(&mut Buffer::empty(tiny), tiny, &Theme::default()),
            None
        );
    }
}
