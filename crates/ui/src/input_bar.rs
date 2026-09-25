//! A reusable bottom input bar: labeled text fields with a cursor, an
//! optional row of buttons and toggles, an optional right-aligned status, and
//! an optional titled top border with a left and a right slot.
//!
//! It is the shared engine behind the panels' bottom bars — the agent's
//! prompt input and the find/replace bar — so they share one focus model, one
//! field renderer and one set of key and mouse semantics. Like
//! [`crate::CompletionList`] and [`crate::ChoiceForm`], the widget owns its
//! layout, focus and keys and reports what happened through
//! [`InputBarAction`]; the host wires the meaning (run a search, send a
//! prompt) and reads the field values back.
//!
//! Focus runs as a ring: every field first, then every control. `Tab` and the
//! arrows move within it, `Enter` on a field submits, `Enter`/`Space` on a
//! control activates it, `Esc` closes.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use termide_core::ThemeColors;

use crate::field_edit::{edit_text_area, edit_text_input, FieldEdit};
use crate::grapheme_utils::str_display_width;
use crate::{TextArea, TextInput};

/// A control on the bar's bottom row: a push button or a toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// A button that reports [`InputBarAction::Activated`] when pressed.
    Button { label: String },
    /// A checkbox-style toggle. Activation flips `on` and still reports
    /// [`InputBarAction::Activated`], so the host can react to the new state.
    Toggle { label: String, on: bool },
}

/// One field's editor: a single-line input, or a multi-line text area (for a
/// prompt box). The host reads and drives a multi-line field through
/// [`InputBar::multiline`] / [`InputBar::multiline_mut`].
enum FieldInput {
    Line(TextInput),
    Multi(TextArea),
}

impl FieldInput {
    fn text(&self) -> String {
        match self {
            FieldInput::Line(input) => input.text().to_string(),
            FieldInput::Multi(area) => area.text(),
        }
    }

    /// Visual rows the field needs: one for a line, its line count for an area.
    fn rows(&self) -> u16 {
        match self {
            FieldInput::Line(_) => 1,
            FieldInput::Multi(area) => (area.line_count().max(1)) as u16,
        }
    }
}

/// The left and right text embedded in the bar's top border.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BorderSlots {
    pub left: String,
    pub right: String,
}

/// What a key or click did, for the host to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputBarAction {
    /// Field `index`'s text changed.
    Edited(usize),
    /// `Enter` on field `index`.
    Submit(usize),
    /// A control was activated (a toggle has already flipped its own state).
    Activated(usize),
    /// `Esc`.
    Close,
}

/// A focusable control: a field, or a bottom-row control by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Field(usize),
    Control(usize),
}

/// A bottom input bar. Build it with [`InputBar::new`] and the `with_*`
/// methods, then drive it with [`InputBar::render`], [`InputBar::handle_key`]
/// and [`InputBar::handle_mouse`].
pub struct InputBar {
    labels: Vec<String>,
    fields: Vec<FieldInput>,
    controls: Vec<Control>,
    /// Index into the focus ring (fields then controls).
    focus: usize,
    /// Right-aligned status on the controls row (a match counter, say).
    status: Option<String>,
    /// The top border and its slots, or `None` for a borderless bar.
    border: Option<BorderSlots>,
    /// Clickable labels drawn at the right end of the top border (a run's
    /// pause and stop, say), each with its own style.
    border_buttons: Vec<(String, Style)>,
    /// Rendered border-button areas, parallel to `border_buttons`.
    border_button_areas: Vec<Rect>,
    /// Placeholder shown in an empty multi-line field while the bar is idle.
    placeholder: Option<String>,
    /// Rendered field areas, parallel to `fields`, for mouse hit-testing; a
    /// multi-line field's area spans all of its rows.
    field_areas: Vec<Rect>,
    /// Rendered control areas: (area, index into `controls`).
    control_areas: Vec<(Rect, usize)>,
    /// The field a left button is currently held down on, so a drag extends
    /// that field's selection and nothing else's.
    drag_field: Option<usize>,
    /// Whether a left button is held down anywhere on the bar, including over a
    /// control, whose press owns the drag until the button is released.
    pressed: bool,
}

impl InputBar {
    /// A bar with one labeled field per entry in `labels` and no controls.
    /// Include any trailing space in a label, e.g. `"Find: "`.
    #[must_use]
    pub fn new(labels: Vec<String>) -> Self {
        let fields = labels
            .iter()
            .map(|_| FieldInput::Line(TextInput::new()))
            .collect();
        Self {
            labels,
            fields,
            controls: Vec::new(),
            focus: 0,
            status: None,
            border: None,
            border_buttons: Vec::new(),
            border_button_areas: Vec::new(),
            placeholder: None,
            field_areas: Vec::new(),
            control_areas: Vec::new(),
            drag_field: None,
            pressed: false,
        }
    }

    /// Append a control to the bottom row.
    #[must_use]
    pub fn with_control(mut self, control: Control) -> Self {
        self.controls.push(control);
        self
    }

    /// Append a multi-line field (a prompt box) with `label`. Drive it through
    /// [`InputBar::multiline_mut`].
    #[must_use]
    pub fn with_multiline_field(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self.fields.push(FieldInput::Multi(TextArea::new()));
        self
    }

    /// Give the bar a top border with a left and a right slot.
    #[must_use]
    pub fn with_border(mut self, left: impl Into<String>, right: impl Into<String>) -> Self {
        self.border = Some(BorderSlots {
            left: left.into(),
            right: right.into(),
        });
        self
    }

    /// Placeholder shown in an empty multi-line field while the bar is idle.
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn set_placeholder(&mut self, placeholder: Option<String>) {
        self.placeholder = placeholder;
    }

    // === Structure ===

    #[must_use]
    pub fn field_count(&self) -> usize {
        self.labels.len()
    }

