//! A plugin's panel drawn as a list: groups of rows, each with the actions
//! the plugin offers on it. termide draws it and handles the keys and the
//! mouse; a chosen action goes back to the plugin as
//! [`PanelEvent::PluginAction`], which answers by sending a new list.
//!
//! The keys are git status's (`[git_status.keybindings]`): an action the
//! plugin marks `view`, `edit` or `revert` takes that key, `Enter` takes a
//! row's first action, the info key lists all of them, and the refresh key
//! sends the list's own refresh action.

use std::any::Any;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use termide_config::{is_go_end, is_go_home, is_move_down, is_move_up, Config};
use termide_core::event::SelectAction;
use termide_core::{
    CommandResult, HotkeyTable, KeyChord, Panel, PanelCommand, PanelEvent, RenderContext,
    SegmentKind, StatusSegment, Theme, ThemeColors, WidthPreference,
};
use termide_ui::path_utils::truncate_right;
use termide_ui::{IndexClickTracker, ScrollBar};

/// What a plugin panel's own `[≡]` entries are routed back under.
const MENU_PREFIX: &str = "plugin-menu:";
/// The selection modal listing a row's actions answers under this.
const ACTIONS_PICK: &str = "plugin-list-actions";
/// Status-bar chips of the selected row's actions, by position.
const ROW_CHIPS: [&str; MAX_ROW_ACTIONS] = [
    "row-action:0",
    "row-action:1",
    "row-action:2",
    "row-action:3",
    "row-action:4",
    "row-action:5",
];

/// The most actions a row offers.
pub const MAX_ROW_ACTIONS: usize = 6;

/// The key an action takes, named as in `[git_status.keybindings]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKey {
    View,
    Edit,
    Revert,
}

impl ActionKey {
    /// The key's name in `[git_status.keybindings]`.
    pub fn name(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Edit => "edit",
            Self::Revert => "revert",
        }
    }

    /// The key of the name, if it is one.
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::View, Self::Edit, Self::Revert]
            .into_iter()
            .find(|key| key.name() == name)
    }
}

/// An action a row offers: what it reads and what goes back to the plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListAction {
    pub label: String,
    pub action: String,
    pub key: Option<ActionKey>,
}

/// A file's state, coloured as git's would be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Modified,
    Added,
    Deleted,
    Ignored,
}

impl ItemStatus {
    /// The status of its name, as `files/status` writes them.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "modified" => Some(Self::Modified),
            "added" => Some(Self::Added),
            "deleted" => Some(Self::Deleted),
            "ignored" => Some(Self::Ignored),
            _ => None,
        }
    }

    fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Ignored => "I",
        }
    }

    fn style(self, theme: &Theme) -> Style {
        match self {
            Self::Modified => Style::default().fg(theme.warning),
            Self::Added => Style::default().fg(theme.success),
            Self::Deleted => Style::default().fg(theme.error),
            Self::Ignored => Style::default()
                .fg(theme.disabled)
                .add_modifier(Modifier::DIM),
        }
    }
}

/// A row under a group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListItem {
    pub label: String,
    /// Shown dimmed after the label.
    pub detail: String,
    pub status: Option<ItemStatus>,
    pub actions: Vec<ListAction>,
}

/// A group's heading row and the rows under it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListGroup {
    pub label: String,
    pub detail: String,
    pub actions: Vec<ListAction>,
    pub items: Vec<ListItem>,
}

/// What the panel shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanelList {
    pub groups: Vec<ListGroup>,
    /// Shown when there are no groups.
    pub empty: String,
    /// Sent on the refresh key.
    pub refresh: Option<String>,
}

/// A row on screen: a group's heading, or an item of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Group(usize),
    Item(usize, usize),
}

