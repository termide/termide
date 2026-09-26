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
    /// Keyboard shortcut for this action, shown dimmed on the right.
    pub shortcut: Option<String>,
}

impl DropdownItem {
    pub fn new(label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: key.into(),
            has_submenu: false,
            is_separator: false,
            is_project: false,
            shortcut: None,
        }
    }

    /// Show `shortcut` dimmed on the right of the row. An empty or missing
    /// shortcut leaves the column blank, so unbound actions stay flush.
    pub fn with_shortcut(mut self, shortcut: Option<String>) -> Self {
        self.shortcut = shortcut.filter(|s| !s.is_empty());
        self
    }

    /// Create a separator item (horizontal line, not selectable)
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            key: String::new(),
            has_submenu: false,
            is_separator: true,
            is_project: false,
            shortcut: None,
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
}

/// On-screen width of a dropdown holding `items`, borders included.
///
/// This is the single source of truth for the geometry: the renderer uses it
/// to size a dropdown and to place a nested submenu to its right, and the
/// mouse handlers use it to hit-test clicks. Recomputing it anywhere else lets
/// the click targets drift away from what is drawn.
pub fn dropdown_width(items: &[DropdownItem]) -> u16 {
    let max_label_len = items
        .iter()
        .map(|item| str_display_width(&item.label))
        .max()
        .unwrap_or(0);
    // Shortcuts share the row with the labels, so the widest of each has
    // to fit side by side or the two would overlap.
    let max_shortcut_len = items
        .iter()
        .filter_map(|item| item.shortcut.as_deref())
        .map(str_display_width)
        .max()
        .unwrap_or(0);
    let shortcut_column = if max_shortcut_len == 0 {
        0
    } else {
        max_shortcut_len + 2
    };
    // 2 (borders) + 1 (space) + label + shortcut + 3 (" ▶ ")
    (max_label_len + shortcut_column + 6).min(48) as u16
}

/// Where a dropdown list is actually drawn once it is fitted to the screen.
///
/// Renderers draw from it and mouse handlers hit-test against it, so a click
/// always lands on the row that is displayed under the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListGeometry {
    /// On-screen rectangle, borders included, clamped to the screen.
    pub area: Rect,
    /// Index of the first item shown in the first row.
    pub scroll_offset: usize,
}

impl ListGeometry {
    /// Fit a list of `item_count` rows, `width` columns wide and showing at
    /// most `max_visible` rows, at (`x`, `y`) inside `screen`.
    ///
    /// The box is shrunk to the screen and moved left or up when it would
    /// overflow, and the scroll offset keeps `selected` visible.
    pub fn compute(
        width: u16,
        item_count: usize,
        max_visible: usize,
        selected: usize,
        x: u16,
        y: u16,
        screen: Rect,
    ) -> Self {
        let width = width.min(screen.width).max(1);
        let height = ((item_count.min(max_visible) + 2) as u16)
            .min(screen.height)
            .max(1);
        let visible = height.saturating_sub(2) as usize;
        let scroll_offset = if visible == 0 {
            0
        } else {
            (selected + 1)
                .saturating_sub(visible)
                .min(item_count.saturating_sub(visible))
        };
        let x = x.min(screen.right().saturating_sub(width)).max(screen.x);
        let y = y.min(screen.bottom().saturating_sub(height)).max(screen.y);
        Self {
            area: Rect {
                x,
                y,
                width,
                height,
            },
            scroll_offset,
        }
    }

    /// Number of item rows visible between the borders.
    pub fn visible_count(&self) -> usize {
        self.area.height.saturating_sub(2) as usize
    }

    /// Index of the item under the screen cell (`x`, `y`), or `None` when the
    /// cell is outside the list or on its top or bottom border.
    pub fn item_at(&self, x: u16, y: u16) -> Option<usize> {
        let inside = x >= self.area.x
            && x < self.area.right()
            && y > self.area.y
            && y < self.area.bottom().saturating_sub(1);
        inside.then(|| self.scroll_offset + (y - self.area.y - 1) as usize)
    }
}

