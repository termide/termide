//! Outline panel for termide.
//!
//! Displays a navigable list of structural symbols (functions, classes, structs, etc.)
//! extracted from the active editor's source code using tree-sitter queries,
//! with language-specific fallback extractors for formats such as Markdown
//! and Alatyr.

mod alatyr;
mod regex_fallback;
pub mod symbols;
mod treesitter;

use std::any::Any;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up};
use unicode_width::UnicodeWidthStr;

use termide_core::{
    CommandResult, Panel, PanelCommand, PanelEvent, RenderContext, ThemeColors, WidthPreference,
};
use termide_theme::Theme;
use termide_ui::ScrollBar;

pub use symbols::{SymbolInfo, SymbolKind};

/// Pending navigation request from outline to editor.
pub struct OutlineNavigation {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

/// Outline panel showing structural symbols from the active editor.
pub struct OutlinePanel {
    /// Scrollbar drawn by the last render, for mouse thumb dragging.
    scrollbars: termide_core::ScrollBars,
    /// Extracted symbols for the tracked file.
    symbols: Vec<SymbolInfo>,
    /// Path of the file currently being tracked.
    tracked_file: Option<PathBuf>,
    /// Language of the tracked file.
    tracked_language: Option<String>,

    /// Currently selected index.
    selected_index: usize,
    /// Scroll offset (top visible item).
    scroll_offset: usize,
    /// Last rendered area height.
    last_height: usize,

    /// Cached theme for rendering.
    cached_theme: Theme,
    /// Vim mode for j/k navigation.
    vim_mode: bool,

    /// Pending navigation: the app tick handler reads and clears this.
    pending_navigation: Option<OutlineNavigation>,

    /// Whether the panel is stale (collapsed, awaiting refresh).
    is_stale: bool,

    /// Whether the panel needs the app to re-push content after unstale.
    needs_repopulate: bool,

    /// Reusable tree-sitter parser (avoids allocation on every parse).
    parser: tree_sitter::Parser,

    /// Cached tree-drawing prefixes, computed once per symbol update.
    tree_prefixes: Vec<String>,
}

impl OutlinePanel {
    /// Create a new outline panel.
    pub fn new(theme: Theme) -> Self {
        Self {
            symbols: Vec::new(),
            tracked_file: None,
            tracked_language: None,
            selected_index: 0,
            scrollbars: termide_core::ScrollBars::default(),
            scroll_offset: 0,
            last_height: 10,
            cached_theme: theme,
            vim_mode: false,
            pending_navigation: None,
            is_stale: false,
            needs_repopulate: false,
            parser: tree_sitter::Parser::new(),
            tree_prefixes: Vec::new(),
        }
    }

    /// Update content from the active editor.
    ///
    /// Skips re-parsing if the panel is stale (collapsed).
    /// Resets selection when the tracked file changes.
    pub fn update_content(
        &mut self,
        file_path: Option<PathBuf>,
        content: &str,
        language: Option<&str>,
    ) {
        if self.is_stale {
            let file_changed = self.tracked_file != file_path;
            self.tracked_file = file_path;
            self.tracked_language = language.map(|s| s.to_string());
            if file_changed {
                self.symbols.clear();
                self.tree_prefixes.clear();
                self.selected_index = 0;
                self.scroll_offset = 0;
            }
            return;
        }

        let file_changed = self.tracked_file != file_path;
        self.tracked_file = file_path.clone();
        self.tracked_language = language.map(|s| s.to_string());

        self.symbols =
            symbols::extract_symbols(content, language, file_path.as_deref(), &mut self.parser);

        // Precompute tree-drawing prefixes
        self.tree_prefixes = (0..self.symbols.len())
            .map(|i| compute_tree_prefix(&self.symbols, i))
            .collect();

        // Reset selection when file changes
        if file_changed {
            self.selected_index = 0;
            self.scroll_offset = 0;
        }

        // Clamp selection
        if !self.symbols.is_empty() && self.selected_index >= self.symbols.len() {
            self.selected_index = self.symbols.len() - 1;
        }
    }

    /// Move selection up.
    fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
        self.ensure_visible();
    }

