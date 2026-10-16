//! Hover popup for LSP hover information.
//!
//! Displays documentation and type information at cursor position
//! when user Ctrl+clicks on a symbol.

use lsp_types::{Hover, HoverContents, MarkedString};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
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

/// Hover popup state and rendering.
pub struct HoverPopup {
    /// Parsed hover content lines.
    lines: Vec<String>,
    /// The same content before word wrapping, kept for copying: pasting a
    /// signature or an error message into a search engine should not carry
    /// the popup's line breaks.
    raw_text: String,
    /// Scroll offset for long content.
    scroll_offset: usize,
}

impl HoverPopup {
    /// Create a new hover popup from LSP hover response.
    ///
    /// Returns None if hover contents are empty.
    pub fn from_hover(hover: Hover) -> Option<Self> {
        let raw_text = Self::extract_raw_text(&hover.contents)?;
        let lines = Self::extract_lines(&hover.contents)?;
        if lines.is_empty() {
            return None;
        }
        Some(Self {
            lines,
            raw_text,
            scroll_offset: 0,
        })
    }

    /// The hover text as the server sent it, unwrapped.
    pub fn text(&self) -> &str {
        &self.raw_text
    }

    /// Extract lines from hover contents with word wrapping.
    /// Flatten hover contents into plain text, before any wrapping.
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

    fn extract_lines(contents: &HoverContents) -> Option<Vec<String>> {
        let raw_text = Self::extract_raw_text(contents)?;

        // Apply word wrap to fit within popup width (minus padding)
        let wrap_width = (MAX_POPUP_WIDTH - 2) as usize;
        let wrapped = Self::wrap_text(&raw_text, wrap_width);

        // Clean up each line
        let lines: Vec<String> = wrapped
            .into_iter()
            .map(|line| Self::clean_markdown_line(&line))
            .collect();

        if lines.is_empty() || lines.iter().all(|l| l.trim().is_empty()) {
            None
        } else {
            Some(lines)
        }
    }

