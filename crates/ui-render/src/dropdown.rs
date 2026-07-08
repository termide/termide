//! Dropdown menu widget.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear},
};

use termide_core::ThemeColors;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::{render_text_cells, str_display_width, ScrollBar};

/// Dropdown menu item
#[derive(Debug, Clone)]
pub struct DropdownItem {
    pub label: String,
    pub key: String,
    /// Whether this item opens a submenu
    pub has_submenu: bool,
    /// Whether this item is a separator line (not selectable)
    pub is_separator: bool,
    /// Whether this item comes from a project-local .termide/ directory (rendered bold)
    pub is_project: bool,
}

impl DropdownItem {
    pub fn new(label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: key.into(),
            has_submenu: false,
            is_separator: false,
            is_project: false,
        }
    }

    /// Create a separator item (horizontal line, not selectable)
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            key: String::new(),
            has_submenu: false,
            is_separator: true,
            is_project: false,
        }
    }

    /// Mark this item as having a submenu
    pub fn with_submenu(mut self) -> Self {
        self.has_submenu = true;
        self
    }

    /// Mark this item as project-local (rendered bold)
    pub fn with_project(mut self) -> Self {
        self.is_project = true;
        self
    }
}

/// Maximum visible items in dropdown before scrolling
const MAX_VISIBLE_ITEMS: usize = 20;

/// Dropdown menu
pub struct Dropdown<'a> {
    items: &'a [DropdownItem],
    selected: usize,
    x: u16,
    y: u16,
    theme: &'a Theme,
    max_visible: usize,
    scroll_offset: usize,
}

impl<'a> Dropdown<'a> {
    pub fn new(
        items: &'a [DropdownItem],
        selected: usize,
        x: u16,
        y: u16,
        theme: &'a Theme,
    ) -> Self {
        let max_visible = MAX_VISIBLE_ITEMS.min(items.len());
        // Calculate scroll offset to keep selected item visible
        let scroll_offset = if selected >= max_visible {
            selected - max_visible + 1
        } else {
            0
        };

        Self {
            items,
            selected,
            x,
            y,
            theme,
            max_visible,
            scroll_offset,
        }
    }

    /// Get the width of this dropdown
    pub fn width(&self) -> u16 {
        let max_label_len = self
            .items
            .iter()
            .map(|item| str_display_width(&item.label))
            .max()
            .unwrap_or(0);
        // 2 (borders) + 1 (space) + label + 3 (" ▶ ") = label + 6
        (max_label_len + 6).min(40) as u16
    }

    /// Get the height of this dropdown
    pub fn height(&self) -> u16 {
        let visible_count = self.items.len().min(self.max_visible);
        (visible_count + 2) as u16 // +2 for borders
    }

