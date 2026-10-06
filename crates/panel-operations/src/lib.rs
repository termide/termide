//! Operations Panel for termide.
//!
//! Provides a panel for displaying and managing active file operations
//! (copy, move, upload, download, delete) with progress tracking.

#![allow(clippy::too_many_arguments)]

pub mod rendering;

use std::any::Any;
use std::path::Path;
use std::time::Instant;

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

use termide_config::Config;
use termide_core::{
    CommandResult, ConfirmAction, HeightMode, HotkeyTable, Panel, PanelCommand, PanelEvent,
    PanelState, RenderContext, ThemeColors, WidthPreference,
};
use termide_file_ops::OperationId;
use termide_state::{ActiveOperation, OperationProgress, OperationType};
use termide_theme::Theme;

pub use rendering::format_bytes;

/// Lightweight snapshot of an operation for rendering.
/// Copied from ActiveOperation to avoid borrowing issues.
#[derive(Debug, Clone)]
pub struct OperationSnapshot {
    pub id: OperationId,
    pub op_type: OperationType,
    pub source: String,
    pub dest: String,
    pub progress: OperationProgress,
    pub is_paused: bool,
    pub is_scanning: bool,
    pub started_at: Instant,
    pub speed: f64, // bytes per second
    /// The project a command runs in, when it is not the current one.
    pub project: Option<String>,
}

impl OperationSnapshot {
    /// Create a snapshot from an ActiveOperation reference, with
    /// `current_project` the root of the project on screen.
    pub fn from_active(op: &ActiveOperation, current_project: &Path) -> Self {
        Self {
            id: op.id,
            op_type: op.op_type,
            source: op.source.clone(),
            dest: op.dest.clone(),
            progress: op.progress.clone(),
            is_paused: op.is_paused,
            is_scanning: op.is_scanning,
            started_at: op.started_at,
            speed: op.speed_tracker.speed(),
            project: (op.op_type.is_command() && op.project != current_project)
                .then(|| termide_core::util::shorten_home_path(&op.project.display().to_string())),
        }
    }

    /// Card height for this operation based on its type and state.
    /// Type label and percent are in the border title, not content lines.
    pub fn card_height(&self) -> u16 {
        let is_command = self.op_type.is_command();
        let has_dest = !self.dest.is_empty();
        let has_data = !self.is_scanning && self.op_type.has_data_progress();
        // Content lines:
        //   Command: dest(?) + project(?) + elapsed(1) (name is in border title)
        //   File op: bar(1) + source(1) + dest(?) + files(1) + data+speed(?) + elapsed(1)
        let content_lines: u16 = if is_command {
            has_dest as u16 + self.project.is_some() as u16 + 1
        } else {
            1 // progress bar
            + 1 // source path
            + has_dest as u16
            + 1 // files count
            + if has_data { 2 } else { 0 }
            + 1 // elapsed
        };
        content_lines + 2 // + top/bottom border
    }
}

/// Operations Panel - shows active file operations
pub struct OperationsPanel {
    /// Currently selected operation index
    selected_index: usize,
    /// Scroll offset for long operation lists
    scroll_offset: usize,
    /// Cached theme colors for rendering
    cached_theme: ThemeColors,
    /// Cached vim_mode setting
    vim_mode: bool,
    /// Last rendered area (for mouse handling)
    last_area: Rect,
    /// Card areas for mouse click detection (operation_index, area)
    card_areas: Vec<(usize, Rect)>,
    /// Snapshot of operations for rendering (updated before each render)
    operations: Vec<OperationSnapshot>,
    /// Hotkey table for configurable keyboard shortcuts
    hotkeys: HotkeyTable,
}

impl OperationsPanel {
    /// Create a new Operations panel
    pub fn new() -> Self {
        Self {
            selected_index: 0,
            scroll_offset: 0,
            cached_theme: ThemeColors::default(),
            vim_mode: false,
            last_area: Rect::default(),
            card_areas: Vec::new(),
            operations: Vec::new(),
            hotkeys: HotkeyTable::default(),
        }
    }