/// A plugin's list panel.
pub struct PluginListPanel {
    /// The id the app gave the panel.
    id: u64,
    title: String,
    list: PanelList,
    rows: Vec<Row>,
    selected: usize,
    scroll: usize,
    /// The panel's own entries of its `[≡]` menu: label and action.
    menu: Vec<(String, String)>,
    hotkeys: HotkeyTable,
    /// The config the keys were read from.
    config: Option<Arc<Config>>,
    theme: Theme,
    vim_mode: bool,
    last_height: usize,
    scrollbars: termide_core::ScrollBars,
    clicks: IndexClickTracker,
    /// Events raised outside a key or a click, handed over on the next tick.
    pending: Vec<PanelEvent>,
}

impl PluginListPanel {
    /// A panel a plugin opened: `id` is what the app knows it by.
    pub fn new(id: u64, title: String, list: PanelList) -> Self {
        let mut panel = Self {
            id,
            title,
            list: PanelList::default(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            menu: Vec::new(),
            hotkeys: HotkeyTable::new(),
            config: None,
            theme: Theme::default(),
            vim_mode: false,
            last_height: 10,
            scrollbars: termide_core::ScrollBars::default(),
            clicks: IndexClickTracker::new(),
            pending: Vec::new(),
        };
        panel.set_list(None, list);
        panel
    }

    /// The id the app gave the panel.
    pub fn plugin_panel_id(&self) -> u64 {
        self.id
    }

    /// Set the panel's own `[≡]` entries, `(label, action)`.
    pub fn set_plugin_menu(&mut self, menu: Vec<(String, String)>) {
        self.menu = menu;
    }

    /// Replace the list, keeping the selection on the same row when it is
    /// still there.
    pub fn set_list(&mut self, title: Option<String>, list: PanelList) {
        if let Some(title) = title {
            self.title = title;
        }
        let kept = self.selected_key();
        self.list = list;
        self.rows = self
            .list
            .groups
            .iter()
            .enumerate()
            .flat_map(|(g, group)| {
                std::iter::once(Row::Group(g))
                    .chain((0..group.items.len()).map(move |i| Row::Item(g, i)))
            })
            .collect();
        let found = kept
            .and_then(|key| (0..self.rows.len()).find(|&r| self.row_key(r) == Some(key.clone())));
        self.selected =
            found.unwrap_or_else(|| self.selected.min(self.rows.len().saturating_sub(1)));
        self.ensure_visible();
    }

    /// The plugin is gone: `message` says so, and nothing is left to do.
    pub fn set_stopped(&mut self, message: String) {
        self.menu.clear();
        self.set_list(
            None,
            PanelList {
                empty: message,
                ..PanelList::default()
            },
        );
    }

    /// What tells a row apart across updates: its group's label and its own.
    fn row_key(&self, row: usize) -> Option<(String, Option<String>)> {
        Some(match *self.rows.get(row)? {
            Row::Group(g) => (self.list.groups[g].label.clone(), None),
            Row::Item(g, i) => (
                self.list.groups[g].label.clone(),
                Some(self.list.groups[g].items[i].label.clone()),
            ),
        })
    }

    fn selected_key(&self) -> Option<(String, Option<String>)> {
        self.row_key(self.selected)
    }

    /// The actions of row `row`.
    fn row_actions(&self, row: usize) -> &[ListAction] {
        match self.rows.get(row) {
            Some(Row::Group(g)) => &self.list.groups[*g].actions,
            Some(Row::Item(g, i)) => &self.list.groups[*g].items[*i].actions,
            None => &[],
        }
    }

    fn selected_actions(&self) -> &[ListAction] {
        self.row_actions(self.selected)
    }

    /// The text of row `row`, as copied.
    fn row_text(&self, row: usize) -> Option<String> {
        let (label, detail) = match *self.rows.get(row)? {
            Row::Group(g) => (&self.list.groups[g].label, &self.list.groups[g].detail),
            Row::Item(g, i) => {
                let item = &self.list.groups[g].items[i];
                (&item.label, &item.detail)
            }
        };
        Some(if detail.is_empty() {
            label.clone()
        } else {
            format!("{label} {detail}")
        })
    }

    fn send(&self, action: &str) -> Vec<PanelEvent> {
        vec![PanelEvent::PluginAction {
            panel: self.id,
            action: action.to_string(),
        }]
    }

    /// Run the selected row's action at `index`.
    fn run(&self, index: usize) -> Vec<PanelEvent> {
        match self.selected_actions().get(index) {
            Some(action) => self.send(&action.action),
            None => vec![],
        }
    }

    /// The key text of `action`, for the status bar and the actions list.
    fn key_text(&self, index: usize, action: &ListAction) -> String {
        let named = action
            .key
            .map(|key| self.hotkeys.display(key.name()))
            .filter(|text| !text.is_empty());
        match (index, named) {
            (0, Some(text)) => format!("Enter / {text}"),
            (0, None) => "Enter".to_string(),
            (_, named) => named.unwrap_or_default(),
        }
    }

    /// List the selected row's actions to pick from.
    fn offer_actions(&self) -> Vec<PanelEvent> {
        let actions = self.selected_actions();
        if actions.is_empty() {
            return vec![];
        }
        let options = actions
            .iter()
            .enumerate()
            .map(|(index, action)| match self.key_text(index, action) {
                key if key.is_empty() => action.label.clone(),
                key => format!("{} ({key})", action.label),
            })
            .collect();
        let title = self
            .row_text(self.selected)
            .map_or_else(String::new, |text| truncate_right(&text, 60));
        vec![PanelEvent::ShowSelect {
            title,
            options,
            on_select: SelectAction::Custom(ACTIONS_PICK.to_string()),
        }]
    }

    fn ensure_visible(&mut self) {
        let height = self.last_height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(height));
    }

    fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.ensure_visible();
    }

    fn wheel(&mut self, delta: isize) {
        let height = self.last_height.max(1);
        let max = self.rows.len().saturating_sub(height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
        let last = self.rows.len().saturating_sub(1);
        self.selected = self
            .selected
            .clamp(self.scroll, (self.scroll + height - 1).min(last));
    }

    fn draw_row(&self, row: usize, y: u16, area: Rect, buf: &mut Buffer, selected: bool) {
        let theme = &self.theme;
        let base = if selected {
            Style::default().bg(theme.selected_bg).fg(theme.selected_fg)
        } else {
            Style::default().bg(theme.bg).fg(theme.fg)
        };
        let dim = if selected {
            base
        } else {
            base.fg(theme.accented_fg)
        };
        let right = area.right().saturating_sub(1);
        for x in area.left()..area.right() {
            buf[(x, y)].set_style(base);
            buf[(x, y)].set_char(' ');
        }
        let mut x = area.left() + 1;
        let (label, detail, label_style) = match self.rows[row] {
            Row::Group(g) => {
                let group = &self.list.groups[g];
                (
                    &group.label,
                    &group.detail,
                    base.add_modifier(Modifier::BOLD),
                )
            }
            Row::Item(g, i) => {
                let item = &self.list.groups[g].items[i];
                x += 2;
                let mut style = base;
                if let Some(status) = item.status {
                    let colour = base.patch(status.style(theme));
                    x = put(buf, x, y, right, status.letter(), colour);
                    x = put(buf, x, y, right, " ", base);
                    // The selection's own colours keep the label readable.
                    if !selected {
                        style = colour;
                    }
                }
                (&item.label, &item.detail, style)
            }
        };
        x = put(buf, x, y, right, label, label_style);
        if !detail.is_empty() && x + 2 < right {
            x = put(buf, x, y, right, "  ", base);
            put(buf, x, y, right, detail, dim);
        }
    }
}

/// Write `text` from `x` on row `y`, cut short of `right`; where it ends.
fn put(buf: &mut Buffer, x: u16, y: u16, right: u16, text: &str, style: Style) -> u16 {
    let room = right.saturating_sub(x) as usize;
    if room == 0 {
        return x;
    }
    let text = truncate_right(text, room);
    buf.set_stringn(x, y, &text, room, style);
    x + text.width() as u16
}

