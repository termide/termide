//! Settings modal with tabbed interface for editing application configuration.

use anyhow::Result;
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Clear, Widget},
};
use termide_config::Config;
use termide_i18n as i18n;
use termide_theme::Theme;

use crate::{Modal, ModalResult};

mod fields;
mod input;
mod kb;
mod render;
mod state;

use kb::kb_binding_names;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Which settings tab is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Editor,
    FileManager,
    Terminal,
    Lsp,
    Logging,
    Vfs,
    Keybindings,
}

/// Top-level leaf tabs in the sidebar (excluding the Keybindings group).
const TOP_LEVEL_TABS: [SettingsTab; 7] = [
    SettingsTab::General,
    SettingsTab::Editor,
    SettingsTab::FileManager,
    SettingsTab::Terminal,
    SettingsTab::Lsp,
    SettingsTab::Logging,
    SettingsTab::Vfs,
];

/// Sidebar width in columns.
const MODAL_SIDEBAR_WIDTH: u16 = 18;

impl SettingsTab {
    fn label(self) -> String {
        let t = i18n::t();
        match self {
            SettingsTab::General => t.settings_tab_general().to_string(),
            SettingsTab::Editor => t.settings_tab_editor().to_string(),
            SettingsTab::FileManager => t.settings_tab_file_manager().to_string(),
            SettingsTab::Terminal => t.settings_tab_terminal().to_string(),
            SettingsTab::Lsp => t.settings_tab_lsp().to_string(),
            SettingsTab::Logging => t.settings_tab_logging().to_string(),
            SettingsTab::Vfs => t.settings_tab_vfs().to_string(),
            SettingsTab::Keybindings => t.settings_tab_keybindings().to_string(),
        }
    }
}

/// Which UI zone has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Sidebar,
    Content,
    Buttons,
}

/// A single visible row in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarRow {
    /// Top-level leaf — activating sets `active_tab`.
    Leaf(SettingsTab),
    /// Expandable "Keybindings" group header.
    KbGroupHeader,
    /// Keybindings subsection (index into `KB_SECTIONS`, 0..7).
    KbChild(usize),
}

/// LSP tab sub-mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LspMode {
    /// Normal field browsing + server list.
    Fields,
    /// Editing an LSP server (new or existing).
    ServerEdit,
}

/// Keybindings tab sub-mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KbMode {
    /// Browsing bindings for the active section — user picks one to rebind.
    Bindings,
    /// Capturing a keypress for the selected binding.
    Capturing,
}

/// Result returned when the settings modal closes.
///
/// `Apply` and `CreateProjectOverride` box `Config` because it's large
/// (~3.6 KB) and infrequent — keeping the enum small avoids bloating
/// every `ModalResult` carrier.
#[derive(Debug)]
pub enum SettingsResult {
    /// User clicked "Apply & Save" — apply and persist the config to the
    /// currently-active target (project file if it exists, global
    /// otherwise).
    Apply(Box<Config>),
    /// User clicked "Create project override" — write the current config
    /// as a per-project diff against `defaults + global`.
    CreateProjectOverride(Box<Config>),
    /// User clicked "Remove project override" — delete the project file
    /// (with a confirmation step handled by the caller).
    RemoveProjectOverride,
    /// User clicked "Cancel" (or Esc from tab bar).
    Cancel,
}

/// Bottom buttons. The third slot toggles between
/// "Create / Remove project override" depending on
/// `SettingsModal::project_override_active`. The trailing slot is
/// Cancel and is matched as the catch-all in `execute_selected_button`.
const BUTTON_APPLY: usize = 0;
const BUTTON_RESET: usize = 1;
const BUTTON_PROJECT_OVERRIDE: usize = 2;
const BUTTON_COUNT: usize = 4;

/// Get localized button labels. The third label depends on whether the
/// per-project override file currently exists.
fn button_labels(project_override_active: bool) -> [String; BUTTON_COUNT] {
    let t = i18n::t();
    let project_label = if project_override_active {
        t.settings_btn_remove_project_override()
    } else {
        t.settings_btn_create_project_override()
    };
    [
        t.settings_btn_apply().to_string(),
        t.settings_btn_reset().to_string(),
        project_label.to_string(),
        t.settings_btn_cancel().to_string(),
    ]
}