    /// The focus ring: field indices first, then control indices.
    fn ring(&self) -> Vec<Focus> {
        let mut ring: Vec<Focus> = (0..self.labels.len()).map(Focus::Field).collect();
        ring.extend((0..self.controls.len()).map(Focus::Control));
        ring
    }

    /// The control the focus is on.
    #[must_use]
    pub fn focus(&self) -> Focus {
        let ring = self.ring();
        ring[self.focus.min(ring.len().saturating_sub(1))]
    }

    /// The focused field's index, or `None` when a control is focused.
    #[must_use]
    pub fn focused_field(&self) -> Option<usize> {
        match self.focus() {
            Focus::Field(i) => Some(i),
            Focus::Control(_) => None,
        }
    }

    /// Rows the bar occupies: the border, one row per field, and the controls
    /// row when there are controls.
    #[must_use]
    pub fn height(&self) -> u16 {
        // Border row, each field's rows (a multi-line field takes several),
        // then the controls row — preceded by a separator row when bordered.
        let fields_rows: u16 = self.fields.iter().map(FieldInput::rows).sum();
        let controls_rows = if self.controls.is_empty() {
            0
        } else {
            1 + u16::from(self.border.is_some())
        };
        u16::from(self.border.is_some()) + fields_rows + controls_rows
    }

    // === Values ===

    /// A single-line field's text. A multi-line field returns `""`; read it
    /// with [`InputBar::field_value`] or [`InputBar::multiline`] instead.
    #[must_use]
    pub fn field_text(&self, index: usize) -> &str {
        match self.fields.get(index) {
            Some(FieldInput::Line(input)) => input.text(),
            _ => "",
        }
    }

    /// The text of any field (single- or multi-line).
    #[must_use]
    pub fn field_value(&self, index: usize) -> String {
        self.fields
            .get(index)
            .map(FieldInput::text)
            .unwrap_or_default()
    }

    /// A multi-line field's text area, for the host to render/drive directly.
    #[must_use]
    pub fn multiline(&self, index: usize) -> Option<&TextArea> {
        match self.fields.get(index) {
            Some(FieldInput::Multi(area)) => Some(area),
            _ => None,
        }
    }

    pub fn multiline_mut(&mut self, index: usize) -> Option<&mut TextArea> {
        match self.fields.get_mut(index) {
            Some(FieldInput::Multi(area)) => Some(area),
            _ => None,
        }
    }

    pub fn set_field_text(&mut self, index: usize, text: impl Into<String>) {
        match self.fields.get_mut(index) {
            Some(FieldInput::Line(input)) => *input = TextInput::with_default(text.into()),
            Some(FieldInput::Multi(area)) => *area = TextArea::with_text(&text.into()),
            None => {}
        }
    }

    /// Whether the control at `index` is a toggle that is on.
    #[must_use]
    pub fn control_on(&self, index: usize) -> bool {
        matches!(
            self.controls.get(index),
            Some(Control::Toggle { on: true, .. })
        )
    }

    pub fn set_control_on(&mut self, index: usize, value: bool) {
        if let Some(Control::Toggle { on, .. }) = self.controls.get_mut(index) {
            *on = value;
        }
    }

    pub fn set_control_label(&mut self, index: usize, label: impl Into<String>) {
        match self.controls.get_mut(index) {
            Some(Control::Button { label: l } | Control::Toggle { label: l, .. }) => {
                *l = label.into();
            }
            None => {}
        }
    }

    pub fn set_label(&mut self, index: usize, label: impl Into<String>) {
        if let Some(l) = self.labels.get_mut(index) {
            *l = label.into();
        }
    }

    pub fn set_status(&mut self, status: Option<String>) {
        self.status = status;
    }

    /// Set the clickable labels at the right end of the top border, in order;
    /// [`InputBar::border_button_at`] tells which one a click lands on. An
    /// empty list removes them.
    pub fn set_border_buttons(&mut self, buttons: Vec<(String, Style)>) {
        self.border_buttons = buttons;
    }

    /// The border button under (`col`, `row`), as an index into the list
    /// given to [`InputBar::set_border_buttons`].
    #[must_use]
    pub fn border_button_at(&self, col: u16, row: u16) -> Option<usize> {
        self.border_button_areas
            .iter()
            .position(|area| hit(*area, col, row))
    }

    pub fn set_border_slots(&mut self, left: impl Into<String>, right: impl Into<String>) {
        self.border = Some(BorderSlots {
            left: left.into(),
            right: right.into(),
        });
    }

    // === Focus ===

    pub fn focus_first(&mut self) {
        self.focus = 0;
    }

    pub fn focus_field(&mut self, index: usize) {
        if index < self.labels.len() {
            self.focus = index;
        }
    }

    fn focus_next(&mut self) {
        let len = self.ring().len();
        if len > 0 {
            self.focus = (self.focus + 1) % len;
        }
    }

    fn focus_prev(&mut self) {
        let len = self.ring().len();
        if len > 0 {
            self.focus = (self.focus + len - 1) % len;
        }
    }

    // === Input ===

