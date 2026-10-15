//! Inline find / replace bar — a panel-embeddable search form.
//!
//! Unlike a floating search modal, this widget renders *inside* a host panel's
//! own area (no modal frame) and coexists with the panel's body: the panel
//! keeps focus on its results while the bar is open and only routes keys to
//! the bar when the user moves focus into it (e.g. with `Tab`). Focus is
//! managed *only among the bar's own controls* — deciding whether the bar or
//! the panel body has focus is the host's responsibility.
//!
//! The widget is a thin, search-flavoured facade over the reusable
//! [`termide_ui::InputBar`]: it names the fields ([`FindField`]) and the
//! button/toggle row ([`Btn`]), maps the generic [`termide_ui::InputBarAction`]
//! to a [`FindBarAction`] the host acts on, and reads the field values and
//! toggle states back out.
//!
//! The host drives it like this:
//! - call [`FindBar::height`] to reserve rows out of the panel's `Rect`;
//! - call [`FindBar::render`] with `active = true` when the bar (not the body)
//!   currently holds focus;
//! - forward keys to [`FindBar::handle_key`] / clicks to
//!   [`FindBar::handle_mouse`] and act on the returned [`FindBarAction`];
//! - read [`FindBar::find_text`] / [`FindBar::replace_text`] / [`FindBar::mask_text`]
//!   / [`FindBar::use_regex`] / [`FindBar::case_sensitive`] to run the search.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{buffer::Buffer, layout::Rect};

use termide_core::ThemeColors;
use termide_theme::Theme;
use termide_ui::{Control, InputBar, InputBarAction};

/// An input field the bar can expose, in render order.
///
/// Variant names describe the field's *role*, not its on-screen label: hosts
/// override labels via [`FindBar::set_label`]. The file-manager content bar,
/// for instance, shows `Mask` as "Find:" (the glob) and `Find` as "Text:"
/// (the query), while the editor shows `Find` as "Find:".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindField {
    /// Glob mask (file-manager content search).
    Mask,
    /// The search query.
    Find,
    /// The replacement text.
    Replace,
}

impl FindField {
    fn label(self) -> &'static str {
        match self {
            FindField::Mask => "Mask: ",
            FindField::Find => "Find: ",
            FindField::Replace => "Repl: ",
        }
    }
}

/// A control on the buttons row. Action buttons confirm an operation; the
/// `Regex` / `Case` / `Hex` toggles flip search behavior. The host supplies
/// the complete, ordered button row (see [`FindBarConfig::buttons`]) —
/// including the toggles, in whatever position it wants them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Replace,
    ReplaceAll,
    Prev,
    Next,
    /// A "select all" checkbox (host-defined meaning, e.g. all files for
    /// replace).
    SelectAll,
    Regex,
    Case,
    /// Interpret the query as a hex byte sequence (binary viewer search).
    Hex,
}

impl Btn {
    fn is_toggle(self) -> bool {
        matches!(self, Btn::Regex | Btn::Case | Btn::Hex | Btn::SelectAll)
    }

    /// The generic control this button maps to in the [`InputBar`].
    fn control(self) -> Control {
        let label = match self {
            Btn::Replace => "Replace",
            Btn::ReplaceAll => "Replace all",
            Btn::Prev => "◄ Prev",
            Btn::Next => "Next ►",
            Btn::SelectAll => "Select all",
            Btn::Regex => ".*",
            Btn::Case => "Aa",
            Btn::Hex => "hex",
        };
        if self.is_toggle() {
            Control::Toggle {
                label: label.to_string(),
                on: false,
            }
        } else {
            Control::Button {
                label: label.to_string(),
            }
        }
    }
}

/// What the host should do in response to a key / click.
///
/// Anything that mutates the query (typing, or flipping a toggle) collapses to
/// [`FindBarAction::QueryChanged`] so the host can re-run the search uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindBarAction {
    /// A field was edited or a toggle flipped — re-run the search.
    QueryChanged,
    /// `Ctrl+R` — re-run the current query against the (possibly changed)
    /// content without editing the query.
    Refresh,
    /// Go to the next match.
    Next,
    /// Go to the previous match.
    Previous,
    /// Replace the current match.
    Replace,
    /// Replace every match.
    ReplaceAll,
    /// `Enter` on a field — the host decides what to do based on
    /// [`FindBar::focused_field`].
    Submit,
    /// Toggle the "select all" checkbox.
    SelectAll,
    /// Close the bar.
    Close,
}

