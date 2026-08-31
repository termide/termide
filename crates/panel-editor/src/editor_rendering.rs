//! Rendering methods for the Editor.
//!
//! Centralizes the per-frame work driven by the Panel trait: viewport
//! sync, wrap-mode selection, delegation to the rendering orchestrator
//! in `crate::rendering`, blame overlay, scrollbar, and LSP popups.

use ratatui::{buffer::Buffer, layout::Rect};

use termide_config::Config;
use termide_theme::Theme;
use termide_ui::ScrollBar;

use crate::{rendering, word_wrap};

use super::Editor;

impl Editor {
    /// Render with custom highlighter (for LogViewer).
    pub fn render_with_highlighter<H: termide_highlight::LineHighlighter>(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        config: &Config,
        is_focused: bool,
        highlighter: &mut H,
    ) {
        // Update viewport size
        let (content_width, content_height) = rendering::calculate_content_dimensions(
            area.width,
            area.height,
            self.buffer.line_count(),
        );

        let effective_width = if self.config.word_wrap {
            content_width
        } else {
            0
        };
        let use_smart_wrap = if self.config.word_wrap && content_width > 0 {
            self.should_use_smart_wrap(config)
        } else {
            false
        };

        // Update wrap settings BEFORE building cumulative cache
        // This ensures cache is invalidated if width changed
        self.render_cache
            .update_wrap_settings(effective_width, use_smart_wrap);
        self.render_cache.content_height = content_height;

        self.viewport.resize(content_width, content_height);

        let virtual_lines_total = self.virtual_line_count(config);
        self.render_cache.virtual_line_count = virtual_lines_total;

        // Ensure cursor is visible (only when viewport follows cursor)
        if self.scroll_follows_cursor {
            if self.config.word_wrap && content_width > 0 {
                self.ensure_cursor_visible_word_wrap(content_height);
            } else {
                self.viewport
                    .ensure_cursor_visible(&self.cursor, virtual_lines_total);
            }
        }

        // Render with custom highlighter
        rendering::render_editor_content(
            buf,
            area,
            &self.buffer,
            &self.viewport,
            &self.cursor,
            &self.git.diff_cache,
            self.config.syntax_highlighting,
            highlighter,
            &self.search.state,
            &self.selection,
            &self.lsp.diagnostics,
            theme,
            is_focused,
            config.editor.show_git_diff,
            self.config.word_wrap,
            use_smart_wrap,
            content_width,
            content_height,
        );

        // Blame annotation overlay on the cursor line
        self.render_blame_annotation(
            buf,
            area,
            content_width,
            content_height,
            use_smart_wrap,
            theme,
        );
    }

    /// Convert syntax language name to human-readable display name.
    pub(crate) fn format_language_name(syntax_name: &str) -> &str {
        match syntax_name {
            "rust" => "Rust",
            "python" => "Python",
            "go" => "Go",
            "javascript" => "JavaScript",
            "typescript" => "TypeScript",
            "tsx" => "TSX",
            "c" => "C",
            "cpp" => "C++",
            "java" => "Java",
            "ruby" => "Ruby",
            "html" => "HTML",
            "css" => "CSS",
            "json" => "JSON",
            "toml" => "TOML",
            "yaml" => "YAML",
            "bash" => "Bash",
            "markdown" => "Markdown",
            _ => syntax_name,
        }
    }