    /// Handle a key while the bar holds focus. `Esc` closes; the rest depends
    /// on whether a field or a control is focused.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<InputBarAction> {
        if key.code == KeyCode::Esc {
            return Some(InputBarAction::Close);
        }
        match self.focus() {
            Focus::Field(i) => self.handle_field_key(i, key),
            Focus::Control(i) => self.handle_control_key(i, key),
        }
    }

    fn handle_field_key(&mut self, index: usize, key: KeyEvent) -> Option<InputBarAction> {
        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                self.focus_next();
                None
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus_prev();
                None
            }
            KeyCode::Enter => Some(InputBarAction::Submit(index)),
            _ => match edit_field_input(&mut self.fields[index], key) {
                FieldEdit::Edited => Some(InputBarAction::Edited(index)),
                FieldEdit::Navigated | FieldEdit::NotHandled => None,
            },
        }
    }

    /// Drive one field's editing keys directly, bypassing the focus ring and
    /// the `Enter`/`Tab` keys that belong to the bar. Hosts that own their key
    /// routing — the agent's prompt box, where `Enter` sends — use this so
    /// every field in the app shares one grammar of typing, navigation,
    /// selection, clipboard and undo.
    pub fn edit_field(&mut self, index: usize, key: KeyEvent) -> FieldEdit {
        match self.fields.get_mut(index) {
            Some(field) => edit_field_input(field, key),
            None => FieldEdit::NotHandled,
        }
    }

    fn handle_control_key(&mut self, index: usize, key: KeyEvent) -> Option<InputBarAction> {
        match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                self.focus_prev();
                None
            }
            KeyCode::Right | KeyCode::Tab => {
                self.focus_next();
                None
            }
            KeyCode::Up => {
                // Jump back to the last field, if there is one.
                if !self.labels.is_empty() {
                    self.focus = self.labels.len() - 1;
                }
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => Some(self.activate(index)),
            _ => None,
        }
    }

    fn activate(&mut self, index: usize) -> InputBarAction {
        if let Some(Control::Toggle { on, .. }) = self.controls.get_mut(index) {
            *on = !*on;
        }
        InputBarAction::Activated(index)
    }

    /// Handle a left press, drag or release: on a field a press focuses it and
    /// places the cursor, starting the selection that a drag then extends; on a
    /// control a press activates it. A drag only moves the selection when the
    /// press that owns it landed on the same field, so a drag begun elsewhere —
    /// in the transcript, on a divider — cannot select prompt text.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<InputBarAction> {
        let pressed = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.drag_field = None;
            self.pressed = false;
            return None;
        }
        if !pressed && !matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) {
            return None;
        }
        // A drag without a press on this bar belongs to whatever else the
        // pointer was dragging — the transcript, a divider — not to the bar.
        if !pressed && self.drag_field.is_none() {
            return None;
        }
        // A drag belongs to the field its press started in, and clamps to that
        // field's rows: dragged past the top or bottom edge it selects to the
        // edge instead of computing a position outside the field.
        let (col, row) = match self.drag_field {
            Some(i) if !pressed => match self.field_areas.get(i).copied() {
                Some(area) => (
                    mouse
                        .column
                        .clamp(area.x, area.x + area.width.saturating_sub(1)),
                    mouse
                        .row
                        .clamp(area.y, area.y + area.height.saturating_sub(1)),
                ),
                None => return None,
            },
            _ => (mouse.column, mouse.row),
        };
        for i in 0..self.field_areas.len() {
            let area = self.field_areas[i];
            if !hit_area(area, col, row) {
                continue;
            }
            // Only a press takes ownership of the drag: a drag that extends the
            // selection leaves it in place, so the drags that follow — past the
            // edge of the field, into the transcript — keep extending it too.
            if pressed {
                self.drag_field = Some(i);
            }
            self.pressed = true;
            // Match the rendered prefix: "› " for an empty (prompt) label.
            let prefix = if self.labels[i].is_empty() {
                "› "
            } else {
                &self.labels[i]
            };
            let label_w = str_display_width(prefix) as u16;
            let start_x = area.x + label_w;
            let text_x = usize::from(col.saturating_sub(start_x));
            match &mut self.fields[i] {
                FieldInput::Line(input) => {
                    // A press on the label lands at the start of the text.
                    let pos = screen_x_to_char_pos(input.text(), text_x);
                    if pressed {
                        self.focus = i;
                        input.set_cursor_with_selection_start(pos);
                    } else {
                        input.extend_selection_to(pos);
                    }
                }
                FieldInput::Multi(ta) => {
                    // Map through the layout the last render recorded, so a
                    // click on a wrapped row lands on the character drawn there.
                    let pos = ta.position_at_drawn_row(usize::from(row - area.y), text_x);
                    if pressed {
                        self.focus = i;
                        ta.place_cursor(pos.row, pos.col);
                    } else {
                        ta.extend_selection_to(pos.row, pos.col);
                    }
                }
            }
            return None;
        }
        if !pressed {
            return None;
        }
        let clicked = self
            .control_areas
            .iter()
            .find_map(|(area, idx)| hit(*area, col, row).then_some(*idx));
        if let Some(idx) = clicked {
            self.focus = self.labels.len() + idx;
            self.pressed = true;
            return Some(self.activate(idx));
        }
        None
    }

    /// Whether [`InputBar::handle_mouse`] would act on `mouse`: a press or a
    /// drag over one of its fields or controls, a drag still owned by the bar
    /// (its button went down there and has not been released), or the release
    /// that ends it. Hover and wheel events never belong to the bar, so a host
    /// can ask this instead of repainting on every pointer motion.
    #[must_use]
    pub fn mouse_hits(&self, mouse: MouseEvent) -> bool {
        match mouse.kind {
            MouseEventKind::Up(MouseButton::Left) => self.pressed,
            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag_field.is_some() || self.click_hits(mouse.column, mouse.row)
            }
            MouseEventKind::Down(MouseButton::Left) => self.click_hits(mouse.column, mouse.row),
            _ => false,
        }
    }

    /// Whether a field holds a selection, so the host knows a `Ctrl+C` or
    /// `Ctrl+X` has something to take.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.fields.iter().any(|field| match field {
            FieldInput::Line(input) => input.has_selection(),
            FieldInput::Multi(area) => area.has_selection(),
        })
    }

    /// The selected text of the focused field, if it has one.
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        let index = match self.focus() {
            Focus::Field(i) => i,
            Focus::Control(_) => return None,
        };
        self.fields.get(index).and_then(|field| match field {
            FieldInput::Line(input) => input.selected_text().map(str::to_string),
            FieldInput::Multi(area) => area.selected_text(),
        })
    }

    /// Whether a click at `(col, row)` lands on one of the bar's controls
    /// (after [`InputBar::render`] recorded their areas).
    #[must_use]
    pub fn click_hits(&self, col: u16, row: u16) -> bool {
        self.field_areas.iter().any(|a| hit_area(*a, col, row))
            || self.control_areas.iter().any(|(a, _)| hit(*a, col, row))
    }

    // === Rendering ===

    /// Render the bar into `area`. `active` is whether the bar (not the panel
    /// body) holds focus — it controls the cursor and focus highlight.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, colors: &ThemeColors, active: bool) {
        self.field_areas.clear();
        self.control_areas.clear();
        self.border_button_areas.clear();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let focus = self.focus();
        let mut y = area.y;
        if let Some(border) = &self.border {
            // Buttons sit side by side at the right end (their brackets set
            // them apart), one short of the edge; the right slot's text moves
            // left of them.
            let widths: Vec<u16> = self
                .border_buttons
                .iter()
                .map(|(label, _)| str_display_width(label) as u16)
                .collect();
            let reserve = widths.iter().sum::<u16>() + u16::from(!widths.is_empty());
            let fits = reserve + 2 < area.width;
            let reserve = if fits { reserve } else { 0 };
            render_border(
                area,
                y,
                buf,
                colors,
                active,
                &border.left,
                &border.right,
                reserve,
            );
            if fits {
                let mut x = area.x + area.width - reserve;
                for ((label, style), w) in self.border_buttons.iter().zip(widths) {
                    buf.set_string(x, y, label, *style);
                    self.border_button_areas.push(Rect {
                        x,
                        y,
                        width: w,
                        height: 1,
                    });
                    x += w;
                }
            }
            y += 1;
        }

        // Rows available to the fields, after the border and whatever the
        // controls row (with its own separator, when bordered) will need.
        let reserved = if self.controls.is_empty() {
            0
        } else {
            1 + u16::from(self.border.is_some())
        };
        let mut budget = area
            .height
            .saturating_sub(y - area.y)
            .saturating_sub(reserved);

        let placeholder = self.placeholder.clone();
        for i in 0..self.fields.len() {
            if budget == 0 {
                break;
            }
            // A multi-line field takes what it needs but no more than is left,
            // scrolling within that; a single-line field takes one row.
            let rows = self.fields[i].rows().min(budget);
            let field_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: rows,
            };
            self.field_areas.push(field_area);
            budget -= rows;
            let focused = active && focus == Focus::Field(i);
            let label = self.labels[i].clone();
            match &mut self.fields[i] {
                FieldInput::Line(input) => {
                    let row = Rect {
                        height: 1,
                        ..field_area
                    };
                    render_labeled_input(
                        buf,
                        row,
                        &LabeledInput {
                            label: &label,
                            text: input.text(),
                            cursor: input.cursor_pos(),
                            selection: input.selection_range(),
                            focused,
                        },
                        colors,
                    );
                }
                FieldInput::Multi(ta) => {
                    render_multiline(
                        buf,
                        field_area,
                        &label,
                        placeholder.as_deref(),
                        ta,
                        focused,
                        colors,
                    );
                }
            }
            y += rows;
        }

        if !self.controls.is_empty() {
            if self.border.is_some() {
                // A separator between the fields and the controls row.
                let style = Style::default().fg(colors.border);
                for dx in 0..area.width {
                    buf[(area.x + dx, y)].set_symbol("─").set_style(style);
                }
                y += 1;
            }
            let row = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            self.render_controls(row, buf, colors, active, focus);
        } else if let Some(last) = self.field_areas.last().copied() {
            // With no controls row, the status rides on the last field row so
            // a bar of fields alone (a name search) still shows its counter.
            self.render_status(last, buf, colors);
        }
    }

    /// Right-aligned status text; returns the x where it starts so controls
    /// can stop short of it.
    fn render_status(&self, area: Rect, buf: &mut Buffer, colors: &ThemeColors) -> u16 {
        let status = self.status.as_deref().unwrap_or("");
        let status_w = str_display_width(status) as u16;
        let left = area.x + area.width.saturating_sub(status_w);
        if !status.is_empty() {
            buf.set_string(left, area.y, status, Style::default().fg(colors.disabled));
        }
        left
    }

    fn render_controls(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        colors: &ThemeColors,
        active: bool,
        focus: Focus,
    ) {
        // Right-aligned status first, so controls stop short of it.
        let status_left = self.render_status(area, buf, colors);

        let mut x = area.x;
        for idx in 0..self.controls.len() {
            let focused = active && focus == Focus::Control(idx);
            let (text, style) = self.control_render(idx, focused, colors);
            let w = str_display_width(&text) as u16;
            if x + w >= status_left {
                break;
            }
            self.control_areas.push((
                Rect {
                    x,
                    y: area.y,
                    width: w,
                    height: 1,
                },
                idx,
            ));
            buf.set_string(x, area.y, &text, style);
            x += w + 1;
        }
    }

    fn control_render(&self, index: usize, focused: bool, colors: &ThemeColors) -> (String, Style) {
        match &self.controls[index] {
            Control::Button { label } => {
                let text = if focused {
                    format!("[ {label} ]")
                } else {
                    format!("  {label}  ")
                };
                let mut style = Style::default().fg(colors.fg);
                if focused {
                    style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
                }
                (text, style)
            }
            Control::Toggle { label, on } => {
                let mark = if *on { "x" } else { " " };
                let text = format!("[{mark}] {label}");
                let mut style = if *on {
                    Style::default()
                        .fg(colors.info)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors.disabled)
                };
                if focused {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                (text, style)
            }
        }
    }
}

