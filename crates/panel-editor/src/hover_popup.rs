//! Hover popup for LSP hover information.
//!
//! Displays documentation and type information at cursor position
//! when user Ctrl+clicks on a symbol. Markdown hovers are laid out by the
//! shared Markdown renderer, so code fences, rules and inline markup show as
//! what they mean rather than as their syntax.

use std::cell::RefCell;

use lsp_types::{Hover, HoverContents, MarkedString, MarkupKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};
use termide_theme::Theme;
use unicode_width::UnicodeWidthStr;

/// Maximum width of the hover popup.
const MAX_POPUP_WIDTH: u16 = 80;

/// Maximum height of the hover popup.
const MAX_POPUP_HEIGHT: u16 = 15;

/// Minimum width of the hover popup.
const MIN_POPUP_WIDTH: u16 = 20;

/// Tab stop used when expanding tabs in hover text.
const HOVER_TAB_WIDTH: usize = 4;

/// The hover text to lay out, tabs already expanded.
enum Body {
    /// Markdown, as LSP marks a `MarkedString` and most `MarkupContent`.
    Markdown(String),
    /// `MarkupKind::PlainText`: shown as is, wrapped at spaces.
    Plain(String),
}

/// Lines laid out for one width and theme.
struct Layout {
    key: (u16, &'static str, Color, Color),
    lines: Vec<Line<'static>>,
}

/// Hover popup state and rendering.
pub struct HoverPopup {
    body: Body,
    /// The content as the server sent it, kept for copying: pasting a
    /// signature or an error message into a search engine should not carry
    /// the popup's line breaks or decoration.
    raw_text: String,
    /// Scroll offset for long content.
    scroll_offset: usize,
    /// The last layout, redone only when the width or the theme changes.
    layout: RefCell<Option<Layout>>,
}

impl HoverPopup {
    /// Create a new hover popup from LSP hover response.
    ///
    /// Returns None if hover contents are empty.
    pub fn from_hover(hover: Hover) -> Option<Self> {
        let raw_text = Self::extract_raw_text(&hover.contents)?;
        if raw_text.trim().is_empty() {
            return None;
        }
        let source: String = raw_text
            .lines()
            .map(Self::expand_tabs)
            .collect::<Vec<_>>()
            .join("\n");
        let body = match &hover.contents {
            HoverContents::Markup(m) if m.kind == MarkupKind::PlainText => Body::Plain(source),
            _ => Body::Markdown(source),
        };
        Some(Self {
            body,
            raw_text,
            scroll_offset: 0,
            layout: RefCell::new(None),
        })
    }

    /// The hover text as the server sent it, unwrapped.
    pub fn text(&self) -> &str {
        &self.raw_text
    }

    /// Flatten hover contents into one text, before any layout.
    fn extract_raw_text(contents: &HoverContents) -> Option<String> {
        match contents {
            HoverContents::Scalar(marked) => Self::marked_string_to_text(marked),
            HoverContents::Markup(markup) => Some(markup.value.clone()),
            HoverContents::Array(arr) => {
                let texts: Vec<String> =
                    arr.iter().filter_map(Self::marked_string_to_text).collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n\n"))
                }
            }
        }
    }

