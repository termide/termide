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

/// One slot drawn in a binding row's value column: a bound key, or the `+`
/// slot that adds another alternative.
///
/// `key` is the index into the binding's key list; `None` marks the `+` slot,
/// which is not a key yet and so has no index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KbSlot {
    pub key: Option<usize>,
    /// Half-open column span `[start, end)`, shared by the renderer and the
    /// click hit-test so the two cannot disagree about where a key is.
    pub span: (u16, u16),
}

/// The slots drawn in one binding row, in the order they are drawn.
pub(super) type KbRow = Vec<KbSlot>;

/// Keybindings tab sub-mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KbMode {
    /// Browsing bindings for the active section — user picks one to rebind.
    Bindings,
    /// Capturing a keypress; where it lands is what the cursor was on.
    Capturing(KbCapture),
}

/// What a captured keypress writes to.
///
/// An action can hold several alternatives, so capture has to know which one
/// it is rewriting: `Enter` on a key replaces that key, `Enter` on the `+`
/// slot at the end of the row adds a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KbCapture {
    /// Overwrite the alternative at this index into the binding's key list.
    Replace(usize),
    /// Append after every alternative the action already has.
    Append,
}

/// Result returned when the settings modal closes. Settings save as they
/// change (see [`SettingsModal::take_save_request`]), so closing carries
/// nothing: the modal only ever ends with `ModalResult::Cancelled`.
#[derive(Debug)]
pub enum SettingsResult {}

/// A change to persist, taken by the app with
/// [`SettingsModal::take_save_request`].
#[derive(Debug, Clone)]
pub struct SettingsSave {
    /// The whole effective config.
    pub config: Config,
    /// Where it is stored. `true`: as a diff against the global layer in
    /// `<project>/.termide/config.toml`. `false`: in the global file, and
    /// the project file, if any, is deleted.
    pub in_project: bool,
}

/// A destructive action waiting for confirmation in the button row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    /// Reset where the settings are stored: the project back to the global
    /// settings, the global settings back to the defaults.
    Reset,
    /// Switch storage from the project to the global file.
    LeaveProject,
}

/// Bottom buttons in normal mode. The storage switch comes first, Reset sits
/// next to Close at the other end.
const BUTTON_STORAGE: usize = 0;
const BUTTON_RESET: usize = 1;
const BUTTON_CLOSE: usize = 2;

/// Buttons while [`Confirm::Reset`] is asked.
const RESET_CONFIRM: usize = 0;
/// Buttons while [`Confirm::LeaveProject`] is asked; the last one is Cancel
/// in both.
const LEAVE_DISCARD: usize = 0;
const LEAVE_MOVE: usize = 1;

// ---------------------------------------------------------------------------
// SettingsModal
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SettingsModal
// ---------------------------------------------------------------------------

/// Full-screen settings modal with tabs, scrollable fields, and action buttons.
#[derive(Debug)]
pub struct SettingsModal {
    /// The effective config, as last saved: every change is saved at once.
    config: Config,
    /// Defaults + global file: what a project resets to, and what the
    /// project file is diffed against.
    global_layer: Config,

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
    /// The text/number field being edited: the shared single-line input, so
    /// selection, word moves, the clipboard and undo work as everywhere else.
    edit_input: termide_ui::TextInput,
    /// Where the edited value was last drawn, for clicks and drags on it.
    edit_area: Option<Rect>,
    /// A press on a text field is being dragged: the drag selects in it.
    field_drag: bool,
    /// Presses by row, so a double click on a field selects its text.
    clicks: termide_ui::ClickTracker<u16>,

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
    lsp_edit_fields: [termide_ui::TextInput; 4],
    /// Which field (0-3) is focused in the LSP edit form.
    lsp_edit_cursor: usize,
    /// Where each LSP form field was last drawn, for clicks and drags.
    lsp_field_areas: Vec<Rect>,

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
    /// Cursor within the key list of the row under `kb_cursor`. `keys().len()`
    /// means the `+` slot at the end of the row, where a new alternative is
    /// added; every smaller index is one of the keys already bound.
    kb_key_cursor: usize,
    /// Scroll offset for binding list.
    kb_scroll: usize,
    /// The slots drawn on each visible row, keyed by the row's index in
    /// `kb_binding_names(kb_section)`. A slot whose `key` is `None` is the
    /// `+` at the end of the row. Keyed by row index rather than screen row
    /// so a click resolves against the row it was drawn on even if the list
    /// scrolled in between. Filled by `render_keybindings`.
    kb_rows: Vec<(usize, KbRow)>,
    /// Double-click tracking for the Keybindings list, keyed by row and the
    /// column its slot starts at. Separate from `clicks`, which tracks text
    /// fields by row: two quick clicks on *different* keys of one row are not
    /// a double click on either.
    kb_clicks: termide_ui::ClickTracker<(u16, u16)>,
    /// Inline message shown after capturing a keybinding (e.g. conflict
    /// warning). Cleared on the next user action.
    kb_capture_message: Option<String>,

    // --- Buttons ---
    selected_button: usize,
    /// Whether the settings are stored in `<project>/.termide/config.toml`
    /// rather than the global file.
    in_project: bool,
    /// The config changed and the app has not taken it yet.
    save_pending: bool,
    /// A destructive action waiting for a yes in the button row.
    confirm: Option<Confirm>,

    /// The open connection's models, fetched off-thread by the app and
    /// pushed in with [`SettingsModal::set_model_options`]; empty until they
    /// arrive (or when the endpoint cannot list them), when the model field
    /// falls back to typing an id.
    pub(super) model_options: Vec<String>,
    /// Whether the open connection's models are on their way: asked for and
    /// not yet in, while the model field shows a spinner.
    pub(super) models_loading: bool,

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
            .title(format!(" {} ", i18n::t().settings_title()))
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

        // Outside a field being edited nothing is typed, so the zones match
        // the layout-normalized key: `Ctrl+Ы` applies as `Ctrl+S` does.
        let key = chord.canonical;
        match self.focus {
            FocusArea::Sidebar => self.handle_sidebar_key(key),
            FocusArea::Content => self.handle_content_key(key),
            FocusArea::Buttons => self.handle_buttons_key(key),
        }
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        self.paste_into_edit(text)
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

        if self.handle_field_mouse(mouse) {
            return Ok(None);
        }

        // The wheel scrolls the content list under the pointer. The sidebar
        // is left out: moving its cursor would switch sections.
        if let MouseEventKind::ScrollUp | MouseEventKind::ScrollDown = mouse.kind {
            let over_content = self
                .last_content_area
                .is_some_and(|area| area.contains((mouse.column, mouse.row).into()));
            if over_content {
                self.wheel_content(mouse.kind == MouseEventKind::ScrollDown);
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
                    self.kb_click(idx, mouse.column);
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
                let labels = self.button_labels();
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
        let labels =
            SettingsModal::new(Config::default(), Config::default(), false).button_labels();
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