    /// Render inline blame annotation on the cursor line (VS Code style).
    ///
    /// In word-wrap mode the annotation is anchored to the **last**
    /// wrap-row of `cursor.line`, not the row the cursor happens to be
    /// on. The last wrap-row is the natural remainder of the wrap and
    /// is almost always the shortest fragment, so the annotation has
    /// the most room to fit. When the cursor moves inside the same
    /// logical line the annotation stays put — easier to read than a
    /// label that hops with each cursor step.
    ///
    /// In no-wrap mode there is only one row per logical line, so this
    /// reduces to the obvious thing.
    ///
    /// `use_smart_wrap` must mirror the value used by the main render
    /// pass — the wrap-row arithmetic below recomputes wrap points for
    /// `cursor.line` and the result has to match the actual layout.
    fn render_blame_annotation(
        &self,
        buf: &mut Buffer,
        area: Rect,
        content_width: usize,
        content_height: usize,
        use_smart_wrap: bool,
        theme: &Theme,
    ) {
        let entry = match self.git.blame_for_line(self.cursor.line) {
            Some(e) => e,
            None => return,
        };

        // Each branch returns `(screen_row, visible_code_width)`. The
        // visible_code_width is the column where the line's content
        // ends on `screen_row` — that's where the annotation starts.
        let (anchor_row, visible_code_width): (u16, usize) = if self.config.word_wrap {
            let line_cow = match self.buffer.line_cow(self.cursor.line) {
                Some(s) => s,
                None => return,
            };
            let line_text = line_cow.trim_end_matches('\n');
            let (_, wrap_points) =
                word_wrap::get_line_wrap_points(line_text, content_width, use_smart_wrap);

            // Last wrap-row index inside the logical line. With N wrap
            // points the line spans N+1 visual rows, so the last one is
            // at index N. For an unwrapped line N == 0.
            let last_wrap_row_in_line = wrap_points.len();

            let top_visual = self
                .render_cache
                .get_visual_row_for_line(self.viewport.top_line)
                .unwrap_or(0);
            let line_visual = self
                .render_cache
                .get_visual_row_for_line(self.cursor.line)
                .unwrap_or(0);

            // The cumulative cache counts only wrap rows; deletion markers and
            // diagnostic rows above the cursor line shift it further down.
            let virtual_rows_between = self.count_virtual_rows_between(
                self.viewport.top_line,
                self.cursor.line,
                content_width,
            );
            let last_row_absolute = line_visual + last_wrap_row_in_line + virtual_rows_between;
            let viewport_top_absolute = top_visual + self.viewport.top_visual_row_offset;
            if last_row_absolute < viewport_top_absolute {
                return;
            }
            let row = last_row_absolute - viewport_top_absolute;
            if row >= content_height {
                return;
            }

            // Width of just the last wrap-row's content (display columns).
            // `viewport.left_column` is always 0 in word-wrap mode, so
            // there's no horizontal scroll to subtract here.
            let last_chunk_start = wrap_points.last().copied().unwrap_or(0);
            let last_chunk_width: usize = {
                use unicode_segmentation::UnicodeSegmentation;
                use unicode_width::UnicodeWidthChar;
                line_text
                    .graphemes(true)
                    .skip(last_chunk_start)
                    .flat_map(|g| g.chars())
                    .map(|c| c.width().unwrap_or(0))
                    .sum()
            };

            (row as u16, last_chunk_width.min(content_width))
        } else {
            if self.cursor.line < self.viewport.top_line {
                return;
            }
            // Deletion markers and diagnostic rows above the cursor push its
            // text down on screen; offset by them so the annotation lands on
            // the cursor's row, not the bare logical-line distance.
            let row = (self.cursor.line - self.viewport.top_line)
                + self.count_virtual_rows_between(
                    self.viewport.top_line,
                    self.cursor.line,
                    content_width,
                );
            if row >= content_height {
                return;
            }
            let row = row as u16;

            let line_visual_width: usize = self
                .buffer
                .line(self.cursor.line)
                .map(|l| {
                    use unicode_width::UnicodeWidthChar;
                    l.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>()
                })
                .unwrap_or(0);
            let visible = line_visual_width
                .saturating_sub(self.viewport.left_column)
                .min(content_width);
            (row, visible)
        };

        let line_number_width = rendering::line_number_width(self.buffer.line_count()) as u16;
        // area is already the inner rect (borders stripped by the app layer before calling render)
        let content_x = area.x + line_number_width;
        let line_y = area.y + anchor_row;
        let ann_x = content_x + visible_code_width as u16;

        // Need at least 12 columns for the annotation to be useful.
        let right_edge = area.x + area.width;
        let available = right_edge.saturating_sub(ann_x) as usize;
        if available < 12 {
            return;
        }

        let annotation = entry.inline_text();
        let truncated = termide_git::truncate_right(&annotation, available);
        let style = ratatui::style::Style::default().fg(theme.disabled);
        buf.set_string(ann_x, line_y, &truncated, style);
    }