    /// Update operations snapshot from active operations, with
    /// `current_project` the root of the project on screen.
    /// Should be called before rendering.
    pub fn update_operations(&mut self, operations: &[&ActiveOperation], current_project: &Path) {
        self.operations = operations
            .iter()
            .map(|op| OperationSnapshot::from_active(op, current_project))
            .collect();

        // Ensure selected index is valid
        if !self.operations.is_empty() && self.selected_index >= self.operations.len() {
            self.selected_index = self.operations.len() - 1;
        }
    }

    /// Get the currently selected operation ID.
    pub fn selected_operation_id(&self) -> Option<OperationId> {
        self.operations.get(self.selected_index).map(|op| op.id)
    }

    /// Build the "confirm cancel" event for the selected operation, if any.
    /// Cancelling an operation is always gated behind a confirmation modal
    /// (Esc and Delete/Backspace alike); the actual cancel runs only after
    /// the user accepts. Returns `None` when nothing is selected.
    fn cancel_confirm(&self) -> Option<PanelEvent> {
        let op_id = self.selected_operation_id()?;
        let t = termide_i18n::t();
        Some(PanelEvent::ShowConfirm {
            message: t.operation_cancel_confirm().to_string(),
            on_confirm: ConfirmAction::CancelOperation(op_id),
        })
    }

    /// Get operations snapshot for rendering.
    pub fn operations(&self) -> &[OperationSnapshot] {
        &self.operations
    }

    /// Get currently selected operation index
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Set selected operation index
    pub fn set_selected(&mut self, index: usize) {
        self.selected_index = index;
    }

    /// Select next operation
    pub fn select_next(&mut self, total: usize) {
        if total > 0 && self.selected_index < total - 1 {
            self.selected_index += 1;
            self.ensure_cursor_visible(total);
        }
    }

    /// Select previous operation
    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.ensure_cursor_visible_up();
        }
    }

    /// Select first operation
    pub fn select_first(&mut self) {
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Select last operation
    pub fn select_last(&mut self, total: usize) {
        if total > 0 {
            self.selected_index = total - 1;
            self.ensure_cursor_visible(total);
        }
    }

    /// Ensure cursor is visible after moving down
    fn ensure_cursor_visible(&mut self, total: usize) {
        let viewport_height = self.last_area.height;
        // Count how many cards fit starting from scroll_offset
        let visible_cards = self.count_visible_cards(viewport_height);

        if visible_cards == 0 {
            return;
        }

        // Adjust scroll if cursor is below visible area
        if self.selected_index >= self.scroll_offset + visible_cards {
            self.scroll_offset = self.selected_index.saturating_sub(visible_cards) + 1;
        }

        // Clamp scroll offset to valid range
        let max_scroll = total.saturating_sub(visible_cards);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    /// Count how many operations fit in the given viewport height starting from scroll_offset.
    fn count_visible_cards(&self, viewport_height: u16) -> usize {
        let mut y = 0u16;
        let mut count = 0;
        for op in self.operations.iter().skip(self.scroll_offset) {
            let h = op.card_height();
            if y + h > viewport_height {
                break;
            }
            y += h;
            count += 1;
        }
        count
    }

    /// Ensure cursor is visible after moving up
    fn ensure_cursor_visible_up(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        }
    }
}