    /// Move selection down.
    fn select_next(&mut self) {
        if self.selected_index + 1 < self.symbols.len() {
            self.selected_index += 1;
        }
        self.ensure_visible();
    }

    /// Sync selection to the symbol containing the given editor cursor line.
    ///
    /// Finds the last symbol whose start line is <= cursor_line (i.e. the
    /// deepest enclosing section) and selects it.
    pub fn sync_cursor_line(&mut self, cursor_line: usize) {
        if self.symbols.is_empty() {
            return;
        }
        // Binary search: find the last symbol whose line <= cursor_line
        let best = self
            .symbols
            .partition_point(|s| s.line <= cursor_line)
            .saturating_sub(1);
        if self.selected_index != best {
            self.selected_index = best;
            self.ensure_visible();
        }
    }

    /// Store pending navigation for the currently selected symbol.
    fn navigate_to_selected(&mut self) {
        if let Some(symbol) = self.symbols.get(self.selected_index) {
            if let Some(ref path) = self.tracked_file {
                self.pending_navigation = Some(OutlineNavigation {
                    path: path.clone(),
                    line: symbol.line,
                    column: symbol.column,
                });
            }
        }
    }

    /// Take pending navigation request (called by app tick handler).
    pub fn take_pending_navigation(&mut self) -> Option<OutlineNavigation> {
        self.pending_navigation.take()
    }

    /// Get the path of the currently tracked file.
    pub fn tracked_file(&self) -> Option<&std::path::Path> {
        self.tracked_file.as_deref()
    }

    /// Return sorted line positions of all symbols (for editor Ctrl+Up/Down navigation).
    pub fn symbol_lines(&self) -> Vec<usize> {
        self.symbols.iter().map(|s| s.line).collect()
    }

    /// Clear the outline (no editor is tracking).
    pub fn clear(&mut self) {
        self.tracked_file = None;
        self.tracked_language = None;
        self.symbols.clear();
        self.tree_prefixes.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.needs_repopulate = false;
    }

    /// Check and clear the repopulate flag (called by app tick handler).
    pub fn needs_repopulate(&mut self) -> bool {
        std::mem::take(&mut self.needs_repopulate)
    }

    /// Ensure selected item is visible.
    fn ensure_visible(&mut self) {
        let content_height = self.last_height;
        self.scroll_offset = termide_ui::ensure_offset_visible(
            self.scroll_offset,
            self.selected_index,
            content_height,
        );
    }
}

/// Compute tree-drawing prefix for a symbol at `index`.
///
/// Returns a string like `"├─ "`, `"│  └─ "`, etc.
/// Top-level symbols (depth 0) get an empty string.
fn compute_tree_prefix(symbols: &[SymbolInfo], index: usize) -> String {
    let depth = symbols[index].depth;
    if depth == 0 {
        return String::new();
    }

    let mut prefix = String::with_capacity(depth * 3);
    for lvl in 1..=depth {
        // Scan forward to determine if there is a next sibling at this level.
        let has_next = symbols[index + 1..]
            .iter()
            .find(|s| s.depth <= lvl)
            .is_some_and(|s| s.depth == lvl);

        if lvl == depth {
            // Last segment — branch or corner
            if has_next {
                prefix.push_str("├─ ");
            } else {
                prefix.push_str("└─ ");
            }
        } else {
            // Ancestor column — continuation bar or blank
            if has_next {
                prefix.push_str("│  ");
            } else {
                prefix.push_str("   ");
            }
        }
    }
    prefix
}