/// On-screen geometry of a [`Dropdown`] holding `items`, requested at
/// (`x`, `y`) with `selected` highlighted and fitted to `screen`.
pub fn dropdown_geometry(
    items: &[DropdownItem],
    selected: usize,
    x: u16,
    y: u16,
    screen: Rect,
) -> ListGeometry {
    ListGeometry::compute(
        dropdown_width(items),
        items.len(),
        MAX_VISIBLE_ITEMS,
        selected,
        x,
        y,
        screen,
    )
}

impl<'a> Dropdown<'a> {
    pub fn new(
        items: &'a [DropdownItem],
        selected: usize,
        x: u16,
        y: u16,
        theme: &'a Theme,
    ) -> Self {
        Self {
            items,
            selected,
            x,
            y,
            theme,
        }
    }

    /// Get the width of this dropdown
    pub fn width(&self) -> u16 {
        dropdown_width(self.items)
    }

    /// Get the height of this dropdown
    pub fn height(&self) -> u16 {
        let visible_count = self.items.len().min(MAX_VISIBLE_ITEMS);
        (visible_count + 2) as u16 // +2 for borders
    }

    /// On-screen geometry of this dropdown once fitted to `screen`.
    pub fn geometry(&self, screen: Rect) -> ListGeometry {
        dropdown_geometry(self.items, self.selected, self.x, self.y, screen)
    }

    pub fn render(&self, buf: &mut Buffer) {
        if self.items.is_empty() {
            return;
        }

        let geometry = self.geometry(buf.area);
        let area = geometry.area;
        let Rect {
            x,
            y,
            width,
            height,
        } = area;
        let scroll_offset = geometry.scroll_offset;
        let visible_count = geometry.visible_count();

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
        let visible_end = (scroll_offset + visible_count).min(self.items.len());
        let visible_items = &self.items[scroll_offset..visible_end];

        // Render rows
        for (i, item) in visible_items.iter().enumerate() {
            let actual_index = scroll_offset + i;
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

            // Shortcut, right-aligned against the suffix and dimmed — it is a
            // reminder, not a thing to read first. On the highlighted row the
            // dim colour would sink into the selection background, so the
            // selected foreground is kept instead.
            if let Some(shortcut) = &item.shortcut {
                let shortcut_width = str_display_width(shortcut) as u16;
                let shortcut_x = suffix_x.saturating_sub(shortcut_width);
                if shortcut_x > cursor_x {
                    let style = if is_selected {
                        base_style
                    } else {
                        Style::default().fg(self.theme.disabled).bg(self.theme.bg)
                    };
                    render_text_cells(buf, shortcut_x, row_y, shortcut, shortcut_width, style);
                }
            }

            render_text_cells(buf, suffix_x, row_y, suffix, 3, base_style);
            let _ = label_width; // suppress warnings
        }

        // Render scrollbar on right edge (inside border)
        let theme_colors = ThemeColors::from(self.theme);
        ScrollBar::render(
            buf,
            x + width - 1,            // Right border position
            y + 1,                    // Inside top border
            height.saturating_sub(2), // Inside borders
            scroll_offset,
            visible_count,
            self.items.len(),
            &theme_colors,
            true, // Dropdown is always focused when visible
        );
    }
}

/// Get sessions submenu items
pub fn get_projects_items(kb: Option<&termide_config::GlobalKeybindings>) -> Vec<DropdownItem> {
    let t = i18n::t();
    let shortcut = |key: &str| kb.and_then(|kb| menu_shortcut(kb, key));
    vec![
        DropdownItem::new(t.projects_new(), "new_project").with_shortcut(shortcut("new_project")),
        DropdownItem::new(t.projects_switch(), "switch_session")
            .with_shortcut(shortcut("switch_session")),
        DropdownItem::new(t.projects_change_root(), "change_root"),
    ]
}

/// Number of items in Sessions submenu
pub const PROJECTS_SUBMENU_ITEM_COUNT: usize = 3;

/// Index of Sessions submenu items
pub const PROJECTS_SUBMENU_NEW: usize = 0;
pub const PROJECTS_SUBMENU_SWITCH: usize = 1;
pub const PROJECTS_SUBMENU_CHANGE_ROOT: usize = 2;