impl Default for OperationsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel for OperationsPanel {
    fn name(&self) -> &'static str {
        "operations"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferNarrow
    }

    /// Exactly the rows the cards need — the panel grows and shrinks with
    /// the operation list instead of taking a share of the column. Two rows
    /// for the border, then the cards, or the one-line notice when empty.
    fn height_mode(&self) -> HeightMode {
        let content = if self.operations.is_empty() {
            1
        } else {
            self.operations
                .iter()
                .fold(0u16, |rows, op| rows.saturating_add(op.card_height()))
        };
        HeightMode::FitContent(content.saturating_add(2))
    }

    fn title(&self) -> String {
        let t = termide_i18n::t();
        t.panel_operations().to_string()
    }

    fn prepare_render(&mut self, theme: &Theme, config: &std::sync::Arc<Config>) {
        self.cached_theme = ThemeColors::from(theme);
        self.vim_mode = config.general.vim_mode;
        // Operations panel has no configurable hotkeys — vim navigation only
        let _ = &self.hotkeys;
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_area = area;

        if self.operations.is_empty() {
            self.render_empty(area, buf, ctx);
            return;
        }

        // Render operations using the custom rendering function
        let card_areas = rendering::render_operations_panel_snapshots(
            &self.operations,
            self.selected_index,
            self.scroll_offset,
            area,
            buf,
            ctx.is_focused,
            ctx.theme.fg,
            ctx.theme.border_focused,
            ctx.theme.disabled,
        );

        self.card_areas = card_areas;
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        // No text input here: every key is a shortcut, matched on the
        // layout-normalized form so it works on a Cyrillic layout too.
        let key = chord.canonical;
        let total = self.operations.len();
        let mut events = vec![];

        match key.code {
            // Navigation
            KeyCode::Up | KeyCode::Char('k') if self.vim_mode || key.code == KeyCode::Up => {
                self.select_prev();
                events.push(PanelEvent::NeedsRedraw);
            }
            KeyCode::Down | KeyCode::Char('j') if self.vim_mode || key.code == KeyCode::Down => {
                self.select_next(total);
                events.push(PanelEvent::NeedsRedraw);
            }
            KeyCode::Home | KeyCode::Char('g') if self.vim_mode || key.code == KeyCode::Home => {
                self.select_first();
                events.push(PanelEvent::NeedsRedraw);
            }
            KeyCode::End | KeyCode::Char('G') if self.vim_mode || key.code == KeyCode::End => {
                self.select_last(total);
                events.push(PanelEvent::NeedsRedraw);
            }

            // Pause/Resume (Space)
            KeyCode::Char(' ') => {
                if let Some(op_id) = self.selected_operation_id() {
                    events.push(PanelEvent::ToggleOperationPause(op_id));
                }
            }

            // Cancel operation (Delete/Backspace). Like Escape, cancelling
            // always asks for confirmation first — see `cancel_confirm`.
            KeyCode::Delete | KeyCode::Backspace => {
                if let Some(event) = self.cancel_confirm() {
                    events.push(event);
                }
            }

            // Escape: if something is selected, treat it as "cancel the
            // selected operation" rather than "close the panel". The
            // matching captures_escape() impl keeps the app's default
            // close-panel-on-Esc from firing in that case.
            KeyCode::Esc => {
                if let Some(event) = self.cancel_confirm() {
                    events.push(event);
                }
            }

            _ => {}
        }

        events
    }

    fn captures_escape(&self) -> bool {
        // Swallow Escape only when there's a selected operation to cancel.
        // With no selection, Escape falls through to the app and closes
        // the panel as usual.
        self.selected_operation_id().is_some()
    }

    fn handle_mouse(&mut self, event: MouseEvent, _panel_area: Rect) -> Vec<PanelEvent> {
        let col = event.column;
        let row = event.row;

        match event.kind {
            MouseEventKind::ScrollUp => {
                self.select_prev();
            }
            MouseEventKind::ScrollDown => {
                // Need total count, handled by app
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Check which card was clicked
                for (idx, card_area) in &self.card_areas {
                    if col >= card_area.x
                        && col < card_area.x + card_area.width
                        && row >= card_area.y
                        && row < card_area.y + card_area.height
                    {
                        self.selected_index = *idx;
                        // The card's top border row carries " [X] Label …"
                        // where [X] is the type icon button. Clicking it
                        // opens the per-operation action menu, mirroring
                        // the panel header `[≡]` behaviour. Bracketed
                        // icon takes ~5 cols (border + space + "[X] ").
                        const ICON_HIT_WIDTH: u16 = 6;
                        let in_icon_zone =
                            row == card_area.y && col < card_area.x.saturating_add(ICON_HIT_WIDTH);
                        if in_icon_zone {
                            if let Some(op) = self.operations.get(*idx) {
                                return vec![PanelEvent::OpenOperationActionMenu {
                                    op_id: op.id,
                                    anchor_x: col,
                                    anchor_y: row,
                                }];
                            }
                        }
                        break;
                    }
                }
            }
            _ => {}
        }

        vec![]
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        let _ = cmd;
        CommandResult::None
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        // Operations panel is transient, don't persist to the layout
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl OperationsPanel {
    /// Render empty state (no operations)
    fn render_empty(&self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        use ratatui::{style::Style, text::Line, widgets::Paragraph};

        let t = termide_i18n::t();
        let text = Paragraph::new(Line::from(t.no_active_operations()))
            .style(Style::default().fg(ctx.theme.disabled))
            .alignment(ratatui::layout::Alignment::Center);
        text.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use termide_core::{ConfirmAction, KeyChord, Panel, PanelEvent};
    use termide_state::OperationProgress;

    fn key(code: KeyCode) -> KeyChord {
        KeyChord::identity(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn panel_with_one_op(id: u64) -> OperationsPanel {
        let mut panel = OperationsPanel::new();
        panel.operations.push(OperationSnapshot {
            id: OperationId(id),
            op_type: OperationType::CopyDownload,
            source: "remote".into(),
            dest: "local".into(),
            progress: OperationProgress::default(),
            is_paused: false,
            is_scanning: false,
            started_at: Instant::now(),
            speed: 0.0,
            project: None,
        });
        panel.selected_index = 0;
        panel
    }

    /// The panel asks the column for exactly its cards plus the two border
    /// rows, and for the one-line notice when it has nothing to show.
    #[test]
    fn height_follows_the_cards() {
        let empty = OperationsPanel::new();
        assert_eq!(empty.height_mode(), HeightMode::FitContent(3));

        let mut panel = panel_with_one_op(1);
        let one_card = panel.operations[0].card_height();
        assert_eq!(panel.height_mode(), HeightMode::FitContent(one_card + 2));

        let second = panel.operations[0].clone();
        panel.operations.push(OperationSnapshot {
            id: OperationId(2),
            ..second
        });
        assert_eq!(
            panel.height_mode(),
            HeightMode::FitContent(2 * one_card + 2)
        );
    }

    /// A command started in another project names it on its card, one line
    /// taller; a file operation shows its paths and never does.
    #[test]
    fn a_command_of_another_project_names_it() {
        let here = Path::new("/work/here");
        let command = |project: &str| {
            ActiveOperation::new(
                OperationId(1),
                OperationType::CommandReport,
                "build".into(),
                String::new(),
                0,
                0,
                project.into(),
            )
        };

        let local = OperationSnapshot::from_active(&command("/work/here"), here);
        assert_eq!(local.project, None);
        let other = OperationSnapshot::from_active(&command("/work/there"), here);
        assert_eq!(other.project.as_deref(), Some("/work/there"));
        assert_eq!(other.card_height(), local.card_height() + 1);

        let copy = ActiveOperation::new(
            OperationId(2),
            OperationType::Copy,
            "a".into(),
            "b".into(),
            1,
            1,
            "/work/there".into(),
        );
        assert_eq!(OperationSnapshot::from_active(&copy, here).project, None);
    }

    /// Cancelling is always gated behind a confirmation modal: the key must
    /// emit `ShowConfirm(CancelOperation)` and never cancel directly.
    fn assert_requests_confirmation(code: KeyCode, id: u64) {
        // The cancel path pulls the confirmation message from i18n.
        let _ = termide_i18n::init();
        let mut panel = panel_with_one_op(id);
        let events = panel.handle_key(key(code));

        assert!(
            events.iter().any(|e| matches!(
                e,
                PanelEvent::ShowConfirm {
                    on_confirm: ConfirmAction::CancelOperation(op),
                    ..
                } if op.0 == id
            )),
            "{code:?} should emit ShowConfirm(CancelOperation), got {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PanelEvent::CancelOperation(_))),
            "{code:?} must not cancel without confirmation, got {events:?}"
        );
    }

    #[test]
    fn esc_requests_confirmation() {
        assert_requests_confirmation(KeyCode::Esc, 7);
    }

    #[test]
    fn delete_requests_confirmation() {
        assert_requests_confirmation(KeyCode::Delete, 9);
    }

    #[test]
    fn backspace_requests_confirmation() {
        assert_requests_confirmation(KeyCode::Backspace, 11);
    }
}