/// Route an editing key to one field, whatever kind it is.
fn edit_field_input(field: &mut FieldInput, key: KeyEvent) -> FieldEdit {
    match field {
        FieldInput::Line(input) => edit_text_input(input, key),
        FieldInput::Multi(area) => edit_text_area(area, key),
    }
}

/// Draw a top border `───` with the left text just after the corner and the
/// right text just before it, or before the `reserve` columns the border
/// buttons take at the right end.
#[allow(clippy::too_many_arguments)]
fn render_border(
    area: Rect,
    y: u16,
    buf: &mut Buffer,
    colors: &ThemeColors,
    active: bool,
    left: &str,
    right: &str,
    reserve: u16,
) {
    let border_color = if active {
        colors.border_focused
    } else {
        colors.border
    };
    let style = Style::default().fg(border_color);
    for dx in 0..area.width {
        buf[(area.x + dx, y)].set_symbol("─").set_style(style);
    }
    let label_style = Style::default().fg(colors.disabled);
    if !left.is_empty() && area.width > 2 {
        let text: String = left
            .chars()
            .take(area.width.saturating_sub(2) as usize)
            .collect();
        buf.set_string(area.x + 1, y, &text, label_style);
    }
    if !right.is_empty() {
        let w = str_display_width(right) as u16;
        if w + 1 + reserve < area.width {
            buf.set_string(area.x + area.width - 1 - w - reserve, y, right, label_style);
        }
    }
}

