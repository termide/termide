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
use unicode_width::UnicodeWidthStr;

use crate::{Modal, ModalResult};

mod connection;
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
    Ai,
    /// One AI connection, opened from the AI tab: not in the sidebar, it
    /// names the fields of the page that edits it.
    Connection,
    Keybindings,
}

/// Top-level leaf tabs in the sidebar (excluding the Keybindings group).
const TOP_LEVEL_TABS: [SettingsTab; 8] = [
    SettingsTab::General,
    SettingsTab::Editor,
    SettingsTab::FileManager,
    SettingsTab::Terminal,
    SettingsTab::Lsp,
    SettingsTab::Logging,
    SettingsTab::Vfs,
    SettingsTab::Ai,
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
            SettingsTab::Ai | SettingsTab::Connection => t.settings_tab_agent().to_string(),
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
    /// Keybindings subsection (index into `KB_SECTIONS`).
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
const BUTTON_PROJECT_OVERRIDE: usize = 1;
/// Reset sits next to Cancel, away from the two Apply buttons: it is the
/// other destructive end of the row, and putting it between them invited
/// wiping the config while reaching for "Apply to Project".
const BUTTON_RESET: usize = 2;
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
        project_label.to_string(),
        t.settings_btn_reset().to_string(),
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

    /// Whether the config differs from the shipped defaults, i.e. whether
    /// "Reset to Defaults" has anything to do. Cached because rendering must
    /// not serialise the whole config on every frame; refreshed wherever the
    /// config changes, through `mark_dirty`.
    pub(super) reset_available: bool,

    /// Open enum dropdown, if any: which field it belongs to, and where the
    /// highlight sits. `area` is filled in by the renderer so clicks can be
    /// mapped back to entries.
    pub(super) enum_picker: Option<EnumPicker>,

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

    // --- AI connections ---
    /// The connection page, while one is open from the AI tab.
    connection_edit: Option<connection::ConnectionEdit>,
    /// The connection whose models the app should fetch for the model
    /// dropdown, taken with [`SettingsModal::take_model_fetch_request`].
    model_fetch_request: Option<termide_config::Connection>,

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

    /// The open connection's models, fetched off-thread by the app and
    /// pushed in with [`SettingsModal::set_model_options`]; empty until they
    /// arrive (or when the endpoint cannot list them), when the model field
    /// falls back to typing an id.
    pub(super) model_options: Vec<String>,

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

/// Column span of each button, as `[ label ]` boxes centred in `area`.
///
/// Shared by the renderer and by hit-testing. They used to compute this
/// separately, both with `label.len()` — which counts *bytes* while the
/// renderer draws *characters*. On a localised build every label past the
/// first was offset by the difference, so clicking a button either did
/// nothing or hit its neighbour.
fn button_spans(area_x: u16, area_width: u16, labels: &[String]) -> Vec<(usize, usize)> {
    const SPACING: usize = 4;

    let box_widths: Vec<usize> = labels.iter().map(|l| l.width() + 4).collect();
    let total: usize = box_widths.iter().sum::<usize>() + SPACING * labels.len().saturating_sub(1);

    let mut x = area_x as usize + (area_width as usize).saturating_sub(total) / 2;
    let mut spans = Vec::with_capacity(labels.len());
    for width in box_widths {
        spans.push((x, x + width));
        x += width + SPACING;
    }
    spans
}

/// State of an open enum dropdown.
#[derive(Debug, Clone)]
pub(super) struct EnumPicker {
    /// Index into `fields_for_tab(active_tab)`.
    pub field_index: usize,
    pub cursor: usize,
    pub scroll: usize,
    /// Where it was last drawn; `None` until the first render.
    pub area: Option<Rect>,
}

/// How many entries an open dropdown shows before it scrolls.
pub(super) const ENUM_PICKER_MAX_VISIBLE: usize = 12;

impl Modal for SettingsModal {
    type Result = SettingsResult;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_rect = Self::calculate_size(area);
        self.last_modal_area = Some(modal_rect);

        // Clear and draw outer frame
        Clear.render(modal_rect, buf);
        let block = Block::default()
            .title(format!(
                " {}{} ",
                i18n::t().settings_title(),
                if self.dirty { " *" } else { "" }
            ))
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

        // Last, so the list sits above the form it belongs to.
        self.render_enum_picker(content, buf, theme);
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
        // An open dropdown is on top, so it gets first refusal on every event.
        if let Some(picker) = self.enum_picker.clone() {
            if let Some(rect) = picker.area {
                let inside = rect.contains((mouse.column, mouse.row).into());
                match mouse.kind {
                    MouseEventKind::ScrollUp if inside => {
                        self.move_enum_cursor_public(false);
                        return Ok(None);
                    }
                    MouseEventKind::ScrollDown if inside => {
                        self.move_enum_cursor_public(true);
                        return Ok(None);
                    }
                    MouseEventKind::Down(_) if inside => {
                        // -1 for the top border.
                        let row = mouse.row.saturating_sub(rect.y + 1) as usize;
                        let index = picker.scroll + row;
                        if let Some(p) = self.enum_picker.as_mut() {
                            p.cursor = index;
                        }
                        self.commit_enum_picker();
                        return Ok(None);
                    }
                    // A click anywhere else dismisses the list without
                    // choosing, the way a dropdown is expected to behave.
                    MouseEventKind::Down(_) => {
                        self.close_enum_picker();
                        return Ok(None);
                    }
                    _ => {}
                }
            }
        }

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
                        // The buttons row acts on the button under the click;
                        // the gap between them does nothing.
                        if rows[idx] == fields::ContentRow::ConnectionButtons {
                            let column = mouse.column as usize;
                            let spans =
                                connection::connection_button_spans(content_area.x as usize + 2);
                            let Some(button) = spans
                                .iter()
                                .position(|(start, end)| (*start..*end).contains(&column))
                            else {
                                return Ok(None);
                            };
                            if let Some(edit) = self.connection_edit.as_mut() {
                                edit.button = button;
                            }
                        }
                        // Clicking a control operates it, the way Enter does.
                        // Moving the cursor and leaving the switch alone looks
                        // like the click was ignored.
                        self.activate_current_row();
                    }
                }
                return Ok(None);
            }
        }

        // Click on buttons area — determine which button was clicked
        if let Some(btn_area) = self.last_buttons_area {
            if btn_area.contains((mouse.column, mouse.row).into()) {
                self.focus = FocusArea::Buttons;
                let labels = button_labels(self.project_override_active);
                for (i, (start, end)) in button_spans(btn_area.x, btn_area.width, &labels)
                    .into_iter()
                    .enumerate()
                {
                    let column = mouse.column as usize;
                    if column >= start && column < end {
                        self.selected_button = i;
                        return self.execute_selected_button();
                    }
                }
                return Ok(None);
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod button_layout_tests {
    use super::*;

    /// Renderer and hit-testing derived button positions separately, both from
    /// `label.len()` — bytes, where the renderer draws characters. On a
    /// localised build every button past the first sat some columns away from
    /// where clicks were expected, so "Сбросить" could not be clicked at all.
    #[test]
    fn spans_follow_display_width_not_byte_length() {
        let labels: Vec<String> = ["Применить", "Сбросить", "Отмена"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let spans = button_spans(0, 80, &labels);
        assert_eq!(spans.len(), labels.len());

        for (span, label) in spans.iter().zip(&labels) {
            assert_eq!(
                span.1 - span.0,
                label.width() + 4,
                "a box is the label plus \"[ \" and \" ]\""
            );
        }

        // Boxes keep their order and never overlap.
        for pair in spans.windows(2) {
            assert!(pair[0].1 <= pair[1].0);
        }
    }

    /// The centre of every button must hit that button.
    #[test]
    fn clicking_the_middle_of_a_button_selects_it() {
        let labels = button_labels(false);
        let spans = button_spans(3, 100, &labels);

        for (index, (start, end)) in spans.iter().enumerate() {
            let centre = (start + end) / 2;
            let hit = spans
                .iter()
                .position(|(s, e)| centre >= *s && centre < *e)
                .expect("the centre lands inside some button");
            assert_eq!(hit, index);
        }
    }
}

#[cfg(test)]
mod button_order_tests {
    use super::*;

    /// The constants index into `button_labels`, and nothing ties them to it.
    /// Reordering the row without moving a constant would fire the wrong
    /// action — reset where the user pressed Apply to Project.
    #[test]
    fn constants_match_the_rendered_order() {
        let labels = button_labels(false);
        let t = i18n::t();

        assert_eq!(labels[BUTTON_APPLY], t.settings_btn_apply());
        assert_eq!(
            labels[BUTTON_PROJECT_OVERRIDE],
            t.settings_btn_create_project_override()
        );
        assert_eq!(labels[BUTTON_RESET], t.settings_btn_reset());
        assert_eq!(labels[BUTTON_COUNT - 1], t.settings_btn_cancel());
    }

    /// Reset must stay next to Cancel, away from the two Apply buttons.
    #[test]
    fn reset_sits_last_before_cancel() {
        let labels = button_labels(false);
        let order: Vec<&str> = labels.iter().map(|l| l.as_str()).collect();
        let t = i18n::t();

        let position = |needle: &str| {
            order
                .iter()
                .position(|l| *l == needle)
                .expect("label present")
        };

        assert!(
            position(t.settings_btn_apply()) < position(t.settings_btn_create_project_override())
        );
        assert!(
            position(t.settings_btn_create_project_override()) < position(t.settings_btn_reset())
        );
        assert_eq!(
            position(t.settings_btn_reset()) + 1,
            position(t.settings_btn_cancel())
        );
    }

    #[test]
    fn the_project_label_follows_whether_an_override_exists() {
        let t = i18n::t();
        assert_eq!(
            button_labels(true)[BUTTON_PROJECT_OVERRIDE],
            t.settings_btn_remove_project_override()
        );
    }
}