// ---------------------------------------------------------------------------
// SettingsModal
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SettingsModal
// ---------------------------------------------------------------------------

/// Full-screen settings modal with tabs, scrollable fields, and action buttons.
#[derive(Debug)]
pub struct SettingsModal {
    /// Working copy of config (mutated in-place; only saved on Apply).
    config: Config,

    // --- Tab state ---
    active_tab: SettingsTab,

    // --- Sidebar state ---
    /// Cursor index into `visible_sidebar_rows()`.
    sidebar_cursor: usize,
    /// Vertical scroll offset for the sidebar.
    sidebar_scroll: usize,
    /// Whether the Keybindings group is expanded.
    keybindings_expanded: bool,

    // --- Focus ---
    focus: FocusArea,
    /// Which field row is focused (within the current tab's content).
    field_cursor: usize,
    /// Vertical scroll offset for the content area.
    content_scroll: usize,

    // --- Editing ---
    /// True when a text/number field is being edited inline.
    editing: bool,
    /// Current edit buffer for text/number fields.
    edit_buffer: String,

    // --- LSP server management ---
    lsp_mode: LspMode,
    /// Index of the server being edited (None = adding new).
    lsp_edit_index: Option<usize>,
    /// Sorted server language names for stable indexing.
    lsp_server_keys: Vec<String>,
    /// Inline edit form for LSP server: [language, command, args, root_markers].
    lsp_edit_fields: [String; 4],
    /// Which field (0-3) is focused in the LSP edit form.
    lsp_edit_cursor: usize,

    // --- Keybindings tab ---
    kb_mode: KbMode,
    /// Which section (0-6) is selected.
    kb_section: usize,
    /// Cursor within the binding list of the current section.
    kb_cursor: usize,
    /// Scroll offset for binding list.
    kb_scroll: usize,
    /// Inline message shown after capturing a keybinding (e.g. conflict
    /// warning). Cleared on the next user action.
    kb_capture_message: Option<String>,

    // --- Buttons ---
    selected_button: usize,
    dirty: bool,
    /// Whether `<project>/.termide/config.toml` exists. Drives the
    /// "Create / Remove project override" button label and decides how
    /// the modal result handler routes the third-button click.
    project_override_active: bool,

    // --- Area caches (for mouse hit-testing) ---
    last_modal_area: Option<Rect>,
    last_sidebar_area: Option<Rect>,
    last_content_area: Option<Rect>,
    last_buttons_area: Option<Rect>,
}

// ---------------------------------------------------------------------------
// Modal trait implementation
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Modal trait implementation
// ---------------------------------------------------------------------------

impl Modal for SettingsModal {
    type Result = SettingsResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_rect = Self::calculate_size(area);
        self.last_modal_area = Some(modal_rect);

        // Clear and draw outer frame
        Clear.render(modal_rect, buf);
        let block = Block::default()
            .title(format!(" Settings{} ", if self.dirty { " *" } else { "" }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accented_fg))
            .style(Style::default().bg(theme.bg));
        let inner = block.inner(modal_rect);
        block.render(modal_rect, buf);

        // Inner layout: body (flex, horizontal sidebar | separator | content) | buttons (2 rows)
        if inner.height < 5 {
            return;
        }
        let body_height = inner.height.saturating_sub(2);
        let body = Rect::new(inner.x, inner.y, inner.width, body_height);
        let buttons = Rect::new(inner.x, inner.y + body_height, inner.width, 2);

        // Horizontal split inside body
        let sidebar_w = MODAL_SIDEBAR_WIDTH.min(inner.width.saturating_sub(10));
        let sidebar = Rect::new(body.x, body.y, sidebar_w, body.height);
        let sep_x = body.x + sidebar_w;
        let content = Rect::new(
            sep_x + 1,
            body.y,
            body.width.saturating_sub(sidebar_w + 1),
            body.height,
        );

        self.render_sidebar(sidebar, buf, theme);