    pub fn render(&self, buf: &mut Buffer) {
        if self.items.is_empty() {
            return;
        }

        // Clamp to the buffer so a dropdown taller/wider than the terminal
        // never renders past its edges (that panics ratatui — issue #25). The
        // per-row item loop below already stops at the inner height, so a
        // shrunken box just shows fewer rows.
        let width = self.width().min(buf.area.width).max(1);
        let height = self.height().min(buf.area.height).max(1);

        // Check screen boundaries
        let max_x = buf.area.width.saturating_sub(width);
        let max_y = buf.area.height.saturating_sub(height);
        let x = self.x.min(max_x);
        let y = self.y.min(max_y);

        let area = Rect {
            x,
            y,
            width,
            height,
        };

        // Clear area under dropdown
        Clear.render(area, buf);

        // Render border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.accented_fg))
            .style(Style::default().bg(self.theme.bg));
        block.render(area, buf);

        // Inner area (without border)
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        // Fill inner background
        for row in inner.y..inner.y + inner.height {
            for col in inner.x..inner.x + inner.width {
                buf[(col, row)].set_style(Style::default().bg(self.theme.bg));
            }
        }

        // Get visible items
        let visible_end = (self.scroll_offset + self.max_visible).min(self.items.len());
        let visible_items = &self.items[self.scroll_offset..visible_end];

        // Render rows
        for (i, item) in visible_items.iter().enumerate() {
            let actual_index = self.scroll_offset + i;
            let is_selected = actual_index == self.selected;

            let row_y = inner.y + i as u16;
            if row_y >= inner.y + inner.height {
                break;
            }

            // Separator: draw horizontal line, never highlighted
            if item.is_separator {
                let sep_style = Style::default().fg(self.theme.disabled).bg(self.theme.bg);
                for col in inner.x..inner.x + inner.width {
                    buf[(col, row_y)].set_style(sep_style);
                }
                let line = "─".repeat(inner.width.saturating_sub(2) as usize);
                render_text_cells(
                    buf,
                    inner.x + 1,
                    row_y,
                    &line,
                    inner.width.saturating_sub(2),
                    sep_style,
                );
                continue;
            }

            let base_style = if is_selected {
                Style::default()
                    .bg(self.theme.selected_bg)
                    .fg(self.theme.selected_fg)
                    .add_modifier(Modifier::BOLD)
            } else if item.is_project {
                Style::default()
                    .fg(self.theme.fg)
                    .bg(self.theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.fg).bg(self.theme.bg)
            };

            // Fill row background
            for col in inner.x..inner.x + inner.width {
                buf[(col, row_y)].set_style(base_style);
            }

            // " " + label
            let mut cursor_x = inner.x;
            cursor_x += render_text_cells(buf, cursor_x, row_y, " ", inner.width, base_style);
            let label_width = str_display_width(&item.label) as u16;
            cursor_x += render_text_cells(
                buf,
                cursor_x,
                row_y,
                &item.label,
                inner.width.saturating_sub(cursor_x - inner.x),
                base_style,
            );

            // Suffix " ▶ "/" ► " or "   " — always 3 columns wide, right-aligned
            const SUBMENU_ARROW: &str = if cfg!(windows) { " ► " } else { " ▶ " };
            let suffix = if item.has_submenu {
                SUBMENU_ARROW
            } else {
                "   "
            };
            let suffix_x = inner.x + inner.width.saturating_sub(3);
            render_text_cells(buf, suffix_x, row_y, suffix, 3, base_style);
            let _ = (cursor_x, label_width); // suppress warnings
        }

        // Render scrollbar on right edge (inside border)
        let visible_count = self.items.len().min(self.max_visible);
        let theme_colors = ThemeColors::from(self.theme);
        ScrollBar::render(
            buf,
            x + width - 1,            // Right border position
            y + 1,                    // Inside top border
            height.saturating_sub(2), // Inside borders
            self.scroll_offset,
            visible_count,
            self.items.len(),
            &theme_colors,
            true, // Dropdown is always focused when visible
        );
    }
}

/// Get sessions submenu items
pub fn get_sessions_items() -> Vec<DropdownItem> {
    let t = i18n::t();
    vec![
        DropdownItem::new(t.sessions_new(), "new_session"),
        DropdownItem::new(t.sessions_switch(), "switch_session"),
        DropdownItem::new(t.sessions_change_root(), "change_root"),
    ]
}

/// Number of items in Sessions submenu
pub const SESSIONS_SUBMENU_ITEM_COUNT: usize = 3;

/// Index of Sessions submenu items
pub const SESSIONS_SUBMENU_NEW: usize = 0;
pub const SESSIONS_SUBMENU_SWITCH: usize = 1;
pub const SESSIONS_SUBMENU_CHANGE_ROOT: usize = 2;

/// Get tools submenu items
pub fn get_tools_items() -> Vec<DropdownItem> {
    let t = i18n::t();
    vec![
        DropdownItem::new(t.tools_open(), "open"),
        DropdownItem::separator(),
        DropdownItem::new(t.tools_terminal(), "terminal").with_submenu(),
        DropdownItem::new(t.tools_files(), "files"),
        DropdownItem::new(t.tools_editor(), "editor"),
        DropdownItem::new(t.tools_git_status(), "git_status"),
        DropdownItem::new(t.tools_git_log(), "git_log"),
        DropdownItem::new(t.tools_journal(), "journal"),
        DropdownItem::new(t.tools_diagnostics(), "diagnostics"),
        DropdownItem::new(t.tools_operations(), "operations"),
        DropdownItem::new(t.tools_outline(), "outline"),
    ]
}

/// Number of items in Tools submenu (including the separator row).
pub const TOOLS_SUBMENU_ITEM_COUNT: usize = 11;

/// Index of the (non-selectable) separator row in the Tools submenu.
pub const TOOLS_SUBMENU_SEPARATOR: usize = 1;