/// Configuration for a [`FindBar`].
pub struct FindBarConfig {
    /// Fields to show, top to bottom.
    pub fields: Vec<FindField>,
    /// The complete, ordered button row — including the `Regex`/`Case` toggles
    /// and any `SelectAll` checkbox where the host wants them. May be empty for
    /// a bar with no buttons.
    pub buttons: Vec<Btn>,
}

/// Inline find / replace bar.
pub struct FindBar {
    /// Field roles in order, parallel to the inner bar's fields.
    fields: Vec<FindField>,
    /// Button roles in order, parallel to the inner bar's controls.
    buttons: Vec<Btn>,
    inner: InputBar,
    match_info: Option<(usize, usize)>,
    info_text: Option<String>,
}

impl FindBar {
    /// Create a bar from a config. The first field is focused by default.
    pub fn new(config: FindBarConfig) -> Self {
        let FindBarConfig { fields, buttons } = config;
        // The bar's name goes in the top border. A single-field bar drops its
        // inline label for a "› " prompt, since the border already names it;
        // a multi-field bar keeps per-field labels to tell them apart.
        let title = if fields.contains(&FindField::Replace) {
            "Replace"
        } else if fields.contains(&FindField::Mask) {
            "Search"
        } else {
            "Find"
        };
        let labels: Vec<String> = if fields.len() == 1 {
            vec![String::new()]
        } else {
            fields.iter().map(|f| f.label().to_string()).collect()
        };
        let mut inner = InputBar::new(labels).with_border(format!(" {title} "), String::new());
        for btn in &buttons {
            inner = inner.with_control(btn.control());
        }
        Self {
            fields,
            buttons,
            inner,
            match_info: None,
            info_text: None,
        }
    }

    fn field_index(&self, field: FindField) -> Option<usize> {
        self.fields.iter().position(|&f| f == field)
    }

    fn btn_index(&self, btn: Btn) -> Option<usize> {
        self.buttons.iter().position(|&b| b == btn)
    }

    // === Host-facing accessors ===

    /// Number of terminal rows the bar needs.
    pub fn height(&self) -> u16 {
        self.inner.height()
    }

    /// Move focus to the first field (host calls this when entering the bar).
    pub fn focus_first(&mut self) {
        self.inner.focus_first();
    }

    /// Focus a specific field, if the bar exposes it.
    pub fn focus_field(&mut self, field: FindField) {
        if let Some(i) = self.field_index(field) {
            self.inner.focus_field(i);
        }
    }

    /// Whether a click at `(col, row)` lands on any of the bar's controls.
    pub fn click_hits_bar(&self, col: u16, row: u16) -> bool {
        self.inner.click_hits(col, row)
    }

    /// Whether [`FindBar::handle_mouse`] would act on `mouse`: a press on the
    /// bar, a drag it owns (also past its edges, extending a selection in its
    /// field) or the release that ends one.
    pub fn mouse_hits(&self, mouse: MouseEvent) -> bool {
        self.inner.mouse_hits(mouse)
    }

    /// Whether the bar exposes `field`.
    pub fn has_field(&self, field: FindField) -> bool {
        self.fields.contains(&field)
    }

    /// Override the display label of a field (include the trailing space, e.g.
    /// `"Find: "`). No-op if the bar doesn't expose the field.
    pub fn set_label(&mut self, field: FindField, label: impl Into<String>) {
        if let Some(i) = self.field_index(field) {
            self.inner.set_label(i, label);
        }
    }

    /// Override an action button's label (e.g. `ReplaceAll` → "Replace").
    pub fn set_button_label(&mut self, btn: Btn, label: impl Into<String>) {
        if let Some(i) = self.btn_index(btn) {
            self.inner.set_control_label(i, label);
        }
    }

    /// The field that currently has focus, if any (vs a button).
    pub fn focused_field(&self) -> Option<FindField> {
        self.inner
            .focused_field()
            .and_then(|i| self.fields.get(i).copied())
    }

    fn text_of(&self, field: FindField) -> &str {
        self.field_index(field)
            .map_or("", |i| self.inner.field_text(i))
    }

    /// Current query text.
    pub fn find_text(&self) -> &str {
        self.text_of(FindField::Find)
    }