        // Vertical separator between sidebar and content
        for y in body.y..body.y + body.height {
            buf[(sep_x, y)]
                .set_char('│')
                .set_style(Style::default().fg(theme.disabled));
        }

        self.render_content(content, buf, theme);
        self.render_buttons(buttons, buf, theme);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        let key = chord.raw;
        // If editing a text/number field, intercept all keys
        if self.editing {
            return self.handle_edit_key(key);
        }

        // Keybindings tab has its own key handling
        // Keybinding capture consumes the canonical form (see
        // `format_key_event`); navigation keys are identical in both forms.
        if self.active_tab == SettingsTab::Keybindings && self.focus == FocusArea::Content {
            return self.handle_keybindings_key(chord.canonical);
        }

        match self.focus {
            FocusArea::Sidebar => self.handle_sidebar_key(key),
            FocusArea::Content => self.handle_content_key(key),
            FocusArea::Buttons => self.handle_buttons_key(key),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        modal_area: Rect,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        if mouse.kind == MouseEventKind::ScrollUp {
            if self.focus == FocusArea::Content && self.content_scroll > 0 {
                self.content_scroll -= 1;
            } else if self.focus == FocusArea::Sidebar && self.sidebar_scroll > 0 {
                self.sidebar_scroll -= 1;
            }
            return Ok(None);
        }
        if mouse.kind == MouseEventKind::ScrollDown {
            if self.focus == FocusArea::Content {
                self.content_scroll += 1;
            } else if self.focus == FocusArea::Sidebar {
                self.sidebar_scroll += 1;
            }
            return Ok(None);
        }
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return Ok(None);
        }

        // Click outside modal → cancel
        let modal_rect = self.last_modal_area.unwrap_or(modal_area);
        if !modal_rect.contains((mouse.column, mouse.row).into()) {
            return Ok(Some(ModalResult::Cancelled));
        }

        // Click on sidebar
        if let Some(sidebar_area) = self.last_sidebar_area {
            if sidebar_area.contains((mouse.column, mouse.row).into()) {
                self.focus = FocusArea::Sidebar;
                let rel_y = mouse.row as usize - sidebar_area.y as usize;
                let idx = self.sidebar_scroll + rel_y;
                let rows = self.visible_sidebar_rows();
                if idx < rows.len() {
                    self.sidebar_cursor = idx;
                    self.activate_sidebar_row(rows[idx]);
                }
                return Ok(None);
            }
        }

        // Click on content area → focus and select row
        if let Some(content_area) = self.last_content_area {
            if content_area.contains((mouse.column, mouse.row).into()) {
                self.focus = FocusArea::Content;
                let rel_y = mouse.row as usize - content_area.y as usize;
                if self.active_tab == SettingsTab::Keybindings {
                    let idx = self.kb_scroll + rel_y;
                    let names = kb_binding_names(self.kb_section);
                    if idx < names.len() {
                        self.kb_cursor = idx;
                    }
                } else {
                    let idx = self.content_scroll + rel_y;
                    let rows = self.content_rows();
                    if idx < rows.len() && rows[idx].is_selectable() {
                        self.field_cursor = idx;
                    }
                }
                return Ok(None);
            }
        }

        // Click on buttons area — determine which button was clicked
        if let Some(btn_area) = self.last_buttons_area {
            if btn_area.contains((mouse.column, mouse.row).into()) {
                self.focus = FocusArea::Buttons;
                // Calculate button positions to determine which one was clicked
                let spacing = 4;
                let labels = button_labels(self.project_override_active);
                let total_label_len: usize = labels.iter().map(|l| l.len() + 4).sum::<usize>()
                    + spacing * (labels.len().saturating_sub(1));
                let mut x = btn_area.x as usize
                    + (btn_area.width as usize).saturating_sub(total_label_len) / 2;
                for (i, label) in labels.iter().enumerate() {
                    let btn_end = x + label.len() + 4; // "[ label ]"
                    if (mouse.column as usize) >= x && (mouse.column as usize) < btn_end {
                        self.selected_button = i;
                        return self.execute_selected_button();
                    }
                    x = btn_end + spacing;
                }
                return Ok(None);
            }
        }

        Ok(None)
    }
}