/// Index of Tools submenu items
pub const TOOLS_SUBMENU_OPEN: usize = 0;
pub const TOOLS_SUBMENU_TERMINAL: usize = 2;
pub const TOOLS_SUBMENU_FILES: usize = 3;
pub const TOOLS_SUBMENU_EDITOR: usize = 4;
pub const TOOLS_SUBMENU_GIT_STATUS: usize = 5;
pub const TOOLS_SUBMENU_GIT_LOG: usize = 6;
pub const TOOLS_SUBMENU_JOURNAL: usize = 7;
pub const TOOLS_SUBMENU_DIAGNOSTICS: usize = 8;
pub const TOOLS_SUBMENU_OPERATIONS: usize = 9;
pub const TOOLS_SUBMENU_OUTLINE: usize = 10;

/// Get shell picker submenu items from discovered shells.
///
/// Marks the default shell with a `●` indicator.
pub fn get_shell_items(
    shells: &[termide_panel_terminal::shell_utils::ShellInfo],
    default_shell: Option<&str>,
) -> Vec<DropdownItem> {
    shells
        .iter()
        .map(|shell| {
            let is_default = default_shell.map(|d| d == shell.path).unwrap_or(false);
            let label = if is_default {
                format!("{} ●", shell.name)
            } else {
                shell.name.clone()
            };
            DropdownItem::new(label, &shell.path)
        })
        .collect()
}

/// Get options submenu items
pub fn get_options_items() -> Vec<DropdownItem> {
    let t = i18n::t();
    vec![
        DropdownItem::new(t.preferences_themes(), "themes").with_submenu(),
        DropdownItem::new(t.preferences_language(), "language").with_submenu(),
        DropdownItem::new(t.preferences_edit(), "edit_preferences"),
        DropdownItem::new(t.options_help(), "help"),
        DropdownItem::new(t.menu_quit(), "quit"),
    ]
}

/// Number of items in Options submenu
pub const OPTIONS_SUBMENU_ITEM_COUNT: usize = 5;

/// Index of Options submenu items
pub const OPTIONS_SUBMENU_THEMES: usize = 0;
pub const OPTIONS_SUBMENU_LANGUAGE: usize = 1;
pub const OPTIONS_SUBMENU_PREFERENCES: usize = 2;
pub const OPTIONS_SUBMENU_HELP: usize = 3;
pub const OPTIONS_SUBMENU_QUIT: usize = 4;

/// Special command ID for "Add command..." menu item
pub const COMMAND_ADD_NEW: &str = "__add_command__";
/// Special ID for "Manage commands" menu item
pub const COMMAND_MANAGE: &str = "__manage_commands__";

/// Get commands submenu items from CommandsRegistry
/// Format command label with type icon prefix (when terminal supports emoji).
/// 💻 = runs in terminal panel, ⚙ = background, 📋 = background with result modal
fn command_label(command: &termide_config::commands::CommandItem) -> String {
    use termide_config::commands::CommandMode;

    let display_name = command
        .metadata
        .as_ref()
        .and_then(|m| m.display_name.as_deref())
        .unwrap_or(&command.name);

    let key_hint = command
        .metadata
        .as_ref()
        .and_then(|m| m.key.as_deref())
        .map(|k| format!(" [{}]", k))
        .unwrap_or_default();

    if termide_core::use_emoji_icons() {
        let icon = match command.mode {
            CommandMode::Report => "📋",
            CommandMode::Background => "⚙",
            CommandMode::Terminal => "💻",
        };
        format!("{} {}{}", icon, display_name, key_hint)
    } else {
        format!("{}{}", display_name, key_hint)
    }
}