    /// Convert MarkedString to plain text.
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
                    // Prefix with language for syntax context
                    Some(format!("```{}\n{}\n```", ls.language, ls.value))
                }
            }
        }
    }

    /// Basic markdown line cleanup (remove excessive formatting).
    fn clean_markdown_line(line: &str) -> String {
        // Keep the line mostly as-is, but trim trailing whitespace
        line.trim_end().to_string()
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
            let line = Self::expand_tabs(line);
            let line = line.as_str();
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

    /// Check if popup is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Scroll up by given amount.
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll down by given amount.
    pub fn scroll_down(&mut self, amount: usize) {
        let max_offset = self.lines.len().saturating_sub(MAX_POPUP_HEIGHT as usize);
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
        if self.lines.is_empty() {
            return None;
        }

        // Calculate available space in the area (with margin for borders)
        let margin = 1u16;
        let available_width = area.width.saturating_sub(margin * 2);
        let available_height = area.height.saturating_sub(margin * 2);

        if available_width < MIN_POPUP_WIDTH || available_height < 2 {
            return None; // Not enough space
        }

        // Calculate popup dimensions constrained to available space
        let content_width = self
            .lines
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(MIN_POPUP_WIDTH as usize);

        let popup_width = (content_width as u16 + 2)
            .clamp(MIN_POPUP_WIDTH, MAX_POPUP_WIDTH)
            .min(available_width);

        let visible_lines = self.lines.len().min(MAX_POPUP_HEIGHT as usize);
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

        // Render background
        let bg_style = Style::default().bg(theme.accented_bg).fg(theme.fg);
        for y in popup_rect.top()..popup_rect.bottom() {
            for x in popup_rect.left()..popup_rect.right() {
                if x >= buf.area.left()
                    && x < buf.area.right()
                    && y >= buf.area.top()
                    && y < buf.area.bottom()
                {
                    buf[(x, y)].set_style(bg_style);
                    buf[(x, y)].set_char(' ');
                }
            }
        }

        // Render content lines
        for (display_idx, line) in self
            .lines
            .iter()
            .skip(self.scroll_offset)
            .take(visible_lines)
            .enumerate()
        {
            let y = popup_rect.top() + display_idx as u16;
            let x_start = popup_rect.left() + 1; // 1 char padding

            // Determine line style (highlight code blocks)
            let line_style = if line.starts_with("```") || line.starts_with("   ") {
                Style::default().bg(theme.accented_bg).fg(theme.accented_fg)
            } else {
                bg_style
            };

            // Lines are pre-wrapped, so just render directly
            // Render characters with proper unicode width handling
            let mut x_offset = 0u16;
            for ch in line.chars() {
                let x = x_start + x_offset;
                if x < popup_rect.right() && x < buf.area.width && y < buf.area.height {
                    buf[(x, y)].set_char(ch);
                    buf[(x, y)].set_style(line_style);
                }
                let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
                x_offset += ch_width;
            }
        }

        // Render scroll indicators
        if self.scroll_offset > 0 {
            let x = popup_rect.right().saturating_sub(1);
            let y = popup_rect.top();
            if x < buf.area.width && y < buf.area.height {
                buf[(x, y)].set_char('▲');
                buf[(x, y)].set_style(bg_style);
            }
        }
        if self.scroll_offset + visible_lines < self.lines.len() {
            let x = popup_rect.right().saturating_sub(1);
            let y = popup_rect.bottom().saturating_sub(1);
            if x < buf.area.width && y < buf.area.height {
                buf[(x, y)].set_char('▼');
                buf[(x, y)].set_style(bg_style);
            }
        }

        Some(popup_rect)
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
    use lsp_types::{MarkupContent, MarkupKind};

    #[test]
    fn test_from_hover_markup() {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "# Function\n\nThis is a test function.".to_string(),
            }),
            range: None,
        };

        let popup = HoverPopup::from_hover(hover);
        assert!(popup.is_some());
        let popup = popup.unwrap();
        assert!(!popup.is_empty());
        assert_eq!(popup.lines.len(), 3);
    }

    #[test]
    fn test_from_hover_empty() {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "   ".to_string(),
            }),
            range: None,
        };

        let popup = HoverPopup::from_hover(hover);
        assert!(popup.is_none());
    }

    #[test]
    fn test_scroll() {
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: (0..20)
                    .map(|i| format!("Line {}", i))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            range: None,
        };

        let mut popup = HoverPopup::from_hover(hover).unwrap();
        assert_eq!(popup.scroll_offset, 0);

        popup.scroll_down(5);
        assert_eq!(popup.scroll_offset, 5);

        popup.scroll_up(3);
        assert_eq!(popup.scroll_offset, 2);

        popup.scroll_up(10);
        assert_eq!(popup.scroll_offset, 0);
    }
}

#[cfg(test)]
mod copy_tests {
    use super::*;
    use lsp_types::{MarkupContent, MarkupKind};

    fn hover_with(text: &str) -> Hover {
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: text.to_string(),
            }),
            range: None,
        }
    }

    /// The popup wraps its text to fit the window; copying must hand back what
    /// the server sent, or a pasted signature arrives full of line breaks that
    /// were never in it.
    #[test]
    fn copied_text_is_not_the_wrapped_text() {
        let long = "fn example<T: Iterator<Item = String>>(argument: T, another: usize) -> Result<Vec<String>, std::io::Error>";
        let popup = HoverPopup::from_hover(hover_with(long)).expect("popup");

        assert_eq!(popup.text(), long);
        assert!(
            popup.lines.len() > 1,
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
        assert!(popup.lines.iter().all(|l| !l.chars().any(char::is_control)));
        assert_eq!(popup.lines[2], "    root        string");

        let area = Rect::new(0, 0, 100, 20);
        let mut buf = Buffer::empty(area);
        let rect = popup
            .render(&mut buf, area, 5, 2, &Theme::default())
            .expect("rendered");
        for y in 0..area.height {
            for x in 0..area.width {
                assert_ne!(buf[(x, y)].symbol(), "\t", "tab at ({x},{y})");
            }
        }
        let widest = popup.lines.iter().map(|l| l.width()).max().unwrap();
        assert!(widest as u16 + 1 < rect.right() - rect.left() + 1);
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