/// The AI submenu: four fixed sections, each opening a nested list. The item
/// keys (`agents`/`sessions`/`skills`/`prompts`) are the contract the app's AI
/// menu handler decodes; the `AI_SUBMENU_*` indices below address the rows.
///
/// `browser` is whether the agents' web browser is shown in a window, `None`
/// when there is no browser to show; with a value, a separator and a row
/// keyed `browser` that switches it follow the sections.
pub fn get_ai_items(browser: Option<bool>) -> Vec<DropdownItem> {
    let t = i18n::t();
    let mut items = vec![
        DropdownItem::new(t.menu_ai_agents(), "agents").with_submenu(),
        DropdownItem::new(t.menu_ai_sessions(), "sessions").with_submenu(),
        DropdownItem::new(t.menu_ai_skills(), "skills").with_submenu(),
        DropdownItem::new(t.menu_ai_prompts(), "prompts").with_submenu(),
    ];
    if let Some(shown) = browser {
        let label = if shown {
            t.menu_ai_hide_browser()
        } else {
            t.menu_ai_show_browser()
        };
        items.push(DropdownItem::separator());
        items.push(DropdownItem::new(label, AI_BROWSER_KEY));
    }
    items
}

/// Key of the AI submenu row that shows or hides the agents' browser.
pub const AI_BROWSER_KEY: &str = "browser";

/// Number of items in the AI submenu.
pub const AI_SUBMENU_ITEM_COUNT: usize = 4;
/// Index of the Agents section in the AI submenu.
pub const AI_SUBMENU_AGENTS: usize = 0;
/// Index of the Sessions section in the AI submenu.
pub const AI_SUBMENU_SESSIONS: usize = 1;
/// Index of the Skills section in the AI submenu.
pub const AI_SUBMENU_SKILLS: usize = 2;
/// Index of the Prompts section in the AI submenu.
pub const AI_SUBMENU_PROMPTS: usize = 3;

/// The agent file-choice submenu (third level): which file of an agent to edit.
/// Row keys `soul`/`toml` are decoded by the app's AI menu handler.
pub fn get_ai_agent_choice_items() -> Vec<DropdownItem> {
    let t = i18n::t();
    vec![
        DropdownItem::new(t.menu_ai_edit_prompt(), "soul"),
        DropdownItem::new(t.menu_ai_edit_settings(), "toml"),
    ]
}

/// Get tools submenu items
pub fn get_tools_items(kb: Option<&termide_config::GlobalKeybindings>) -> Vec<DropdownItem> {
    let t = i18n::t();
    let shortcut = |key: &str| kb.and_then(|kb| menu_shortcut(kb, key));
    // The row order is the contract behind the TOOLS_SUBMENU_* indices
    // below: the action dispatcher, keyboard navigation and mouse hit-testing
    // all address rows by those constants, so a row added or removed here
    // without moving them opens the neighbour of what was clicked.
    vec![
        DropdownItem::new(t.tools_open(), "open"),
        DropdownItem::separator(),
        DropdownItem::new(t.tools_terminal(), "terminal")
            .with_submenu()
            .with_shortcut(shortcut("terminal")),
        DropdownItem::new(t.tools_files(), "files").with_shortcut(shortcut("files")),
        DropdownItem::new(t.tools_editor(), "editor").with_shortcut(shortcut("editor")),
        DropdownItem::new(t.tools_git_status(), "git_status").with_shortcut(shortcut("git_status")),
        DropdownItem::new(t.tools_git_log(), "git_log").with_shortcut(shortcut("git_log")),
        DropdownItem::new(t.tools_journal(), "journal").with_shortcut(shortcut("journal")),
        DropdownItem::new(t.tools_diagnostics(), "diagnostics")
            .with_shortcut(shortcut("diagnostics")),
        DropdownItem::new(t.tools_operations(), "operations"),
        DropdownItem::new(t.tools_outline(), "outline").with_shortcut(shortcut("outline")),
        DropdownItem::new(t.tools_agent(), "agent").with_shortcut(shortcut("agent")),
    ]
}