pub fn get_commands_items(
    registry: &termide_config::commands::CommandsRegistry,
) -> Vec<DropdownItem> {
    use termide_config::commands::{encode_command_menu_key, CommandMenuKeyKind};

    let t = i18n::t();
    let mut items = vec![
        DropdownItem::new(t.menu_commands_add(), COMMAND_ADD_NEW),
        DropdownItem::separator(),
    ];

    let has_project = registry.root_items.iter().any(|s| s.is_project)
        || registry.groups.iter().any(|g| g.is_project);
    let has_global = registry.root_items.iter().any(|s| !s.is_project)
        || registry.groups.iter().any(|g| !g.is_project);

    // Project commands first (bold)
    for command in registry.root_items.iter().filter(|s| s.is_project) {
        items.push(
            DropdownItem::new(
                command_label(command),
                encode_command_menu_key(CommandMenuKeyKind::Command, &command.name, true),
            )
            .with_project(),
        );
    }
    for group in registry.groups.iter().filter(|g| g.is_project) {
        items.push(
            DropdownItem::new(
                &group.name,
                encode_command_menu_key(CommandMenuKeyKind::Group, &group.name, true),
            )
            .with_submenu()
            .with_project(),
        );
    }

    // Separator between project and global
    if has_project && has_global {
        items.push(DropdownItem::separator());
    }

    // Global commands
    for command in registry.root_items.iter().filter(|s| !s.is_project) {
        items.push(DropdownItem::new(
            command_label(command),
            encode_command_menu_key(CommandMenuKeyKind::Command, &command.name, false),
        ));
    }
    for group in registry.groups.iter().filter(|g| !g.is_project) {
        items.push(
            DropdownItem::new(
                &group.name,
                encode_command_menu_key(CommandMenuKeyKind::Group, &group.name, false),
            )
            .with_submenu(),
        );
    }

    items
}