    /// Render editor content
    pub(crate) fn render_content(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        config: &Config,
        is_focused: bool,
        border_right_x: Option<u16>,
    ) {
        // Update viewport size (subtract space for line numbers)
        let (content_width, content_height) = rendering::calculate_content_dimensions(
            area.width,
            area.height,
            self.buffer.line_count(),
        );

        let effective_width = if self.config.word_wrap {
            content_width
        } else {
            0 // Set to 0 when word wrap is disabled to trigger fallback behavior
        };
        let use_smart_wrap = if self.config.word_wrap && content_width > 0 {
            self.should_use_smart_wrap(config)
        } else {
            false
        };

        // Update wrap settings BEFORE building cumulative cache
        // This ensures cache is invalidated if width changed
        self.render_cache
            .update_wrap_settings(effective_width, use_smart_wrap);
        self.render_cache.content_height = content_height;

        self.viewport.resize(content_width, content_height);

        // Compute and cache virtual line count for viewport calculations
        let virtual_lines_total = self.virtual_line_count(config);
        self.render_cache.virtual_line_count = virtual_lines_total;

        // Ensure cursor is visible (only when viewport follows cursor)
        if self.scroll_follows_cursor {
            if self.config.word_wrap && content_width > 0 {
                // Word wrap mode: use visual row-aware scrolling
                self.ensure_cursor_visible_word_wrap(content_height);
            } else {
                // Standard mode: use physical line scrolling
                self.viewport
                    .ensure_cursor_visible(&self.cursor, virtual_lines_total);
            }
        }

        // Delegate to rendering orchestrator
        rendering::render_editor_content(
            buf,
            area,
            &self.buffer,
            &self.viewport,
            &self.cursor,
            &self.git.diff_cache,
            self.config.syntax_highlighting,
            &mut self.render_cache.highlight,
            &self.search.state,
            &self.selection,
            &self.lsp.diagnostics,
            theme,
            is_focused,
            config.editor.show_git_diff,
            self.config.word_wrap,
            use_smart_wrap,
            content_width,
            content_height,
        );

        // Blame annotation overlay on the cursor line
        self.render_blame_annotation(
            buf,
            area,
            content_width,
            content_height,
            use_smart_wrap,
            theme,
        );

        // Render scrollbar on the right border
        self.scrollbars = termide_core::ScrollBars::default();
        if let Some(border_x) = border_right_x {
            let theme_colors = termide_core::ThemeColors::from(theme);
            let bar = ScrollBar::render_tracked(
                buf,
                border_x,
                area.y,
                area.height,
                self.viewport.top_line,
                content_height,
                virtual_lines_total,
                &theme_colors,
                is_focused,
            );
            self.scrollbars.vertical = bar;
        }

        // Render completion popup if active
        if let Some(ref popup) = self.lsp.completion_popup {
            use unicode_width::UnicodeWidthChar;

            // Only render if cursor is in visible area
            if self.cursor.line >= self.viewport.top_line
                && self.cursor.line < self.viewport.top_line + content_height
            {
                // Calculate cursor screen position
                let line_number_width =
                    rendering::line_number_width(self.buffer.line_count()) as u16;
                let content_x = area.x + 1 + line_number_width; // +1 for border

                // Calculate cursor X position within the line
                // Calculate display width up to cursor column
                let cursor_screen_col: usize = self
                    .buffer
                    .line(self.cursor.line)
                    .map(|line| {
                        line.chars()
                            .take(self.cursor.column)
                            .map(|c| c.width().unwrap_or(0))
                            .sum()
                    })
                    .unwrap_or(0);

                let cursor_x = content_x + cursor_screen_col as u16;
                let cursor_y = area.y + 1 + (self.cursor.line - self.viewport.top_line) as u16;

                // Render popup within editor area only and store rect for mouse hit testing
                self.lsp.popup_rect = popup.render(buf, area, cursor_x, cursor_y, theme);
            } else {
                self.lsp.popup_rect = None;
            }
        } else {
            self.lsp.popup_rect = None;
        }

        // Render code-action popup if active (anchored at the cursor like
        // completion).
        if let Some(ref popup) = self.lsp.code_action_popup {
            use unicode_width::UnicodeWidthChar;
            if self.cursor.line >= self.viewport.top_line
                && self.cursor.line < self.viewport.top_line + content_height
            {
                let line_number_width =
                    rendering::line_number_width(self.buffer.line_count()) as u16;
                let content_x = area.x + 1 + line_number_width;
                let cursor_screen_col: usize = self
                    .buffer
                    .line(self.cursor.line)
                    .map(|line| {
                        line.chars()
                            .take(self.cursor.column)
                            .map(|c| c.width().unwrap_or(0))
                            .sum()
                    })
                    .unwrap_or(0);
                let cursor_x = content_x + cursor_screen_col as u16;
                let cursor_y = area.y + 1 + (self.cursor.line - self.viewport.top_line) as u16;
                popup.render(buf, area, cursor_x, cursor_y, theme);
            }
        }

        // Render hover popup if active
        if let Some(ref popup) = self.lsp.hover_popup {
            // Use stored mouse position for popup placement
            if let Some((mouse_x, mouse_y)) = self.lsp.last_mouse_position {
                self.lsp.hover_popup_rect = popup.render(buf, area, mouse_x, mouse_y, theme);
            } else {
                self.lsp.hover_popup_rect = None;
            }
        } else {
            self.lsp.hover_popup_rect = None;
        }

        // Render color preview popup if active
        if let Some(ref preview) = self.lsp.color_preview {
            preview.render(buf, area);
        }
    }
}