impl Panel for OutlinePanel {
    fn name(&self) -> &'static str {
        "outline"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferNarrow
    }

    fn title(&self) -> String {
        let t = termide_i18n::t();
        let base = t.outline_title();
        match self.tracked_file.as_ref().and_then(|p| p.file_name()) {
            Some(name) => format!("{} {}", base, name.to_string_lossy()),
            None => base.to_string(),
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::MarkStale => {
                self.is_stale = true;
                CommandResult::None
            }
            PanelCommand::RefreshIfStale => {
                if self.is_stale {
                    self.is_stale = false;
                    if self.tracked_file.is_some() && self.symbols.is_empty() {
                        self.needs_repopulate = true;
                    }
                    CommandResult::NeedsRedraw(true)
                } else {
                    CommandResult::None
                }
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.scroll_offset = offset;
                // The wheel moves the selection here and lets `ensure_visible`
                // drag the window along; a thumb drag moves the window, so pull
                // the selection into it to keep the two consistent.
                let visible = self.scrollbars.vertical.map_or(1, |bar| bar.visible).max(1);
                let last = self.symbols.len().saturating_sub(1);
                let low = offset.min(last);
                let high = (offset + visible - 1).min(last);
                self.selected_index = self.selected_index.clamp(low, high);
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<termide_config::Config>) {
        self.cached_theme = *theme;
        self.vim_mode = config.general.vim_mode;
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_height = area.height as usize;
        let theme = self.cached_theme;

        // Clear area
        let bg_style = Style::default().bg(theme.bg).fg(theme.fg);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_style(bg_style);
                buf[(x, y)].set_char(' ');
            }
        }

        // Content area (no header — title already shown by the framework)
        let content_top = area.top();
        let content_height = area.height as usize;

        if self.symbols.is_empty() {
            // Show "No symbols" message
            let t = termide_i18n::t();
            let msg = t.outline_no_symbols();
            let msg_y = content_top + content_height as u16 / 2;
            let msg_x = area.left() + (area.width.saturating_sub(msg.width() as u16)) / 2;

            let dim_style = Style::default().fg(theme.accented_fg);
            for (i, ch) in msg.chars().enumerate() {
                let x = msg_x + i as u16;
                if x < area.right() {
                    buf[(x, msg_y)].set_char(ch);
                    buf[(x, msg_y)].set_style(dim_style);
                }
            }
        } else {
            let wide_mode = area.width >= termide_config::defaults::TREE_VIEW_MIN_WIDTH;

            // Render symbol entries
            for display_idx in 0..content_height {
                let sym_idx = self.scroll_offset + display_idx;
                if sym_idx >= self.symbols.len() {
                    break;
                }
                let symbol = &self.symbols[sym_idx];
                let y = content_top + display_idx as u16;
                // Selection cursor hidden when the panel isn't focused.
                let is_selected = ctx.is_focused && sym_idx == self.selected_index;

                // Determine styles
                let line_style = if is_selected {
                    Style::default().bg(theme.fg).fg(theme.bg)
                } else {
                    bg_style
                };

                let icon_style = if is_selected {
                    Style::default().bg(theme.accented_fg).fg(theme.bg)
                } else {
                    Style::default().bg(theme.bg).fg(theme.accented_fg)
                };

                // Clear line
                for x in area.left()..area.right() {
                    buf[(x, y)].set_style(line_style);
                    buf[(x, y)].set_char(' ');
                }

                // Use cached tree prefix (wide mode only, depth > 0)
                let prefix = if wide_mode && symbol.depth > 0 {
                    self.tree_prefixes
                        .get(sym_idx)
                        .map(|s| s.as_str())
                        .unwrap_or("")
                } else {
                    ""
                };

                // Render: [1 space][prefix][icon] [name]  :[line]
                let mut cursor_x = area.left() + 1;

                // Render tree prefix characters
                let prefix_style = if is_selected {
                    Style::default().bg(theme.fg).fg(theme.disabled)
                } else {
                    Style::default().bg(theme.bg).fg(theme.disabled)
                };
                for ch in prefix.chars() {
                    if cursor_x < area.right() {
                        buf[(cursor_x, y)].set_char(ch);
                        buf[(cursor_x, y)].set_style(prefix_style);
                    }
                    cursor_x += 1;
                }

                // Render icon (code symbols only)
                let name_x = if symbol.kind.is_code() {
                    if cursor_x < area.right() {
                        buf[(cursor_x, y)].set_char(symbol.kind.icon());
                        buf[(cursor_x, y)].set_style(icon_style);
                    }
                    cursor_x + 2
                } else {
                    cursor_x
                };
                let is_prominent = if symbol.kind.is_code() {
                    symbol.depth == 0
                } else {
                    symbol.depth <= 1
                };
                let name_style = if is_selected || is_prominent {
                    line_style
                } else {
                    Style::default()
                        .bg(if is_selected { theme.fg } else { theme.bg })
                        .fg(theme.disabled)
                };

                // In flat mode, use full_name when available for context
                let display_name = if !wide_mode {
                    symbol.full_name.as_deref().unwrap_or(&symbol.name)
                } else {
                    &symbol.name
                };
                let mut x_offset: u16 = 0;
                for ch in display_name.chars() {
                    let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    let x = name_x + x_offset;
                    if x + ch_width as u16 > area.right().saturating_sub(6) {
                        break;
                    }
                    buf[(x, y)].set_char(ch);
                    buf[(x, y)].set_style(name_style);
                    x_offset += ch_width as u16;
                }

                // Render line number (right-aligned)
                let line_str = format!(":{}", symbol.line + 1);
                let line_num_x = area.right().saturating_sub(line_str.width() as u16 + 1);
                let line_num_style = if is_selected {
                    Style::default().bg(theme.fg).fg(theme.bg)
                } else {
                    Style::default().bg(theme.bg).fg(theme.disabled)
                };
                for (i, ch) in line_str.chars().enumerate() {
                    let x = line_num_x + i as u16;
                    if x < area.right() && x > name_x {
                        buf[(x, y)].set_char(ch);
                        buf[(x, y)].set_style(line_num_style);
                    }
                }
            }
        }

        // Render scrollbar
        self.scrollbars.vertical = None;
        if self.symbols.len() > content_height && area.width > 2 {
            let scrollbar_x = area.right() - 1;
            let theme_colors = ThemeColors::from(&theme);
            self.scrollbars.vertical = ScrollBar::render_tracked(
                buf,
                scrollbar_x,
                content_top,
                content_height as u16,
                self.scroll_offset,
                content_height,
                self.symbols.len(),
                &theme_colors,
                true,
            );
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        let key = chord.raw;
        // Ctrl+C: copy selected symbol name to clipboard
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if let Some(symbol) = self.symbols.get(self.selected_index) {
                let _ = termide_clipboard::copy(&symbol.name);
            }
            return vec![];
        }

        // Vim-mode navigation (j/k/g/G)
        if is_move_up(&key, self.vim_mode) {
            self.select_prev();
            self.navigate_to_selected();
            return vec![];
        }
        if is_move_down(&key, self.vim_mode) {
            self.select_next();
            self.navigate_to_selected();
            return vec![];
        }
        if is_go_home(&key, self.vim_mode) {
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.navigate_to_selected();
            return vec![];
        }
        if is_go_end(&key, self.vim_mode) {
            self.selected_index = self.symbols.len().saturating_sub(1);
            self.ensure_visible();
            self.navigate_to_selected();
            return vec![];
        }

        match key.code {
            KeyCode::PageUp => {
                let page_size = self.last_height;
                for _ in 0..page_size {
                    self.select_prev();
                }
                self.navigate_to_selected();
            }
            KeyCode::PageDown => {
                let page_size = self.last_height;
                for _ in 0..page_size {
                    self.select_next();
                }
                self.navigate_to_selected();
            }
            KeyCode::Enter => {
                self.navigate_to_selected();
            }
            _ => {}
        }
        vec![]
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Vec<PanelEvent> {
        // area is the FULL panel rect (including border/title).
        // Content starts 1 row below the top border.
        let content_top = area.top() + 1;
        let content_bottom = area.bottom().saturating_sub(1);

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.select_prev();
            }
            MouseEventKind::ScrollDown => {
                self.select_next();
            }
            MouseEventKind::Down(MouseButton::Left)
                if mouse.row >= content_top && mouse.row < content_bottom =>
            {
                let click_offset = (mouse.row - content_top) as usize;
                let new_idx = self.scroll_offset + click_offset;
                if new_idx < self.symbols.len() {
                    self.selected_index = new_idx;
                    self.navigate_to_selected();
                }
            }
            _ => {}
        }
        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, _area: Rect) -> Vec<PanelEvent> {
        let count = self.symbols.len();
        let lines = delta.unsigned_abs() as usize;

        if delta < 0 {
            self.selected_index = self.selected_index.saturating_sub(lines);
        } else {
            self.selected_index = (self.selected_index + lines).min(count.saturating_sub(1));
        }
        self.ensure_visible();
        vec![]
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<termide_core::SessionPanel> {
        Some(termide_core::SessionPanel::Outline)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