    /// Convert MarkedString to Markdown text.
    fn marked_string_to_text(marked: &MarkedString) -> Option<String> {
        match marked {
            MarkedString::String(s) => {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.clone())
                }
            }
            MarkedString::LanguageString(ls) => {
                if ls.value.trim().is_empty() {
                    None
                } else {
                    Some(format!("```{}\n{}\n```", ls.language, ls.value))
                }
            }
        }
    }

    /// Replace tabs with spaces up to the next tab stop and drop other control
    /// characters. Servers such as gopls send struct bodies indented with tabs;
    /// a tab written into a buffer cell reaches the terminal as is, moves the
    /// cursor without painting, and leaves holes in the popup and its line
    /// running past the right edge.
    fn expand_tabs(line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut col = 0usize;
        for ch in line.chars() {
            if ch == '\t' {
                let n = HOVER_TAB_WIDTH - col % HOVER_TAB_WIDTH;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            } else if ch.is_control() {
                continue;
            } else {
                out.push(ch);
                col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            }
        }
        out
    }

    /// Wrap text to fit within max_width, breaking at word boundaries.
    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        let mut result = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                result.push(String::new());
                continue;
            }

            let line_width = trimmed.width();
            if line_width <= max_width {
                result.push(trimmed.to_string());
                continue;
            }

            // Preserve leading whitespace for indented lines (like code)
            let leading_spaces: String = line.chars().take_while(|c| *c == ' ').collect();
            let leading_width = leading_spaces.len();
            let content = trimmed.trim_start();

            // Word wrap
            let mut current_line = leading_spaces.clone();
            let mut current_width = leading_width;

            for word in content.split_whitespace() {
                let word_width = word.width();

                if current_width == leading_width {
                    // First word on line
                    current_line.push_str(word);
                    current_width += word_width;
                } else if current_width + 1 + word_width <= max_width {
                    // Word fits on current line
                    current_line.push(' ');
                    current_line.push_str(word);
                    current_width += 1 + word_width;
                } else {
                    // Start new line with same indentation
                    result.push(current_line);
                    current_line = leading_spaces.clone();
                    current_line.push_str(word);
                    current_width = leading_width + word_width;
                }
            }

            if !current_line.trim().is_empty() {
                result.push(current_line);
            }
        }

        result
    }

    /// Lay the body out for `width` columns of text.
    fn lay_out(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = match &self.body {
            Body::Markdown(src) => {
                let colors = termide_core::ThemeColors::from(theme);
                termide_panel_markdown::render_markdown(src, width, &colors, theme.is_light_theme())
                    .lines
            }
            Body::Plain(src) => Self::wrap_text(src, width as usize)
                .into_iter()
                .map(Line::raw)
                .collect(),
        };
        // A hover that opens with a code block gets the blank the renderer
        // puts before every block; the popup needs no gap at its top.
        let lead = lines.iter().take_while(|l| l.width() == 0).count();
        lines.drain(..lead);
        while lines.last().is_some_and(|l| l.width() == 0) {
            lines.pop();
        }
        lines
    }

    /// Run `f` over the lines laid out for `width` and `theme`, laying them
    /// out again only if either changed since the last call.
    fn with_lines<R>(&self, width: u16, theme: &Theme, f: impl FnOnce(&[Line<'static>]) -> R) -> R {
        let key = (width, theme.name, theme.fg, theme.accented_bg);
        let mut cache = self.layout.borrow_mut();
        if cache.as_ref().is_none_or(|l| l.key != key) {
            *cache = Some(Layout {
                key,
                lines: self.lay_out(width, theme),
            });
        }
        f(&cache.as_ref().expect("laid out above").lines)
    }

    /// A horizontal rule spans whatever width it is given; it should not by
    /// itself make the popup as wide as it can be.
    fn is_rule(line: &Line) -> bool {
        let mut chars = line.spans.iter().flat_map(|s| s.content.chars());
        chars.next() == Some('─') && chars.all(|c| c == '─')
    }

    /// Number of laid-out lines, from the last layout.
    fn line_count(&self) -> usize {
        self.layout.borrow().as_ref().map_or(0, |l| l.lines.len())
    }

    /// Scroll up by given amount.
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll down by given amount.
    pub fn scroll_down(&mut self, amount: usize) {
        let max_offset = self.line_count().saturating_sub(MAX_POPUP_HEIGHT as usize);
        self.scroll_offset = (self.scroll_offset + amount).min(max_offset);
    }

    /// Render the hover popup at the given position.
    ///
    /// Returns the popup rect for mouse hit testing, or None if nothing was rendered.
    pub fn render(
        &self,
        buf: &mut Buffer,
        area: Rect,
        click_x: u16,
        click_y: u16,
        theme: &Theme,
    ) -> Option<Rect> {
        // Calculate available space in the area (with margin for borders)
        let margin = 1u16;
        let available_width = area.width.saturating_sub(margin * 2);
        let available_height = area.height.saturating_sub(margin * 2);

        if available_width < MIN_POPUP_WIDTH || available_height < 2 {
            return None; // Not enough space
        }

        // One column of padding on each side.
        let text_width = MAX_POPUP_WIDTH.min(available_width) - 2;
        self.with_lines(text_width, theme, |lines| {
            if lines.is_empty() {
                return None;
            }
            let content_width = lines
                .iter()
                .filter(|l| !Self::is_rule(l))
                .map(Line::width)
                .max()
                .unwrap_or(0);

            let popup_width = (content_width as u16 + 2)
                .clamp(MIN_POPUP_WIDTH, MAX_POPUP_WIDTH)
                .min(available_width);

            let visible_lines = lines.len().min(MAX_POPUP_HEIGHT as usize);
            let popup_height = (visible_lines as u16).min(available_height);

            // Calculate popup position (stay within area bounds)
            let (popup_x, popup_y) =
                self.calculate_position(area, click_x, click_y, popup_width, popup_height);

            // Ensure popup stays within area
            let final_x = popup_x
                .max(area.x + margin)
                .min(area.right().saturating_sub(popup_width + margin));
            let final_y = popup_y
                .max(area.y + margin)
                .min(area.bottom().saturating_sub(popup_height + margin));

            let popup_rect = Rect::new(final_x, final_y, popup_width, popup_height);
            let drawn = popup_rect.intersection(buf.area);

            // Background
            let bg_style = Style::default().bg(theme.accented_bg).fg(theme.fg);
            for y in drawn.top()..drawn.bottom() {
                for x in drawn.left()..drawn.right() {
                    buf[(x, y)].reset();
                    buf[(x, y)].set_style(bg_style);
                }
            }

            // Content: the lines' own styles go over the popup's background,
            // and anything wider than the popup is cut at its padding.
            let x_start = popup_rect.left() + 1;
            let text_room = popup_rect.width.saturating_sub(2);
            for (display_idx, line) in lines
                .iter()
                .skip(self.scroll_offset)
                .take(popup_height as usize)
                .enumerate()
            {
                let y = popup_rect.top() + display_idx as u16;
                if y >= drawn.bottom() || x_start >= drawn.right() {
                    continue;
                }
                let room = text_room.min(drawn.right() - x_start);
                buf.set_line(x_start, y, line, room);
            }

            // Scroll indicators
            let x = popup_rect.right().saturating_sub(1);
            if x < drawn.right() {
                if self.scroll_offset > 0 && popup_rect.top() < drawn.bottom() {
                    buf[(x, popup_rect.top())].set_char('▲').set_style(bg_style);
                }
                let bottom = popup_rect.bottom().saturating_sub(1);
                if self.scroll_offset + (popup_height as usize) < lines.len()
                    && bottom < drawn.bottom()
                {
                    buf[(x, bottom)].set_char('▼').set_style(bg_style);
                }
            }

            Some(popup_rect)
        })
    }

    /// Calculate popup position ensuring it stays within area bounds.
    fn calculate_position(
        &self,
        area: Rect,
        click_x: u16,
        click_y: u16,
        width: u16,
        height: u16,
    ) -> (u16, u16) {
        // Leave 1 char margin from edges to avoid overlapping borders
        let margin = 1u16;
        let safe_right = area.right().saturating_sub(margin);
        let safe_bottom = area.bottom().saturating_sub(margin);

        // Try to position below and to the right of click
        let mut x = click_x;
        let mut y = click_y + 1;

        // Adjust x if popup would go off right edge (with margin)
        if x + width > safe_right {
            x = safe_right.saturating_sub(width);
        }

        // Flip above click if not enough space below
        if y + height > safe_bottom {
            if click_y >= height + margin {
                y = click_y.saturating_sub(height);
            } else {
                y = safe_bottom.saturating_sub(height);
            }
        }

        // Ensure within bounds
        let final_x = x.max(area.left() + margin);
        let final_y = y.max(area.top() + margin);

        (final_x, final_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::MarkupContent;

    pub(super) fn hover_with(text: &str) -> Hover {
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: text.to_string(),
            }),
            range: None,
        }
    }

    /// Render into a wide buffer; returns the buffer and the popup's rect.
    pub(super) fn draw(popup: &HoverPopup) -> (Buffer, Rect) {
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let rect = popup
            .render(&mut buf, area, 5, 2, &Theme::default())
            .expect("rendered");
        (buf, rect)
    }

    /// The popup's rows as text, padding trimmed.
    pub(super) fn rows(buf: &Buffer, rect: Rect) -> Vec<String> {
        (rect.top()..rect.bottom())
            .map(|y| {
                (rect.left()..rect.right())
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn test_from_hover_markup() {
        let popup = HoverPopup::from_hover(hover_with("# Function\n\nThis is a test function."))
            .expect("popup");
        let (buf, rect) = draw(&popup);
        let rows = rows(&buf, rect);
        assert!(rows.iter().any(|r| r.contains("Function")));
        assert!(rows.iter().any(|r| r == "This is a test function."));
    }

    #[test]
    fn test_from_hover_empty() {
        assert!(HoverPopup::from_hover(hover_with("   ")).is_none());
    }

    #[test]
    fn test_scroll() {
        let text = (0..20)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut popup = HoverPopup::from_hover(hover_with(&text)).unwrap();
        draw(&popup);
        assert_eq!(popup.scroll_offset, 0);

        popup.scroll_down(5);
        assert_eq!(popup.scroll_offset, 5);

        popup.scroll_up(3);
        assert_eq!(popup.scroll_offset, 2);

        popup.scroll_up(10);
        assert_eq!(popup.scroll_offset, 0);
    }

    /// gopls answers with a fenced block, a rule and another block. The
    /// fences and the rule are syntax: none of it may show as typed.
    #[test]
    fn fences_and_rules_do_not_show_as_syntax() {
        let text = "```go\ntype T struct {\n\troot string\n}\n```\n\n---\n\n```go\nfunc (t *T) Blur()\n```";
        let popup = HoverPopup::from_hover(hover_with(text)).expect("popup");
        let (buf, rect) = draw(&popup);
        let rows = rows(&buf, rect);
        assert!(rows.iter().all(|r| !r.contains("```")), "{rows:#?}");
        assert!(rows.iter().all(|r| r != "---"), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("type T struct {")));
        assert!(rows.iter().any(|r| r.contains("func (t *T) Blur()")));
        assert!(!rows[0].is_empty(), "no blank row at the top: {rows:#?}");
        // The rule does not stretch a narrow popup to the widest it can be.
        assert!(rect.width < MAX_POPUP_WIDTH, "width {}", rect.width);
    }

    #[test]
    fn plain_text_is_not_read_as_markdown() {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::PlainText,
                value: "a *b* `c`".to_string(),
            }),
            range: None,
        };
        let popup = HoverPopup::from_hover(hover).expect("popup");
        let (buf, rect) = draw(&popup);
        assert_eq!(rows(&buf, rect), vec!["a *b* `c`".to_string()]);
    }
}