#[cfg(test)]
mod render_highlight_tests {
    use super::Editor;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use std::collections::HashSet;
    use termide_config::Config;
    use termide_theme::Theme;

    /// Render a PHP template through the real `render_content` path and confirm
    /// the editor draws more than one foreground colour — i.e. highlighting is
    /// actually applied on screen, not just inside the highlight cache. Guards
    /// the whole render wiring (`set_document` gate, per-line lookup, cell
    /// styling) that the highlight-crate unit tests can't reach.
    #[test]
    fn php_template_is_rendered_with_multiple_colors() {
        let src =
            "<!DOCTYPE html>\n<html>\n<body>\n<?php\n$name = \"Ivan\";\necho $name;\n?>\n</body>\n";
        let mut editor = Editor::from_text(src, "1.php".to_string());
        assert!(editor.config.syntax_highlighting);
        // Mirror the file-open path which sets syntax from the extension.
        editor.render_cache.highlight.set_syntax("php");

        let theme = Theme::default();
        let config = Config::default();
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);

        editor.render_content(area, &mut buf, &theme, &config, true, None);

        let mut colors: HashSet<Color> = HashSet::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    if cell.symbol() != " " {
                        colors.insert(cell.fg);
                    }
                }
            }
        }

        assert!(
            colors.len() > 1,
            "expected multiple foreground colours from PHP highlighting, saw {colors:?}"
        );
    }

    /// Same as above but mirrors a light-theme session (the reported repro used
    /// `github-light`): set the highlight cache's theme exactly as
    /// `prepare_render` does before rendering.
    #[test]
    fn php_template_is_rendered_with_multiple_colors_light_theme() {
        let src =
            "<!DOCTYPE html>\n<html>\n<body>\n<?php\n$name = \"Ivan\";\necho $name;\n?>\n</body>\n";
        let mut editor = Editor::from_text(src, "1.php".to_string());
        editor.render_cache.highlight.set_syntax("php");

        let theme = *Theme::get_by_name("github-light");
        // Mirror prepare_render's highlight sync.
        editor
            .render_cache
            .highlight
            .set_light_theme(theme.is_light_theme());
        editor.render_cache.highlight.set_default_fg(theme.fg);

        let config = Config::default();
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);

        editor.render_content(area, &mut buf, &theme, &config, true, None);

        let mut colors: HashSet<Color> = HashSet::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    if cell.symbol() != " " {
                        colors.insert(cell.fg);
                    }
                }
            }
        }

        assert!(
            colors.len() > 1,
            "light theme: expected multiple foreground colours, saw {colors:?}"
        );
    }
}