/// Number of items in Tools submenu (including the separator row).
pub const TOOLS_SUBMENU_ITEM_COUNT: usize = 12;

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
pub const TOOLS_SUBMENU_AGENT: usize = 11;

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

/// The shortcut to show beside a menu entry, if the action has one.
///
/// Menu entries and keybindings are named independently — the Tools entry
/// keyed `git_status` is bound as `open_git_status` — so the two are mapped
/// here rather than assumed to match.
pub fn menu_shortcut(kb: &termide_config::GlobalKeybindings, key: &str) -> Option<String> {
    let binding = match key {
        // Options
        "edit_preferences" => &kb.open_preferences,
        "help" => &kb.open_help,
        "detach_instance" => &kb.detach_instance,
        "quit" => &kb.quit,
        // Sessions
        "new_project" => &kb.new_project,
        "switch_session" => &kb.open_projects,
        // Tools / Windows
        "terminal" => &kb.new_terminal,
        "files" => &kb.new_file_manager,
        "editor" => &kb.new_editor,
        "git_status" => &kb.open_git_status,
        "git_log" => &kb.open_git_log,
        "journal" => &kb.new_journal,
        "diagnostics" => &kb.open_diagnostics,
        "outline" => &kb.open_outline,
        // Bookmarks
        BOOKMARK_ADD_CURRENT => &kb.open_bookmark_add,
        // Panel action menu. Only the actions that have a global binding;
        // moving a panel up or down has none, and inventing one here would
        // advertise a key that does nothing.
        PANEL_ACTION_CLOSE => &kb.close_panel,
        PANEL_ACTION_MOVE_LEFT => &kb.swap_left,
        PANEL_ACTION_MOVE_RIGHT => &kb.swap_right,
        _ => return None,
    };
    // Only the primary key: a menu row is a reminder, and "Alt+H, F1" costs
    // width while saying no more than "Alt+H" does. The full list stays
    // visible in Settings and in the Help panel.
    binding
        .as_ref()
        .map(|b| b.display().to_string())
        .filter(|s| !s.is_empty())
}

/// Get options submenu items.
///
/// `can_detach` says whether this termide is hosted in a detachable session.
/// When it is not, the Detach entry is left out entirely rather than shown and
/// refused: a menu item that normally does nothing teaches users to distrust
/// the menu.
pub fn get_options_items(
    can_detach: bool,
    kb: Option<&termide_config::GlobalKeybindings>,
) -> Vec<DropdownItem> {
    let t = i18n::t();
    let shortcut = |key: &str| kb.and_then(|kb| menu_shortcut(kb, key));
    let mut items = vec![
        DropdownItem::new(t.preferences_themes(), "themes").with_submenu(),
        DropdownItem::new(t.preferences_language(), "language").with_submenu(),
        DropdownItem::new(t.preferences_edit(), "edit_preferences")
            .with_shortcut(shortcut("edit_preferences")),
        DropdownItem::new(t.options_help(), "help").with_shortcut(shortcut("help")),
    ];
    // Detaching sits next to Quit because it is the other way of leaving the
    // session — the one that keeps it running.
    if can_detach {
        items.push(
            DropdownItem::new(t.detach_instance(), "detach_instance")
                .with_shortcut(shortcut("detach_instance")),
        );
    }
    items.push(DropdownItem::new(t.menu_quit(), "quit").with_shortcut(shortcut("quit")));
    items
}

/// Index of the two Options entries that open a nested submenu.
///
/// Only these two are addressed by position, because the nested-submenu state
/// is keyed on it. Everything else is dispatched by `DropdownItem::key`, so
/// that a list whose length varies cannot silently map a click to the wrong
/// action.
pub const OPTIONS_SUBMENU_THEMES: usize = 0;
pub const OPTIONS_SUBMENU_LANGUAGE: usize = 1;

/// Special command ID for "Add command..." menu item
pub const COMMAND_ADD_NEW: &str = "__add_command__";
/// Special ID for "Manage commands" menu item
pub const COMMAND_MANAGE: &str = "__manage_commands__";