/// One labeled single-line input to render.
struct LabeledInput<'a> {
    label: &'a str,
    text: &'a str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    focused: bool,
}

/// Render `label` then `text` as a single-line input, scrolled to keep the
/// cursor visible, with a selection highlight and (when focused) the cursor.
fn render_labeled_input(buf: &mut Buffer, area: Rect, field: &LabeledInput, colors: &ThemeColors) {
    let LabeledInput {
        label,
        text,
        cursor,
        selection,
        focused,
    } = *field;
    // An empty label renders the bar's prompt marker instead, so a
    // single-field bar reads like the agent input (its name is in the border).
    let prompt = if label.is_empty() { "› " } else { label };
    let label_w = str_display_width(prompt) as u16;
    buf.set_string(area.x, area.y, prompt, Style::default().fg(colors.fg));
    let x0 = area.x + label_w;
    let width = area.width.saturating_sub(label_w);
    if width == 0 {
        return;
    }

    let chars: Vec<char> = text.chars().collect();
    let widths: Vec<usize> = chars.iter().map(|c| char_width(*c)).collect();
    // Scroll so the cursor's column stays within `width`.
    let cursor_col: usize = widths.iter().take(cursor).sum();
    let mut start = 0usize;
    let mut lead: usize = 0;
    if cursor_col >= width as usize {
        // Drop leading chars until the cursor fits.
        let mut used = 0usize;
        start = chars.len();
        for i in (0..chars.len()).rev() {
            let w = widths[i];
            if used + w > width as usize - 1 {
                break;
            }
            used += w;
            start = i;
        }
        lead = widths[..start].iter().sum();
    }
    let _ = lead;

    let base = if focused {
        Style::default().fg(colors.fg).bg(colors.bg)
    } else {
        Style::default().fg(colors.fg)
    };
    let invert = Style::default().fg(colors.bg).bg(colors.fg);
    let mut col = 0u16;
    for i in start..chars.len() {
        let cw = widths[i] as u16;
        if col + cw > width {
            break;
        }
        let selected = selection.is_some_and(|(s, e)| i >= s && i < e);
        let at_cursor = focused && i == cursor;
        let style = if at_cursor || selected { invert } else { base };
        buf.set_string(x0 + col, area.y, chars[i].to_string(), style);
        col += cw;
    }
    // Cursor at end of text.
    if focused && cursor >= chars.len() && col < width {
        buf[(x0 + col, area.y)].set_style(invert);
    }
}

/// Render a multi-line field (a prompt box): the prompt marker (or label) on
/// the first row, an equal-width indent on continuations, the text scrolled to
/// keep the cursor visible, an idle placeholder, and the cursor when focused.
#[allow(clippy::too_many_arguments)]
fn render_multiline(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    placeholder: Option<&str>,
    ta: &mut TextArea,
    focused: bool,
    colors: &ThemeColors,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let prompt = if label.is_empty() { "› " } else { label };
    let label_w = str_display_width(prompt) as u16;
    let indent: String = " ".repeat(label_w as usize);
    let text_x = area.x + label_w;
    let text_width = area.width.saturating_sub(label_w);
    if text_width == 0 {
        return;
    }

    // Keeps the logical scroll offset (used by click-to-position) roughly in
    // step; the visual scroll below is what the wrapped render actually uses.
    ta.ensure_cursor_visible(area.height as usize);
    let prompt_style = Style::default().fg(colors.fg);
    let text_style = Style::default().fg(colors.fg);
    let tw = text_width as usize;

    // Soft-wrap: each logical line becomes one or more visual rows, so a long
    // prompt reflows instead of being clipped. `visual[i] = (row, start, end)`
    // is the char range [start, end) of logical `row` shown on that visual row.
    let mut visual: Vec<(usize, usize, usize)> = Vec::new();
    for (r, line) in ta.lines().iter().enumerate() {
        for (start, end) in wrap_line(line, tw) {
            visual.push((r, start, end));
        }
    }

    // Scroll to keep the cursor's visual row on screen (bottom-anchored while
    // typing at the end).
    let cursor = ta.cursor();
    let cursor_vi = visual
        .iter()
        .enumerate()
        .filter(|(_, (row, start, _))| *row == cursor.row && *start <= cursor.col)
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let height = area.height as usize;
    let scroll = cursor_vi.saturating_sub(height.saturating_sub(1));

    // Hand the layout back to the area before borrowing its text again: a click
    // on a wrapped row then maps to the character drawn there, not to the
    // logical row it started from.
    ta.set_wrap(visual.clone(), scroll);
    let lines = ta.lines();

    // Char range to invert on each logical row, so a selection that spans
    // wrapped rows reads as one solid block.
    let selected: Vec<(usize, usize, usize)> = match ta.selection_range() {
        Some((start, end)) if start != end => (start.row..=end.row)
            .map(|row| {
                let from = if row == start.row { start.col } else { 0 };
                let to = if row == end.row {
                    end.col
                } else {
                    lines[row].chars().count()
                };
                (row, from, to)
            })
            .filter(|(_, from, to)| from < to)
            .collect(),
        _ => Vec::new(),
    };
    let selected_style = Style::default().fg(colors.bg).bg(colors.fg);

    for r in 0..height {
        let y = area.y + r as u16;
        let prefix = if r == 0 { prompt } else { indent.as_str() };
        buf.set_string(area.x, y, prefix, prompt_style);
        if let Some(&(row, start, end)) = visual.get(scroll + r) {
            let seg: String = lines[row].chars().skip(start).take(end - start).collect();
            buf.set_stringn(text_x, y, &seg, tw, text_style);
            // Invert the selected characters of this visual row. One
            // logical row holds at most one selected range, and it can show
            // on several wrapped rows of that line.
            let selection = selected.iter().find(|(srow, _, _)| *srow == row);
            let mut col = 0usize;
            for (i, c) in lines[row].chars().enumerate().skip(start).take(end - start) {
                let cw = char_width(c);
                if col + cw > tw {
                    break;
                }
                if selection.is_some_and(|(_, from, to)| i >= *from && i < *to) {
                    for x in 0..cw {
                        buf[(text_x + (col + x) as u16, y)].set_style(selected_style);
                    }
                }
                col += cw;
            }
        }
    }

    let empty = lines.len() <= 1 && lines.first().is_none_or(String::is_empty);
    if empty && !focused {
        if let Some(ph) = placeholder {
            buf.set_stringn(text_x, area.y, ph, tw, Style::default().fg(colors.disabled));
        }
    }

    if focused {
        if let Some(vr) = cursor_vi.checked_sub(scroll).filter(|vr| *vr < height) {
            let (row, start, _) = visual[cursor_vi];
            let col: usize = lines[row]
                .chars()
                .skip(start)
                .take(cursor.col - start)
                .map(char_width)
                .sum();
            if (col as u16) < text_width {
                let x = text_x + col as u16;
                let y = area.y + vr as u16;
                buf[(x, y)].set_style(Style::default().fg(colors.bg).bg(colors.fg));
            }
        }
    }
}