impl Panel for PluginListPanel {
    fn name(&self) -> &'static str {
        "plugin_list"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon(&self) -> Option<&str> {
        Some("🧩")
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferNarrow
    }

    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        self.theme = *theme;
        self.vim_mode = config.general.vim_mode;
        if self
            .config
            .as_ref()
            .is_some_and(|seen| Arc::ptr_eq(seen, config))
        {
            return;
        }
        self.config = Some(Arc::clone(config));
        let kb = &config.git_status.keybindings;
        let mut hotkeys = HotkeyTable::new();
        hotkeys.insert(ActionKey::View.name(), &kb.view);
        hotkeys.insert(ActionKey::Edit.name(), &kb.edit);
        hotkeys.insert(ActionKey::Revert.name(), &kb.revert);
        hotkeys.insert("info", &kb.info);
        hotkeys.insert("refresh", &kb.refresh);
        self.hotkeys = hotkeys;
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_height = area.height as usize;
        let theme = self.theme;
        let bg = Style::default().bg(theme.bg).fg(theme.fg);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_style(bg);
                buf[(x, y)].set_char(' ');
            }
        }
        if self.rows.is_empty() {
            if area.height > 0 {
                let dim = bg.fg(theme.accented_fg);
                put(
                    buf,
                    area.left() + 1,
                    area.top(),
                    area.right(),
                    &self.list.empty,
                    dim,
                );
            }
            self.scrollbars.vertical = None;
            return;
        }
        self.ensure_visible();
        let height = area.height as usize;
        let end = (self.scroll + height).min(self.rows.len());
        for (line, row) in (self.scroll..end).enumerate() {
            let selected = ctx.is_focused && row == self.selected;
            self.draw_row(row, area.top() + line as u16, area, buf, selected);
        }
        self.scrollbars.vertical = None;
        if self.rows.len() > height && area.width > 2 {
            self.scrollbars.vertical = ScrollBar::render_tracked(
                buf,
                area.right() - 1,
                area.top(),
                area.height,
                self.scroll,
                height,
                self.rows.len(),
                &ThemeColors::from(&theme),
                ctx.is_focused,
            );
        }
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        // No text input here: every key is a shortcut, matched on the
        // layout-normalized form so it works on any keyboard layout.
        let key = chord.canonical;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if let Some(text) = self.row_text(self.selected) {
                let _ = termide_clipboard::copy(&text);
            }
            return vec![];
        }
        if self.hotkeys.matches_canonical("refresh", &key) {
            return match self.list.refresh.clone() {
                Some(action) => self.send(&action),
                None => vec![],
            };
        }
        for named in [ActionKey::View, ActionKey::Edit, ActionKey::Revert] {
            if self.hotkeys.matches_canonical(named.name(), &key) {
                let index = self
                    .selected_actions()
                    .iter()
                    .position(|action| action.key == Some(named));
                return index.map_or_else(Vec::new, |index| self.run(index));
            }
        }
        if self.hotkeys.matches_canonical("info", &key) {
            return self.offer_actions();
        }
        if is_move_up(&key, self.vim_mode) {
            self.move_by(-1);
        } else if is_move_down(&key, self.vim_mode) {
            self.move_by(1);
        } else if is_go_home(&key, self.vim_mode) {
            self.selected = 0;
            self.ensure_visible();
        } else if is_go_end(&key, self.vim_mode) {
            self.selected = self.rows.len().saturating_sub(1);
            self.ensure_visible();
        } else {
            let page = self.last_height.saturating_sub(1).max(1) as isize;
            match key.code {
                KeyCode::PageUp => self.move_by(-page),
                KeyCode::PageDown => self.move_by(page),
                KeyCode::Enter => return self.run(0),
                _ => {}
            }
        }
        vec![]
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> Vec<PanelEvent> {
        let row = || {
            (mouse.row >= area.top() && mouse.row < area.bottom())
                .then(|| self.scroll + (mouse.row - area.top()) as usize)
                .filter(|&row| row < self.rows.len())
        };
        match mouse.kind {
            MouseEventKind::ScrollUp => self.wheel(-3),
            MouseEventKind::ScrollDown => self.wheel(3),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(row) = row() {
                    self.selected = row;
                    if self.clicks.click(row) == 2 {
                        return self.run(0);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(row) = row() {
                    self.selected = row;
                    return self.offer_actions();
                }
            }
            _ => {}
        }
        vec![]
    }

    fn handle_scroll(&mut self, delta: i32, _area: Rect) -> Vec<PanelEvent> {
        self.wheel(delta as isize * 3);
        vec![]
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        std::mem::take(&mut self.pending)
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { offset, .. } => {
                self.scroll = offset;
                self.wheel(0);
                CommandResult::NeedsRedraw(true)
            }
            PanelCommand::SelectionMade { action, index } if action == ACTIONS_PICK => {
                let events = self.run(index);
                self.pending.extend(events);
                CommandResult::Handled(true)
            }
            _ => CommandResult::None,
        }
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        let mut segments = vec![StatusSegment::new(" ", SegmentKind::Label)];
        for (index, action) in self.selected_actions().iter().enumerate() {
            if index > 0 {
                segments.push(StatusSegment::new(" │ ", SegmentKind::Label));
            }
            segments.push(StatusSegment::clickable(
                action.label.clone(),
                SegmentKind::Active,
                ROW_CHIPS[index],
            ));
            let key = self.key_text(index, action);
            if !key.is_empty() {
                segments.push(StatusSegment::new(format!(" {key}"), SegmentKind::Label));
            }
        }
        segments
    }

    fn context_menu_items(&self) -> Vec<(String, String)> {
        self.menu
            .iter()
            .map(|(label, action)| (label.clone(), format!("{MENU_PREFIX}{action}")))
            .collect()
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        if let Some(action) = action.strip_prefix(MENU_PREFIX) {
            return self.send(action);
        }
        match ROW_CHIPS.iter().position(|chip| *chip == action) {
            Some(index) => self.run(index),
            None => vec![],
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn action(label: &str, action: &str, key: Option<ActionKey>) -> ListAction {
        ListAction {
            label: label.into(),
            action: action.into(),
            key,
        }
    }

    fn item(label: &str) -> ListItem {
        ListItem {
            label: label.into(),
            detail: "edit".into(),
            status: Some(ItemStatus::Modified),
            actions: vec![
                action("Open", &format!("open {label}"), None),
                action("Diff", &format!("diff {label}"), Some(ActionKey::View)),
                action(
                    "Revert",
                    &format!("revert {label}"),
                    Some(ActionKey::Revert),
                ),
            ],
        }
    }

    fn list() -> PanelList {
        PanelList {
            groups: vec![
                ListGroup {
                    label: "Default changelist".into(),
                    actions: vec![action("Submit", "submit default", None)],
                    items: vec![item("a.c"), item("b.c")],
                    ..ListGroup::default()
                },
                ListGroup {
                    label: "Change 7".into(),
                    detail: "fix".into(),
                    items: vec![item("c.c")],
                    ..ListGroup::default()
                },
            ],
            empty: "Nothing".into(),
            refresh: Some("refresh".into()),
        }
    }

    fn panel() -> PluginListPanel {
        let mut panel = PluginListPanel::new(3, "p4: Opened".into(), list());
        let mut config = Config::default();
        config.git_status.keybindings.with_defaults();
        panel.prepare_render(&Theme::default(), &Arc::new(config));
        panel
    }

    fn press(
        panel: &mut PluginListPanel,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Vec<PanelEvent> {
        panel.handle_key(KeyChord::new(
            KeyEvent::new(code, modifiers),
            &termide_core::KeyNormalizer::default(),
        ))
    }

    fn sent(events: &[PanelEvent]) -> Option<&str> {
        match events {
            [PanelEvent::PluginAction { panel: 3, action }] => Some(action),
            _ => None,
        }
    }

    #[test]
    fn rows_are_the_groups_and_their_items_in_order() {
        let panel = panel();
        assert_eq!(
            panel.rows,
            vec![
                Row::Group(0),
                Row::Item(0, 0),
                Row::Item(0, 1),
                Row::Group(1),
                Row::Item(1, 0)
            ]
        );
    }

    #[test]
    fn enter_runs_the_first_action_and_named_keys_their_own() {
        let mut panel = panel();
        assert_eq!(
            sent(&press(&mut panel, KeyCode::Enter, KeyModifiers::NONE)),
            Some("submit default")
        );
        press(&mut panel, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(
            sent(&press(&mut panel, KeyCode::Enter, KeyModifiers::NONE)),
            Some("open a.c")
        );
        assert_eq!(
            sent(&press(&mut panel, KeyCode::F(3), KeyModifiers::NONE)),
            Some("diff a.c")
        );
        assert_eq!(
            sent(&press(&mut panel, KeyCode::Backspace, KeyModifiers::NONE)),
            Some("revert a.c")
        );
        assert!(
            press(&mut panel, KeyCode::F(4), KeyModifiers::NONE).is_empty(),
            "no edit action"
        );
        assert_eq!(
            sent(&press(
                &mut panel,
                KeyCode::Char('r'),
                KeyModifiers::CONTROL
            )),
            Some("refresh")
        );
    }

    #[test]
    fn the_info_key_lists_the_actions_and_the_pick_runs_one() {
        let mut panel = panel();
        press(&mut panel, KeyCode::Down, KeyModifiers::NONE);
        let events = press(&mut panel, KeyCode::Char(' '), KeyModifiers::NONE);
        let [PanelEvent::ShowSelect { options, .. }] = events.as_slice() else {
            panic!("no list: {events:?}");
        };
        assert_eq!(
            options,
            &["Open (Enter)", "Diff (F3)", "Revert (Backspace / Delete)"]
        );
        panel.handle_command(PanelCommand::SelectionMade {
            action: ACTIONS_PICK.into(),
            index: 2,
        });
        assert_eq!(sent(&panel.tick()), Some("revert a.c"));
    }

    #[test]
    fn an_update_keeps_the_selected_row_where_it_went() {
        let mut panel = panel();
        panel.move_by(4);
        assert_eq!(panel.row_text(panel.selected).as_deref(), Some("c.c edit"));
        let mut shorter = list();
        shorter.groups[0].items.remove(0);
        panel.set_list(None, shorter);
        assert_eq!(panel.row_text(panel.selected).as_deref(), Some("c.c edit"));

        panel.set_list(None, PanelList::default());
        assert_eq!(panel.selected, 0);
        assert!(panel.selected_actions().is_empty());
    }

    #[test]
    fn a_stopped_plugin_leaves_nothing_to_do() {
        let mut panel = panel();
        panel.set_plugin_menu(vec![("Refresh".into(), "refresh".into())]);
        panel.set_stopped("The plugin p4 stopped.".into());
        assert!(panel.context_menu_items().is_empty());
        assert!(press(&mut panel, KeyCode::Enter, KeyModifiers::NONE).is_empty());
        assert!(press(&mut panel, KeyCode::Char('r'), KeyModifiers::CONTROL).is_empty());
    }

    #[test]
    fn status_chips_run_the_selected_rows_actions() {
        let mut panel = panel();
        press(&mut panel, KeyCode::Down, KeyModifiers::NONE);
        let chips: Vec<_> = panel
            .status_segments()
            .into_iter()
            .filter_map(|segment| segment.action.map(|_| segment.text))
            .collect();
        assert_eq!(chips, ["Open", "Diff", "Revert"]);
        assert_eq!(
            sent(&panel.handle_status_action(ROW_CHIPS[1])),
            Some("diff a.c")
        );
        assert_eq!(
            sent(&panel.handle_status_action("plugin-menu:sync")),
            Some("sync")
        );
    }
}