/// Get commands submenu items from CommandsRegistry
/// A command's own shortcut, shown in the same column as every other menu
/// entry's. It used to be glued onto the label as ` [Ctrl+K]`, which put the
/// keys in a different place and a different style from the rest of the menus.
fn command_shortcut(command: &termide_config::commands::CommandItem) -> Option<String> {
    command
        .metadata
        .as_ref()
        .and_then(|m| m.key.as_deref())
        .filter(|k| !k.is_empty())
        .map(|k| k.to_string())
}

/// Format command label with type icon prefix (when terminal supports emoji).
/// 💻 = runs in terminal panel, ⚙ = background, 📋 = background with result modal
fn command_label(command: &termide_config::commands::CommandItem) -> String {
    use termide_config::commands::CommandMode;

    let display_name = command
        .metadata
        .as_ref()
        .and_then(|m| m.display_name.as_deref())
        .unwrap_or(&command.name);

    if termide_core::use_emoji_icons() {
        let icon = match command.mode {
            CommandMode::Report => "📋",
            CommandMode::Background => "⚙",
            CommandMode::Terminal => "💻",
        };
        format!("{icon} {display_name}")
    } else {
        display_name.to_string()
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
            .with_shortcut(command_shortcut(command))
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
        items.push(
            DropdownItem::new(
                command_label(command),
                encode_command_menu_key(CommandMenuKeyKind::Command, &command.name, false),
            )
            .with_shortcut(command_shortcut(command)),
        );
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
                    )
                    .with_shortcut(command_shortcut(command));
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
    kb: Option<&termide_config::GlobalKeybindings>,
) -> Vec<DropdownItem> {
    let t = i18n::t();
    let mut items = vec![
        DropdownItem::new(t.bookmarks_add_bookmark(), BOOKMARK_ADD_CURRENT)
            .with_shortcut(kb.and_then(|kb| menu_shortcut(kb, BOOKMARK_ADD_CURRENT))),
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
    kb: Option<&termide_config::GlobalKeybindings>,
) -> Vec<DropdownItem> {
    let t = termide_i18n::t();
    let shortcut = |key: &str| kb.and_then(|kb| menu_shortcut(kb, key));
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
        items.push(
            DropdownItem::new(t.panel_action_move_left(), PANEL_ACTION_MOVE_LEFT)
                .with_shortcut(shortcut(PANEL_ACTION_MOVE_LEFT)),
        );
        items.push(
            DropdownItem::new(t.panel_action_move_right(), PANEL_ACTION_MOVE_RIGHT)
                .with_shortcut(shortcut(PANEL_ACTION_MOVE_RIGHT)),
        );
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

    items.push(
        DropdownItem::new(t.panel_action_close(), PANEL_ACTION_CLOSE)
            .with_shortcut(shortcut(PANEL_ACTION_CLOSE)),
    );

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
    let width = dropdown_width(items);
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

#[cfg(test)]
mod options_menu_tests {
    use super::*;

    fn keys(can_detach: bool) -> Vec<String> {
        get_options_items(can_detach, None)
            .into_iter()
            .map(|i| i.key)
            .collect()
    }

    #[test]
    fn detach_is_offered_only_when_the_session_can_detach() {
        assert_eq!(
            keys(true),
            vec![
                "themes",
                "language",
                "edit_preferences",
                "help",
                "detach_instance",
                "quit"
            ]
        );
        assert_eq!(
            keys(false),
            vec!["themes", "language", "edit_preferences", "help", "quit"]
        );
    }

    /// The nested submenus (Themes, Language) are the only entries addressed
    /// by position, so those two indices must stay put whichever shape the
    /// list takes.
    #[test]
    fn nested_submenu_indices_hold_for_both_shapes() {
        for can_detach in [true, false] {
            let items = get_options_items(can_detach, None);
            assert_eq!(items[OPTIONS_SUBMENU_THEMES].key, "themes");
            assert_eq!(items[OPTIONS_SUBMENU_LANGUAGE].key, "language");
            assert!(items[OPTIONS_SUBMENU_THEMES].has_submenu);
            assert!(items[OPTIONS_SUBMENU_LANGUAGE].has_submenu);
        }
    }

    /// Quit is last in both shapes: detaching is an alternative to quitting,
    /// not a replacement, and muscle memory goes to the bottom entry.
    #[test]
    fn quit_stays_last() {
        for can_detach in [true, false] {
            assert_eq!(keys(can_detach).last().unwrap(), "quit");
        }
    }
}

#[cfg(test)]
mod menu_shortcut_tests {
    use super::*;
    use termide_config::GlobalKeybindings;

    fn defaults() -> GlobalKeybindings {
        let mut kb = GlobalKeybindings::default();
        kb.with_defaults();
        kb
    }

    #[test]
    fn entries_show_the_binding_of_the_action_they_run() {
        let kb = defaults();
        assert_eq!(menu_shortcut(&kb, "quit").as_deref(), Some("Alt+Q"));
        assert_eq!(
            menu_shortcut(&kb, "detach_instance").as_deref(),
            Some("Alt+D")
        );
        // Menu key and binding name differ here, which is the reason for the
        // explicit mapping.
        assert_eq!(
            menu_shortcut(&kb, "git_status").as_deref(),
            Some("Alt+G"),
            "the Tools entry `git_status` is bound as `open_git_status`"
        );
        assert_eq!(
            menu_shortcut(&kb, "help").as_deref(),
            Some("Alt+H"),
            "only the primary key, not the whole `Alt+H, F1` list"
        );
    }

    #[test]
    fn the_browser_row_follows_the_ai_sections_only_when_there_is_a_browser() {
        let plain = get_ai_items(None);
        assert_eq!(plain.len(), AI_SUBMENU_ITEM_COUNT);
        let with_browser = get_ai_items(Some(false));
        assert_eq!(with_browser.len(), AI_SUBMENU_ITEM_COUNT + 2);
        assert!(with_browser[AI_SUBMENU_ITEM_COUNT].is_separator);
        let row = &with_browser[AI_SUBMENU_ITEM_COUNT + 1];
        assert_eq!(row.key, AI_BROWSER_KEY);
        assert_ne!(
            row.label,
            get_ai_items(Some(true))[AI_SUBMENU_ITEM_COUNT + 1].label
        );
        // The sections keep their indices.
        assert_eq!(with_browser[AI_SUBMENU_PROMPTS].key, "prompts");
    }

    /// The TOOLS_SUBMENU_* indices address rows of `get_tools_items`; the two
    /// drifted once (the separator row was dropped from the list but not from
    /// the indices) and every entry from Terminal on opened its neighbour.
    #[test]
    fn tools_items_sit_at_their_index_constants() {
        let items = get_tools_items(Some(&defaults()));
        assert_eq!(items.len(), TOOLS_SUBMENU_ITEM_COUNT);
        assert!(items[TOOLS_SUBMENU_SEPARATOR].is_separator);
        for (index, key) in [
            (TOOLS_SUBMENU_OPEN, "open"),
            (TOOLS_SUBMENU_TERMINAL, "terminal"),
            (TOOLS_SUBMENU_FILES, "files"),
            (TOOLS_SUBMENU_EDITOR, "editor"),
            (TOOLS_SUBMENU_GIT_STATUS, "git_status"),
            (TOOLS_SUBMENU_GIT_LOG, "git_log"),
            (TOOLS_SUBMENU_JOURNAL, "journal"),
            (TOOLS_SUBMENU_DIAGNOSTICS, "diagnostics"),
            (TOOLS_SUBMENU_OPERATIONS, "operations"),
            (TOOLS_SUBMENU_OUTLINE, "outline"),
        ] {
            assert_eq!(items[index].key, key, "row {index} should be `{key}`");
        }
        assert!(
            items[TOOLS_SUBMENU_TERMINAL].has_submenu,
            "Terminal opens the shell picker"
        );
    }

    #[test]
    fn entries_without_an_action_have_no_shortcut() {
        let kb = defaults();
        assert_eq!(menu_shortcut(&kb, "themes"), None);
        assert_eq!(menu_shortcut(&kb, "language"), None);
        assert_eq!(menu_shortcut(&kb, "nonexistent"), None);
    }

    /// Every menu that has bindable entries must annotate them — the point is
    /// that the menus agree with each other, not that one of them is special.
    #[test]
    fn every_menu_annotates_the_entries_that_have_bindings() {
        let kb = defaults();

        let sessions = get_projects_items(Some(&kb));
        assert_eq!(
            sessions
                .iter()
                .find(|i| i.key == "new_project")
                .unwrap()
                .shortcut
                .as_deref(),
            Some("Alt+N")
        );

        let tools = get_tools_items(Some(&kb));
        for key in ["terminal", "files", "editor", "git_status", "outline"] {
            assert!(
                tools
                    .iter()
                    .find(|i| i.key == key)
                    .unwrap()
                    .shortcut
                    .is_some(),
                "Tools entry {key} should show its shortcut"
            );
        }

        let panel = get_panel_action_menu_items(2, 2, Some(&kb));
        assert_eq!(
            panel
                .iter()
                .find(|i| i.key == PANEL_ACTION_CLOSE)
                .unwrap()
                .shortcut
                .as_deref(),
            Some("Alt+W")
        );

        let bookmarks =
            get_bookmarks_items(&termide_config::BookmarksConfig::default(), None, Some(&kb));
        assert_eq!(
            bookmarks
                .iter()
                .find(|i| i.key == BOOKMARK_ADD_CURRENT)
                .unwrap()
                .shortcut
                .as_deref(),
            Some("Alt+B")
        );
    }

    /// Options entries carry their shortcuts through to the dropdown, and the
    /// submenu entries stay blank.
    #[test]
    fn options_items_are_annotated() {
        let kb = defaults();
        let items = get_options_items(true, Some(&kb));

        let by_key = |key: &str| {
            items
                .iter()
                .find(|i| i.key == key)
                .unwrap_or_else(|| panic!("{key} missing"))
        };
        assert_eq!(by_key("quit").shortcut.as_deref(), Some("Alt+Q"));
        assert_eq!(by_key("detach_instance").shortcut.as_deref(), Some("Alt+D"));
        assert_eq!(by_key("themes").shortcut, None);

        // Without keybindings nothing is annotated, and nothing panics.
        assert!(get_options_items(true, None)
            .iter()
            .all(|i| i.shortcut.is_none()));
    }

    /// A command's own hotkey used to be glued onto its label as ` [Ctrl+K]`,
    /// so the Commands menu showed keys in a different place and a different
    /// colour from every other menu.
    #[test]
    fn command_hotkeys_move_out_of_the_label() {
        use termide_config::commands::{CommandItem, CommandMetadata, CommandMode};

        let command = CommandItem {
            name: "deploy".to_string(),
            command: Some("make deploy".to_string()),
            mode: CommandMode::Terminal,
            is_project: false,
            metadata: Some(CommandMetadata {
                key: Some("Ctrl+Shift+D".to_string()),
                ..Default::default()
            }),
        };

        let label = command_label(&command);
        assert!(
            !label.contains('['),
            "the hotkey must not be part of the label: {label}"
        );
        assert!(label.contains("deploy"));
        assert_eq!(command_shortcut(&command).as_deref(), Some("Ctrl+Shift+D"));
    }

    #[test]
    fn a_command_without_a_hotkey_has_no_shortcut() {
        use termide_config::commands::{CommandItem, CommandMode};

        let command = CommandItem {
            name: "build".to_string(),
            command: Some("make".to_string()),
            mode: CommandMode::Background,
            is_project: false,
            metadata: None,
        };
        assert_eq!(command_shortcut(&command), None);
    }

    /// The row must be wide enough for the longest label and the longest
    /// shortcut side by side, or one would be drawn over the other.
    #[test]
    fn width_accounts_for_the_shortcut_column() {
        let theme = termide_theme::Theme::default();
        let plain_items = [DropdownItem::new("Quit", "quit")];
        let annotated_items =
            [DropdownItem::new("Quit", "quit").with_shortcut(Some("Alt+Q".to_string()))];

        let plain = Dropdown::new(&plain_items, 0, 0, 0, &theme);
        let annotated = Dropdown::new(&annotated_items, 0, 0, 0, &theme);

        assert!(
            annotated.width() > plain.width(),
            "annotated {} should exceed plain {}",
            annotated.width(),
            plain.width()
        );
    }
}