/// Split `line` into visual segments no wider than `width` display columns,
/// each a `[start, end)` char-index range. An empty line yields one empty
/// segment so it still occupies a row.
fn wrap_line(line: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut w = 0usize;
    for (i, c) in line.chars().enumerate() {
        let cw = char_width(c);
        if w + cw > width && i > start {
            out.push((start, i));
            start = i;
            w = 0;
        }
        w += cw;
    }
    out.push((start, line.chars().count()));
    out
}

/// The number of visual rows the text occupies when soft-wrapped to `width`.
pub fn wrapped_row_count(text: &str, width: usize) -> usize {
    text.split('\n')
        .map(|line| wrap_line(line, width).len())
        .sum()
}

fn char_width(c: char) -> usize {
    str_display_width(&c.to_string()).max(1)
}

/// The char position in `text` under a screen x-offset inside the input,
/// accounting for wide characters; past the end returns the length.
fn screen_x_to_char_pos(text: &str, screen_x: usize) -> usize {
    let mut width = 0;
    for (i, c) in text.chars().enumerate() {
        let cw = char_width(c);
        if width + cw > screen_x {
            return i;
        }
        width += cw;
    }
    text.chars().count()
}

fn hit(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.x + area.width && row == area.y
}

/// Like [`hit`], but matches any row within a multi-row area.
fn hit_area(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn bar() -> InputBar {
        InputBar::new(vec!["Find: ".into(), "Repl: ".into()])
            .with_control(Control::Toggle {
                label: "Case".into(),
                on: false,
            })
            .with_control(Control::Button {
                label: "Next".into(),
            })
    }

    #[test]
    fn wrap_line_splits_on_display_width() {
        // Empty line still occupies one (empty) row.
        assert_eq!(wrap_line("", 4), vec![(0, 0)]);
        // Fits exactly, no split.
        assert_eq!(wrap_line("abcd", 4), vec![(0, 4)]);
        // One extra char spills onto a second row.
        assert_eq!(wrap_line("abcde", 4), vec![(0, 4), (4, 5)]);
        // A wide char (2 columns) wraps a char earlier.
        assert_eq!(wrap_line("aaa世", 4), vec![(0, 3), (3, 4)]);
    }

    #[test]
    fn wrapped_row_count_sums_logical_lines() {
        assert_eq!(wrapped_row_count("", 4), 1);
        assert_eq!(wrapped_row_count("abcd", 4), 1);
        assert_eq!(wrapped_row_count("abcdef", 4), 2);
        // Two logical lines, the second wraps once.
        assert_eq!(wrapped_row_count("ab\nabcde", 4), 3);
        // A trailing newline yields an extra empty row.
        assert_eq!(wrapped_row_count("abcd\n", 4), 2);
    }

    #[test]
    fn height_counts_border_fields_and_controls() {
        assert_eq!(bar().height(), 3); // 2 fields + control row
        assert_eq!(InputBar::new(vec!["x".into()]).height(), 1);
        // border + 2 fields + internal separator + control row
        assert_eq!(bar().with_border("l", "r").height(), 5);
        // border + 1 field, no controls, no separator
        assert_eq!(
            InputBar::new(vec![String::new()])
                .with_border("Find", "")
                .height(),
            2
        );
    }

    #[test]
    fn typing_edits_the_focused_field_and_reports_it() {
        let mut b = bar();
        assert_eq!(b.focused_field(), Some(0));
        assert_eq!(
            b.handle_key(key(KeyCode::Char('a'))),
            Some(InputBarAction::Edited(0))
        );
        assert_eq!(b.field_text(0), "a");
        assert_eq!(b.field_text(1), "");
    }

    #[test]
    fn tab_walks_fields_then_controls_and_wraps() {
        let mut b = bar();
        b.handle_key(key(KeyCode::Tab)); // Find -> Repl
        assert_eq!(b.focused_field(), Some(1));
        b.handle_key(key(KeyCode::Tab)); // Repl -> control 0
        assert_eq!(b.focus(), Focus::Control(0));
        b.handle_key(key(KeyCode::Tab)); // -> control 1
        assert_eq!(b.focus(), Focus::Control(1));
        b.handle_key(key(KeyCode::Tab)); // wraps -> field 0
        assert_eq!(b.focused_field(), Some(0));
    }

    #[test]
    fn enter_on_a_field_submits_it() {
        let mut b = bar();
        b.handle_key(key(KeyCode::Tab));
        assert_eq!(
            b.handle_key(key(KeyCode::Enter)),
            Some(InputBarAction::Submit(1))
        );
    }

    #[test]
    fn activating_a_toggle_flips_it_and_reports() {
        let mut b = bar();
        b.focus_field(0);
        for _ in 0..2 {
            b.handle_key(key(KeyCode::Tab));
        }
        assert_eq!(b.focus(), Focus::Control(0));
        assert!(!b.control_on(0));
        assert_eq!(
            b.handle_key(key(KeyCode::Char(' '))),
            Some(InputBarAction::Activated(0))
        );
        assert!(b.control_on(0));
        // A plain button activates without a state.
        b.handle_key(key(KeyCode::Right));
        assert_eq!(
            b.handle_key(key(KeyCode::Enter)),
            Some(InputBarAction::Activated(1))
        );
    }

    #[test]
    fn left_right_cycle_controls_and_up_returns_to_last_field() {
        let mut b = bar();
        for _ in 0..2 {
            b.handle_key(key(KeyCode::Tab));
        }
        assert_eq!(b.focus(), Focus::Control(0));
        b.handle_key(key(KeyCode::Right));
        assert_eq!(b.focus(), Focus::Control(1));
        b.handle_key(key(KeyCode::Left));
        assert_eq!(b.focus(), Focus::Control(0));
        b.handle_key(key(KeyCode::Up));
        assert_eq!(b.focused_field(), Some(1));
    }

    #[test]
    fn esc_closes_from_anywhere() {
        let mut b = bar();
        assert_eq!(b.handle_key(key(KeyCode::Esc)), Some(InputBarAction::Close));
        b.handle_key(key(KeyCode::Tab));
        assert_eq!(b.handle_key(key(KeyCode::Esc)), Some(InputBarAction::Close));
    }

    #[test]
    fn seed_and_read_back_field_text() {
        let mut b = bar();
        b.set_field_text(0, "needle");
        b.set_field_text(1, "thread");
        assert_eq!(b.field_text(0), "needle");
        assert_eq!(b.field_text(1), "thread");
    }

    #[test]
    fn a_multiline_field_grows_the_bar_height() {
        // One prompt field, no border: one row while empty.
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        assert_eq!(b.field_count(), 1);
        assert_eq!(b.height(), 1);
        // Two extra lines make it three rows tall.
        let area = b.multiline_mut(0).unwrap();
        area.insert_str("a\nb\nc");
        assert_eq!(b.height(), 3);
    }

    #[test]
    fn typing_into_a_multiline_field_reports_and_reads_back() {
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        assert_eq!(
            b.handle_key(key(KeyCode::Char('h'))),
            Some(InputBarAction::Edited(0))
        );
        assert_eq!(b.field_value(0), "h");
        // Enter submits a field rather than inserting a newline; the host wires
        // newline insertion through `multiline_mut`.
        assert_eq!(
            b.handle_key(key(KeyCode::Enter)),
            Some(InputBarAction::Submit(0))
        );
    }

    #[test]
    fn a_click_on_a_multiline_field_row_focuses_and_places_the_cursor() {
        let mut b = InputBar::new(vec![])
            .with_multiline_field("")
            .with_control(Control::Button {
                label: "Send".into(),
            });
        b.multiline_mut(0).unwrap().insert_str("one\ntwo");
        let area = Rect::new(0, 0, 40, b.height());
        let mut buf = Buffer::filled(area, ratatui::buffer::Cell::default());
        b.render(area, &mut buf, &ThemeColors::default(), true);
        // Click on the second wrapped row, inside "two" after the "› " prefix.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(b.handle_mouse(click), None);
        assert_eq!(b.focused_field(), Some(0));
        let cursor = b.multiline(0).unwrap().cursor();
        assert_eq!(cursor.row, 1);
        assert_eq!(cursor.col, 2);
    }

    #[test]
    fn a_click_focuses_a_field_and_places_the_cursor() {
        use ratatui::style::Style;
        let mut b = bar();
        b.set_field_text(0, "hello");
        let area = Rect::new(0, 0, 40, b.height());
        let mut buf = Buffer::filled(area, ratatui::buffer::Cell::default());
        let _ = Style::default();
        b.render(area, &mut buf, &ThemeColors::default(), true);
        // Click inside the second field's text (row 1, after the "Repl: " label).
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(b.handle_mouse(click), None);
        assert_eq!(b.focused_field(), Some(1));
        // Clicking a control activates it.
        let (carea, _) = b.control_areas[0];
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: carea.x,
            row: carea.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(b.handle_mouse(click), Some(InputBarAction::Activated(0)));
    }

    #[test]
    fn a_drag_across_a_multiline_field_selects_the_text_under_it() {
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        b.multiline_mut(0).unwrap().insert_str("one two\nthree");
        let area = Rect::new(0, 0, 40, b.height());
        let mut buf = Buffer::empty(area);
        b.render(area, &mut buf, &ThemeColors::default(), true);
        let at = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // "› " occupies the first two columns, so column 4 is inside "one".
        assert_eq!(
            b.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 4, 0)),
            None
        );
        assert!(!b.has_selection());
        b.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 6, 1));
        assert_eq!(
            b.multiline(0).unwrap().selected_text(),
            Some("e two\nthre".into())
        );
        // The release ends the drag but keeps the selection, so `Ctrl+C` still
        // has what was just dragged out.
        b.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 6, 1));
        assert_eq!(
            b.multiline(0).unwrap().selected_text(),
            Some("e two\nthre".into())
        );
        assert!(!b.mouse_hits(at(MouseEventKind::Up(MouseButton::Left), 6, 1)));
        // A later drag whose press was not on this bar leaves it alone.
        b.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 6, 0));
        assert_eq!(
            b.multiline(0).unwrap().selected_text(),
            Some("e two\nthre".into())
        );
    }

    #[test]
    fn a_drag_keeps_selecting_while_the_button_travels_on() {
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        b.multiline_mut(0).unwrap().insert_str("one two three four");
        let area = Rect::new(0, 0, 40, b.height());
        let mut buf = Buffer::empty(area);
        b.render(area, &mut buf, &ThemeColors::default(), true);
        let at = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        b.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 4, 0));
        // Terminal emulators report a drag per cell the pointer crosses, so a
        // real selection is a run of them: each one must extend the selection
        // further, not only the first. Column 4 sits on char index 2 of the
        // text (the "› " prefix occupies the first two columns), so a drag to
        // column N selects up to index N - 2.
        let expected = ["e", "e ", "e t", "e tw"];
        for (i, want) in expected.iter().enumerate() {
            let column = 5 + i as u16;
            b.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), column, 0));
            assert_eq!(
                b.multiline(0).unwrap().selected_text(),
                Some((*want).to_string()),
                "drag to column {column}"
            );
        }
        // The bar still owns the pointer, so a drag that has wandered up into
        // the transcript is claimed and clamped rather than dropped.
        assert!(b.mouse_hits(at(MouseEventKind::Drag(MouseButton::Left), 3, 9)));
    }

    #[test]
    fn a_drag_clamps_to_the_field_rows() {
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        b.multiline_mut(0).unwrap().insert_str("one\ntwo");
        let area = Rect::new(0, 0, 40, b.height());
        let mut buf = Buffer::empty(area);
        b.render(area, &mut buf, &ThemeColors::default(), true);
        let at = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        b.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 4, 0));
        // Dragged off the bottom of the field: the selection clamps to its last
        // row instead of computing a position outside the field.
        b.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 4, 20));
        assert_eq!(
            b.multiline(0).unwrap().selected_text(),
            Some("e\ntw".into())
        );
    }

    #[test]
    fn shift_arrows_select_and_ctrl_a_selects_all() {
        let mut b = bar();
        b.set_field_text(0, "needle");
        for _ in 0..3 {
            b.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        for _ in 0..3 {
            b.handle_field_key(0, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        }
        assert_eq!(b.field_text(0), "needle");
        assert_eq!(b.selected_text(), Some("dle".into()));
        // A plain arrow drops the selection instead of extending it.
        b.handle_field_key(0, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(b.selected_text(), None);
        // Ctrl+A takes the whole field.
        b.handle_field_key(0, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(b.selected_text(), Some("needle".into()));
    }

    #[test]
    fn a_selection_shows_inverted_in_the_prompt_box() {
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        b.multiline_mut(0).unwrap().insert_str("hello there");
        b.multiline_mut(0).unwrap().select_all();
        let area = Rect::new(0, 0, 20, b.height());
        let mut buf = Buffer::empty(area);
        b.render(area, &mut buf, &ThemeColors::default(), true);
        // The text starts after the "› " prompt marker; the selection inverts
        // the cell's foreground and background, as the chat block does.
        let colors = ThemeColors::default();
        let selected: Vec<u16> = (2..13)
            .filter(|x| buf[(*x, 0)].bg == colors.fg && buf[(*x, 0)].fg == colors.bg)
            .collect();
        assert_eq!(selected, (2..13).collect::<Vec<_>>());
        // Cell 13 is the cursor at the end of the text; past it the cells
        // stay plain.
        assert_eq!(buf[(13, 0)].bg, colors.fg);
        assert_ne!(buf[(15, 0)].bg, colors.fg);
    }

    #[test]
    fn border_buttons_sit_at_the_right_end_and_report_their_clicks() {
        let mut b = InputBar::new(vec![])
            .with_multiline_field("")
            .with_border("", "hint");
        let style = Style::default();
        b.set_border_buttons(vec![("[a]".into(), style), ("[b]".into(), style)]);
        let area = Rect::new(0, 0, 20, b.height());
        let mut buf = Buffer::empty(area);
        b.render(area, &mut buf, &ThemeColors::default(), true);
        let row: String = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        // Side by side, one border cell short of the edge; the right slot's
        // text moves left of them.
        assert!(row.ends_with("hint─[a][b]─"), "{row:?}");
        assert_eq!(b.border_button_at(13, 0), Some(0));
        assert_eq!(b.border_button_at(16, 0), Some(1));
        assert_eq!(b.border_button_at(19, 0), None);
        assert_eq!(b.border_button_at(16, 1), None);
    }

    #[test]
    fn the_bar_claims_only_presses_and_drags_on_its_own_rows() {
        let mut b = InputBar::new(vec![]).with_multiline_field("");
        let area = Rect::new(0, 0, 40, b.height());
        let mut buf = Buffer::empty(area);
        b.render(area, &mut buf, &ThemeColors::default(), true);
        let at = |kind: MouseEventKind, row| MouseEvent {
            kind,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert!(b.mouse_hits(at(MouseEventKind::Down(MouseButton::Left), 0)));
        assert!(!b.mouse_hits(at(MouseEventKind::Down(MouseButton::Left), 1)));
        assert!(b.mouse_hits(at(MouseEventKind::Drag(MouseButton::Left), 0)));
        assert!(!b.mouse_hits(at(MouseEventKind::ScrollUp, 0)));
        assert!(!b.mouse_hits(at(MouseEventKind::Up(MouseButton::Left), 0)));
        b.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 0));
        // Once a drag has started in the field, its release belongs to it too.
        assert!(b.mouse_hits(at(MouseEventKind::Up(MouseButton::Left), 7)));
    }
}