    /// Current replacement text (empty if there is no replace field).
    pub fn replace_text(&self) -> &str {
        self.text_of(FindField::Replace)
    }

    /// Current glob mask (empty if there is no mask field).
    pub fn mask_text(&self) -> &str {
        self.text_of(FindField::Mask)
    }

    fn toggle_on(&self, btn: Btn) -> bool {
        self.btn_index(btn)
            .is_some_and(|i| self.inner.control_on(i))
    }

    /// Whether regex matching is enabled.
    pub fn use_regex(&self) -> bool {
        self.toggle_on(Btn::Regex)
    }

    /// Whether matching is case-sensitive.
    pub fn case_sensitive(&self) -> bool {
        self.toggle_on(Btn::Case)
    }

    /// Whether the `[hex]` toggle is on (query is a hex byte sequence).
    pub fn hex_mode(&self) -> bool {
        self.toggle_on(Btn::Hex)
    }

    /// Seed a field's text (e.g. restoring the previous query).
    pub fn set_text(&mut self, field: FindField, text: String) {
        if let Some(i) = self.field_index(field) {
            self.inner.set_field_text(i, text);
        }
    }

    /// Update the "N of M" counter.
    pub fn set_match_info(&mut self, current: usize, total: usize) {
        self.match_info = Some((current, total));
        self.sync_status();
    }

    /// Clear the match counter.
    pub fn clear_match_info(&mut self) {
        self.match_info = None;
        self.sync_status();
    }

    /// Set (or clear) the free-form status text shown in place of the counter.
    pub fn set_info_text(&mut self, text: Option<String>) {
        self.info_text = text;
        self.sync_status();
    }

    /// Set the state of the "select all" checkbox button.
    pub fn set_select_all(&mut self, on: bool) {
        if let Some(i) = self.btn_index(Btn::SelectAll) {
            self.inner.set_control_on(i, on);
        }
    }

    /// Push the current counter / info text into the inner bar's status slot.
    /// An explicit info string wins over the "N of M" counter.
    fn sync_status(&mut self) {
        let status = self.info_text.clone().or_else(|| {
            self.match_info
                .map(|(cur, total)| format!("{cur} of {total}"))
        });
        self.inner.set_status(status);
    }

    // === Input ===