/// Get commands nested submenu items for a specific group
pub fn get_commands_group_items(
    registry: &termide_config::commands::CommandsRegistry,
    group_key: &str,
) -> Vec<DropdownItem> {
    let Some(decoded) = termide_config::commands::decode_command_menu_key(group_key) else {
        return Vec::new();
    };
    if decoded.kind != termide_config::commands::CommandMenuKeyKind::Group {
        return Vec::new();
    }

    registry
        .find_group(&decoded.name, decoded.is_project)
        .map(|group| {
            group
                .items
                .iter()
                .map(|command| {
                    let mut item = DropdownItem::new(
                        command_label(command),
                        termide_config::commands::encode_command_menu_key(
                            termide_config::commands::CommandMenuKeyKind::Command,
                            &command.name,
                            command.is_project,
                        ),
                    );
                    if command.is_project {
                        item = item.with_project();
                    }
                    item
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Special bookmark action IDs
pub const BOOKMARK_ADD_CURRENT: &str = "__bookmark_add__";

/// Format bookmark label with type icon prefix (when terminal supports emoji)
fn bookmark_label(bookmark: &termide_config::Bookmark) -> String {
    if termide_core::use_emoji_icons() {
        let icon = bookmark.bookmark_type().icon();
        format!("{} {}", icon, bookmark.display_name())
    } else {
        bookmark.display_name().to_string()
    }
}

/// Get bookmarks submenu items from BookmarksConfig and optional project bookmarks
pub fn get_bookmarks_items(
    config: &termide_config::BookmarksConfig,
    project_config: Option<&termide_config::BookmarksConfig>,
) -> Vec<DropdownItem> {
    let t = i18n::t();
    let mut items = vec![
        DropdownItem::new(t.bookmarks_add_bookmark(), BOOKMARK_ADD_CURRENT),
        DropdownItem::separator(),
    ];

    let group_icon = if termide_core::use_emoji_icons() {
        "📂 "
    } else {
        ""
    };

    // Add project bookmarks first (bold)
    if let Some(proj) = project_config {
        for group_name in proj.named_groups().keys() {
            items.push(
                DropdownItem::new(format!("{group_icon}{group_name}"), group_name.as_str())
                    .with_submenu()
                    .with_project(),
            );
        }
        for bookmark in proj.ungrouped() {
            items.push(DropdownItem::new(bookmark_label(bookmark), &bookmark.path).with_project());
        }
        if !proj.is_empty() && !config.is_empty() {
            items.push(DropdownItem::separator());
        }
    }

    if config.is_empty() && project_config.is_none_or(|p| p.is_empty()) {
        items.push(DropdownItem::new(t.bookmarks_no_bookmarks(), ""));
        return items;
    }

    // Add global named groups (as submenus)
    let named_groups = config.named_groups();
    for group_name in named_groups.keys() {
        items.push(
            DropdownItem::new(format!("{group_icon}{group_name}"), group_name.as_str())
                .with_submenu(),
        );
    }

    // Add global ungrouped bookmarks directly in menu
    for bookmark in config.ungrouped() {
        items.push(DropdownItem::new(bookmark_label(bookmark), &bookmark.path));
    }

    items
}

/// Get bookmarks count for determining submenu item count
pub fn get_bookmarks_item_count(
    config: &termide_config::BookmarksConfig,
    project_config: Option<&termide_config::BookmarksConfig>,
) -> usize {
    let project_count = project_config.map_or(0, |p| {
        let separator = if !p.is_empty() && !config.is_empty() {
            1
        } else {
            0
        };
        p.named_groups().len() + p.ungrouped().len() + separator
    });
    if config.is_empty() && project_config.is_none_or(|p| p.is_empty()) {
        3 // add + separator + no_bookmarks
    } else {
        2 + project_count + config.named_groups().len() + config.ungrouped().len()
    }
}

/// Key for "New stash..." item
pub const STASH_NEW: &str = "__stash_new__";

/// Generate dropdown items for stash list.
/// `has_changes` controls whether "New stash..." is shown.
pub fn get_stash_items(
    entries: &[termide_git::StashEntry],
    has_changes: bool,
) -> Vec<DropdownItem> {
    let t = termide_i18n::t();
    let mut items = Vec::new();

    if has_changes {
        items.push(DropdownItem::new(t.stash_new(), STASH_NEW));
        if !entries.is_empty() {
            items.push(DropdownItem::separator());
        }
    }

    for entry in entries {
        items.push(DropdownItem::new(&entry.message, &entry.ref_str));
    }
    items
}

/// Key prefix for panel action context menu items.
pub const PANEL_ACTION_CLOSE: &str = "__panel_action_close__";
pub const PANEL_ACTION_SPLIT: &str = "__panel_action_split__";
pub const PANEL_ACTION_MOVE_LEFT: &str = "__panel_action_move_left__";
pub const PANEL_ACTION_MOVE_RIGHT: &str = "__panel_action_move_right__";
pub const PANEL_ACTION_MOVE_UP: &str = "__panel_action_move_up__";
pub const PANEL_ACTION_MOVE_DOWN: &str = "__panel_action_move_down__";

/// Build the list of items for the panel action context menu (the dropdown
/// opened from the `[≡]` button on a panel header). The list is filtered by
/// the number of groups and the number of panels in the current group.
///
/// The "toggle stack" entry shows either "Split" (when the current group has
/// more than one panel, since the action will unstack the current one) or
/// "Merge" (when the group has one panel and will be merged with a
/// neighbour). If there is nothing to split or merge (single panel, single
/// group) the entry is omitted.
pub fn get_panel_action_menu_items(
    group_count: usize,
    current_group_len: usize,
) -> Vec<DropdownItem> {
    let t = termide_i18n::t();
    let mut items = Vec::new();

    if current_group_len > 1 {
        items.push(DropdownItem::new(
            t.panel_action_move_up(),
            PANEL_ACTION_MOVE_UP,
        ));
        items.push(DropdownItem::new(
            t.panel_action_move_down(),
            PANEL_ACTION_MOVE_DOWN,
        ));
    }

    if group_count > 1 {
        items.push(DropdownItem::new(
            t.panel_action_move_left(),
            PANEL_ACTION_MOVE_LEFT,
        ));
        items.push(DropdownItem::new(
            t.panel_action_move_right(),
            PANEL_ACTION_MOVE_RIGHT,
        ));
    }

    if current_group_len > 1 {
        items.push(DropdownItem::new(
            t.panel_action_split(),
            PANEL_ACTION_SPLIT,
        ));
    } else if group_count > 1 {
        items.push(DropdownItem::new(
            t.panel_action_merge(),
            PANEL_ACTION_SPLIT,
        ));
    }

    items.push(DropdownItem::new(
        t.panel_action_close(),
        PANEL_ACTION_CLOSE,
    ));

    items
}

/// Compute the top-left corner for the panel action dropdown given its
/// anchor (the panel's `[≡]` button position), clamping to screen bounds so
/// it stays visible on narrow terminals. The dropdown is placed one row
/// below the anchor.
pub fn panel_action_dropdown_position(
    items: &[DropdownItem],
    anchor_x: u16,
    anchor_y: u16,
    screen_w: u16,
    screen_h: u16,
) -> (u16, u16) {
    let width = items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(0) as u16
        + 6;
    let height = items.len() as u16 + 2;
    let x = anchor_x.min(screen_w.saturating_sub(width));
    let y = anchor_y
        .saturating_add(1)
        .min(screen_h.saturating_sub(height));
    (x, y)
}

// ────────────────────────────────────────────────────────────────────
// Operation action menu (Pause/Resume/Cancel on an ops-panel card)
// ────────────────────────────────────────────────────────────────────

pub const OPERATION_ACTION_PAUSE: &str = "__operation_action_pause__";
pub const OPERATION_ACTION_RESUME: &str = "__operation_action_resume__";
pub const OPERATION_ACTION_CANCEL: &str = "__operation_action_cancel__";

/// Build items for the per-operation popup menu opened from the icon
/// on an operations-panel card. The first item swaps label based on the
/// current pause state. Command/script runs (`is_command`) can't be paused —
/// there's no way to suspend an external process — so Pause/Resume is omitted
/// for them and only Cancel (which kills the process) is offered.
pub fn get_operation_action_menu_items(is_paused: bool, is_command: bool) -> Vec<DropdownItem> {
    let mut items = Vec::with_capacity(2);
    if !is_command {
        if is_paused {
            items.push(DropdownItem::new("Resume", OPERATION_ACTION_RESUME));
        } else {
            items.push(DropdownItem::new("Pause", OPERATION_ACTION_PAUSE));
        }
    }
    items.push(DropdownItem::new("Cancel", OPERATION_ACTION_CANCEL));
    items
}

/// Position calculator for the operation action dropdown — same shape
/// as `panel_action_dropdown_position` so the visual behaviour stays
/// consistent.
pub fn operation_action_dropdown_position(
    items: &[DropdownItem],
    anchor_x: u16,
    anchor_y: u16,
    screen_w: u16,
    screen_h: u16,
) -> (u16, u16) {
    panel_action_dropdown_position(items, anchor_x, anchor_y, screen_w, screen_h)
}

/// Get bookmark items for a specific group
pub fn get_bookmarks_group_items(
    config: &termide_config::BookmarksConfig,
    project_config: Option<&termide_config::BookmarksConfig>,
    group_name: &str,
    is_project_group: bool,
) -> Vec<DropdownItem> {
    if is_project_group {
        project_config
            .and_then(|proj| proj.grouped().get(group_name).cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|b| DropdownItem::new(bookmark_label(b), &b.path).with_project())
            .collect()
    } else {
        config
            .grouped()
            .get(group_name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|b| DropdownItem::new(bookmark_label(b), &b.path))
            .collect()
    }
}

#[cfg(test)]
mod overflow_tests {
    use super::{Dropdown, DropdownItem};
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_theme::Theme;

    // Regression for #25: a menu dropdown with more items than the terminal is
    // tall must clamp to the screen instead of writing past the bottom (which
    // panics ratatui). The crash report had a 57x15 area.
    #[test]
    fn render_does_not_overflow_short_terminal() {
        let theme = Theme::get_by_name("default");
        let items: Vec<DropdownItem> = (0..30)
            .map(|i| DropdownItem::new(format!("item-{i:02}"), "id"))
            .collect();
        let mut buf = Buffer::empty(Rect::new(0, 0, 57, 15));
        Dropdown::new(&items, 29, 33, 1, theme).render(&mut buf);
    }
}

#[cfg(test)]
mod operation_menu_tests {
    use super::{
        get_operation_action_menu_items, OPERATION_ACTION_CANCEL, OPERATION_ACTION_PAUSE,
        OPERATION_ACTION_RESUME,
    };

    fn keys(is_paused: bool, is_command: bool) -> Vec<String> {
        get_operation_action_menu_items(is_paused, is_command)
            .into_iter()
            .map(|i| i.key)
            .collect()
    }

    // File operations are pausable: Pause/Resume + Cancel.
    #[test]
    fn file_operation_offers_pause_and_cancel() {
        assert_eq!(
            keys(false, false),
            vec![OPERATION_ACTION_PAUSE, OPERATION_ACTION_CANCEL]
        );
        assert_eq!(
            keys(true, false),
            vec![OPERATION_ACTION_RESUME, OPERATION_ACTION_CANCEL]
        );
    }

    // Script/command runs can't be paused — only Cancel (which kills the
    // process) is offered, regardless of the (meaningless) paused flag.
    #[test]
    fn command_operation_offers_only_cancel() {
        assert_eq!(keys(false, true), vec![OPERATION_ACTION_CANCEL]);
        assert_eq!(keys(true, true), vec![OPERATION_ACTION_CANCEL]);
    }
}