#[cfg(test)]
mod copy_tests {
    use super::tests::{draw, hover_with, rows};
    use super::*;

    /// The popup wraps its text to fit the window; copying must hand back what
    /// the server sent, or a pasted signature arrives full of line breaks that
    /// were never in it.
    #[test]
    fn copied_text_is_not_the_wrapped_text() {
        let long = "fn example<T: Iterator<Item = String>>(argument: T, another: usize) -> Result<Vec<String>, std::io::Error>";
        let popup = HoverPopup::from_hover(hover_with(long)).expect("popup");

        assert_eq!(popup.text(), long);
        assert!(
            popup.with_lines(78, &Theme::default(), |l| l.len()) > 1,
            "precondition: this text is long enough to wrap"
        );
        assert!(!popup.text().contains('\n'));
    }

    /// gopls indents struct fields with tabs. A tab in a buffer cell reaches the
    /// terminal raw: the cursor jumps without painting, leaving holes in the
    /// popup and pushing the line past its right edge.
    #[test]
    fn tabs_are_expanded_and_never_reach_the_buffer() {
        let text =
            "```go\ntype TreeModel struct {\n\troot\t\tstring\n\tfocused\tbool // флаг\n}\n```";
        let popup = HoverPopup::from_hover(hover_with(text)).expect("popup");
        let (buf, rect) = draw(&popup);
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                assert!(
                    !buf[(x, y)].symbol().chars().any(char::is_control),
                    "control character at ({x},{y})"
                );
            }
        }
        let rows = rows(&buf, rect);
        assert!(
            rows.iter().any(|r| r.ends_with("root        string")),
            "{rows:#?}"
        );
        // Copying still hands back the server's text untouched.
        assert_eq!(popup.text(), text);
    }

    #[test]
    fn multiple_sections_are_joined_for_copying() {
        let hover = Hover {
            contents: HoverContents::Array(vec![
                MarkedString::String("first".to_string()),
                MarkedString::String("second".to_string()),
            ]),
            range: None,
        };
        let popup = HoverPopup::from_hover(hover).expect("popup");
        assert_eq!(popup.text(), "first\n\nsecond");
    }
}