    /// Handle a key while the bar holds focus. Returns the host action, if any.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<FindBarAction> {
        // Ctrl+R re-runs the search regardless of the focused control, on any
        // keyboard layout (`Ctrl+К` too); the typed query keeps the raw key.
        let shortcut = termide_core::KeyNormalizer::default().canonicalize(key);
        if shortcut.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(shortcut.code, KeyCode::Char('r') | KeyCode::Char('R'))
        {
            return Some(FindBarAction::Refresh);
        }
        self.inner.handle_key(key).map(|action| self.map(action))
    }

    /// Handle a mouse click. Clicking a field focuses it and positions the
    /// cursor; clicking a control activates it.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<FindBarAction> {
        self.inner
            .handle_mouse(mouse)
            .map(|action| self.map(action))
    }

    // === Clipboard ===
    //
    // The app routes Ctrl+C/X/V and a terminal's bracketed paste to the panel
    // as commands before the bar ever sees the key, so a host whose bar holds
    // focus forwards them here. An edit reports `QueryChanged`, as typing does.

    /// Paste `text` (a bracketed paste, or the clipboard's text) into the
    /// focused field; line breaks become spaces. `None` when a button holds
    /// the focus or nothing changed.
    pub fn paste(&mut self, text: &str) -> Option<FindBarAction> {
        self.inner
            .paste(text)
            .then_some(FindBarAction::QueryChanged)
    }

    /// Paste the system clipboard's text (or copied files' paths) into the
    /// focused field — `Ctrl+V`.
    pub fn paste_clipboard(&mut self) -> Option<FindBarAction> {
        let text = termide_ui::clipboard::paste_text_or_paths()?;
        self.paste(&text)
    }

    /// The focused field's selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.inner.selected_text()
    }

    /// Copy the focused field's selection to the clipboard — `Ctrl+C`.
    /// Whether there was one to copy.
    pub fn copy_selection(&self) -> bool {
        match self.selected_text() {
            Some(text) => {
                let _ = termide_ui::clipboard::copy(&text);
                true
            }
            None => false,
        }
    }

    /// Move the focused field's selection to the clipboard — `Ctrl+X`. `None`
    /// when nothing is selected.
    pub fn cut_selection(&mut self) -> Option<FindBarAction> {
        let text = self.inner.cut_selection()?;
        let _ = termide_ui::clipboard::copy(&text);
        Some(FindBarAction::QueryChanged)
    }

    /// Map a generic [`InputBarAction`] to the search-specific action.
    fn map(&self, action: InputBarAction) -> FindBarAction {
        match action {
            InputBarAction::Close => FindBarAction::Close,
            InputBarAction::Submit(_) => FindBarAction::Submit,
            InputBarAction::Edited(_) => FindBarAction::QueryChanged,
            InputBarAction::Activated(i) => match self.buttons.get(i).copied() {
                Some(Btn::Replace) => FindBarAction::Replace,
                Some(Btn::ReplaceAll) => FindBarAction::ReplaceAll,
                Some(Btn::Prev) => FindBarAction::Previous,
                Some(Btn::Next) => FindBarAction::Next,
                Some(Btn::SelectAll) => FindBarAction::SelectAll,
                // A toggle already flipped its own state; the host re-runs.
                Some(Btn::Regex | Btn::Case | Btn::Hex) | None => FindBarAction::QueryChanged,
            },
        }
    }

    // === Rendering ===

    /// Render the bar into `area`. `active` is whether the bar (rather than the
    /// panel body) currently holds focus — it controls cursor/highlight display.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, active: bool) {
        let colors = ThemeColors::from(theme);
        self.inner.render(area, buf, &colors, active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn content_bar() -> FindBar {
        FindBar::new(FindBarConfig {
            fields: vec![FindField::Mask, FindField::Find, FindField::Replace],
            buttons: vec![Btn::Case, Btn::Regex, Btn::Prev, Btn::Next, Btn::ReplaceAll],
        })
    }

    #[test]
    fn ctrl_r_refreshes_on_a_cyrillic_layout_and_letters_type_as_they_are() {
        let mut bar = content_bar();
        bar.focus_first();
        // `Ctrl+К` is where `Ctrl+R` sits on a Russian layout.
        let refresh = KeyEvent::new(KeyCode::Char('к'), KeyModifiers::CONTROL);
        assert_eq!(bar.handle_key(refresh), Some(FindBarAction::Refresh));
        // Without Ctrl the letter is typed into the query.
        assert_eq!(
            bar.handle_key(key(KeyCode::Char('к'))),
            Some(FindBarAction::QueryChanged)
        );
    }

    /// Move focus onto the button at `btn_idx` (fields lead the ring).
    fn focus_button(bar: &mut FindBar, btn_idx: usize) {
        bar.focus_first();
        for _ in 0..bar.fields.len() + btn_idx {
            bar.handle_key(key(KeyCode::Tab));
        }
    }

    #[test]
    fn ctrl_r_requests_refresh_from_any_control() {
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let mut bar = content_bar();
        assert_eq!(bar.handle_key(ctrl_r), Some(FindBarAction::Refresh));
        focus_button(&mut bar, 0);
        assert_eq!(bar.handle_key(ctrl_r), Some(FindBarAction::Refresh));
    }

    #[test]
    fn height_is_fields_plus_button_row() {
        // border + 3 fields + internal separator + controls row
        assert_eq!(content_bar().height(), 6);
        let find_only = FindBar::new(FindBarConfig {
            fields: vec![FindField::Find],
            buttons: vec![Btn::Case, Btn::Regex, Btn::Prev, Btn::Next],
        });
        // border + 1 field + separator + controls row
        assert_eq!(find_only.height(), 4);
        let name_only = FindBar::new(FindBarConfig {
            fields: vec![FindField::Find],
            buttons: vec![],
        });
        // border + 1 field
        assert_eq!(name_only.height(), 2);
    }

    #[test]
    fn typing_in_first_field_edits_it_and_reports_change() {
        let mut bar = content_bar();
        assert_eq!(bar.focused_field(), Some(FindField::Mask));
        assert_eq!(
            bar.handle_key(key(KeyCode::Char('*'))),
            Some(FindBarAction::QueryChanged)
        );
        assert_eq!(bar.mask_text(), "*");
        assert_eq!(bar.find_text(), "");
    }

    #[test]
    fn tab_walks_fields_then_buttons_and_wraps() {
        let mut bar = content_bar();
        bar.handle_key(key(KeyCode::Tab));
        assert_eq!(bar.focused_field(), Some(FindField::Find));
        bar.handle_key(key(KeyCode::Tab));
        assert_eq!(bar.focused_field(), Some(FindField::Replace));
        bar.handle_key(key(KeyCode::Tab));
        assert_eq!(bar.focused_field(), None);
        for _ in 0..5 {
            bar.handle_key(key(KeyCode::Tab));
        }
        assert_eq!(bar.focused_field(), Some(FindField::Mask));
    }

    #[test]
    fn activating_buttons_yields_actions() {
        let mut bar = content_bar();
        focus_button(&mut bar, 2); // Prev
        assert_eq!(
            bar.handle_key(key(KeyCode::Enter)),
            Some(FindBarAction::Previous)
        );
        focus_button(&mut bar, 3); // Next
        assert_eq!(
            bar.handle_key(key(KeyCode::Char(' '))),
            Some(FindBarAction::Next)
        );
        focus_button(&mut bar, 4); // ReplaceAll
        assert_eq!(
            bar.handle_key(key(KeyCode::Enter)),
            Some(FindBarAction::ReplaceAll)
        );
    }

    #[test]
    fn toggles_flip_state_and_report_query_change() {
        let mut bar = content_bar();
        focus_button(&mut bar, 1); // Regex toggle
        assert!(!bar.use_regex());
        assert_eq!(
            bar.handle_key(key(KeyCode::Enter)),
            Some(FindBarAction::QueryChanged)
        );
        assert!(bar.use_regex());
        focus_button(&mut bar, 0); // Case toggle
        assert!(!bar.case_sensitive());
        assert_eq!(
            bar.handle_key(key(KeyCode::Char(' '))),
            Some(FindBarAction::QueryChanged)
        );
        assert!(bar.case_sensitive());
    }

    #[test]
    fn enter_on_a_field_submits() {
        let mut bar = content_bar();
        bar.handle_key(key(KeyCode::Tab)); // -> Find
        assert_eq!(
            bar.handle_key(key(KeyCode::Enter)),
            Some(FindBarAction::Submit)
        );
        assert_eq!(bar.focused_field(), Some(FindField::Find));
    }

    #[test]
    fn esc_closes_from_anywhere() {
        let mut bar = content_bar();
        assert_eq!(
            bar.handle_key(key(KeyCode::Esc)),
            Some(FindBarAction::Close)
        );
        focus_button(&mut bar, 0);
        assert_eq!(
            bar.handle_key(key(KeyCode::Esc)),
            Some(FindBarAction::Close)
        );
    }

    #[test]
    fn seed_and_read_back_text() {
        let mut bar = content_bar();
        bar.set_text(FindField::Find, "needle".into());
        bar.set_text(FindField::Replace, "thread".into());
        assert_eq!(bar.find_text(), "needle");
        assert_eq!(bar.replace_text(), "thread");
    }

    #[test]
    fn clipboard_commands_edit_the_focused_field_and_report_a_change() {
        termide_ui::clipboard::use_fake_clipboard("from clipboard");
        let mut bar = content_bar();
        bar.focus_field(FindField::Find);
        assert_eq!(bar.paste("a\nb"), Some(FindBarAction::QueryChanged));
        assert_eq!(bar.find_text(), "a b");
        assert!(!bar.copy_selection(), "nothing selected");
        assert_eq!(bar.cut_selection(), None);
        bar.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(bar.copy_selection());
        assert_eq!(termide_ui::clipboard::paste().as_deref(), Some("a b"));
        termide_ui::clipboard::use_fake_clipboard("new");
        assert_eq!(bar.paste_clipboard(), Some(FindBarAction::QueryChanged));
        assert_eq!(bar.find_text(), "new", "the selection was replaced");
        bar.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(bar.cut_selection(), Some(FindBarAction::QueryChanged));
        assert_eq!(bar.find_text(), "");
        assert_eq!(bar.mask_text(), "", "other fields are untouched");
        focus_button(&mut bar, 0);
        assert_eq!(bar.paste("x"), None, "a button takes no paste");
    }

    #[test]
    fn up_from_buttons_returns_to_last_field() {
        let mut bar = content_bar();
        focus_button(&mut bar, 0);
        bar.handle_key(key(KeyCode::Up));
        assert_eq!(bar.focused_field(), Some(FindField::Replace));
    }
}
