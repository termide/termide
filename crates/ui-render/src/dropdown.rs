//! Dropdown menu widget.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
};
use unicode_width::UnicodeWidthStr;

use termide_core::ThemeColors;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui::ScrollBar;

/// Dropdown menu item
#[derive(Debug, Clone)]
pub struct DropdownItem {
    pub label: String,
    pub key: String,
    /// Whether this item opens a submenu
    pub has_submenu: bool,
}

impl DropdownItem {
    pub fn new(label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: key.into(),
            has_submenu: false,
        }
    }

    /// Mark this item as having a submenu
    pub fn with_submenu(mut self) -> Self {
        self.has_submenu = true;
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
            .map(|item| item.label.width())
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

        let width = self.width();
        let height = self.height();

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

        // Get visible items
        let visible_end = (self.scroll_offset + self.max_visible).min(self.items.len());
        let visible_items = &self.items[self.scroll_offset..visible_end];

        // Create list of items
        let items: Vec<ListItem> = visible_items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let actual_index = self.scroll_offset + i;
                let is_selected = actual_index == self.selected;

                let base_style = if is_selected {
                    Style::default()
                        .bg(self.theme.selected_bg)
                        .fg(self.theme.selected_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.fg)
                };

                let mut spans = vec![Span::styled(" ", base_style)];
                spans.push(Span::styled(&item.label, base_style));

                // Add submenu indicator or padding
                let label_width = item.label.width();
                // -6: borders(2) + leading space(1) + suffix " ▶ "(3)
                let padding_len = (width as usize).saturating_sub(label_width + 6);
                if padding_len > 0 {
                    // Use a static buffer to avoid per-item allocation
                    const SPACES: &str = "                                                                                                                                ";
                    let pad = &SPACES[..padding_len.min(SPACES.len())];
                    spans.push(Span::styled(pad, base_style));
                }

                if item.has_submenu {
                    spans.push(Span::styled(" ▶ ", base_style));
                } else {
                    spans.push(Span::styled("   ", base_style));
                }

                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.theme.accented_fg))
                .style(Style::default().bg(self.theme.bg)),
        );

        list.render(area, buf);

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

/// Get tools submenu items
pub fn get_tools_items() -> Vec<DropdownItem> {
    let t = i18n::t();
    vec![
        DropdownItem::new(t.tools_files(), "files"),
        DropdownItem::new(t.tools_terminal(), "terminal").with_submenu(),
        DropdownItem::new(t.tools_editor(), "editor"),
        DropdownItem::new(t.tools_git_status(), "git_status"),
        DropdownItem::new(t.tools_git_log(), "git_log"),
        DropdownItem::new(t.tools_journal(), "journal"),
        DropdownItem::new(t.tools_diagnostics(), "diagnostics"),
        DropdownItem::new(t.tools_operations(), "operations"),
        DropdownItem::new(t.tools_outline(), "outline"),
    ]
}

/// Number of items in Tools submenu
pub const TOOLS_SUBMENU_ITEM_COUNT: usize = 9;

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
        DropdownItem::new(t.options_manage_scripts(), "manage_scripts"),
        DropdownItem::new(t.bookmarks_manage(), "manage_bookmarks"),
        DropdownItem::new(t.preferences_edit(), "edit_preferences"),
        DropdownItem::new(t.options_help(), "help"),
        DropdownItem::new(t.menu_quit(), "quit"),
    ]
}

/// Number of items in Options submenu
pub const OPTIONS_SUBMENU_ITEM_COUNT: usize = 7;

/// Special script ID for "Add script..." menu item
pub const SCRIPT_ADD_NEW: &str = "__add_script__";

/// Get scripts submenu items from ScriptsRegistry
pub fn get_scripts_items(registry: &termide_config::scripts::ScriptsRegistry) -> Vec<DropdownItem> {
    let mut items = Vec::new();

    // Add root items (scripts in scripts/ root)
    for script in &registry.root_items {
        items.push(DropdownItem::new(&script.name, &script.name));
    }

    // Add groups (subdirectories) with submenu indicator
    for group in &registry.groups {
        items.push(DropdownItem::new(&group.name, &group.name).with_submenu());
    }

    // If no scripts exist, show "Add script..." item
    if items.is_empty() {
        let t = i18n::t();
        items.push(DropdownItem::new(t.menu_scripts_add(), SCRIPT_ADD_NEW));
    }

    items
}

/// Get scripts nested submenu items for a specific group
pub fn get_scripts_group_items(
    registry: &termide_config::scripts::ScriptsRegistry,
    group_name: &str,
) -> Vec<DropdownItem> {
    registry
        .groups
        .iter()
        .find(|g| g.name == group_name)
        .map(|group| {
            group
                .items
                .iter()
                .map(|script| DropdownItem::new(&script.name, &script.name))
                .collect()
        })
        .unwrap_or_default()
}

/// Special bookmark action IDs
pub const BOOKMARK_ADD_CURRENT: &str = "__bookmark_add__";
pub const BOOKMARK_MANAGE: &str = "__bookmark_manage__";

/// Get bookmarks submenu items from BookmarksConfig
pub fn get_bookmarks_items(config: &termide_config::BookmarksConfig) -> Vec<DropdownItem> {
    let t = i18n::t();
    let mut items = vec![DropdownItem::new(
        t.bookmarks_add_bookmark(),
        BOOKMARK_ADD_CURRENT,
    )];

    if config.is_empty() {
        items.push(DropdownItem::new(t.bookmarks_no_bookmarks(), ""));
        return items;
    }

    // Add named groups first (as submenus)
    let named_groups = config.named_groups();
    for group_name in named_groups.keys() {
        items.push(DropdownItem::new(group_name, group_name).with_submenu());
    }

    // Add ungrouped bookmarks directly in menu (not in submenu)
    for bookmark in config.ungrouped() {
        items.push(DropdownItem::new(bookmark.display_name(), &bookmark.path));
    }

    items
}

/// Get bookmarks count for determining submenu item count
pub fn get_bookmarks_item_count(config: &termide_config::BookmarksConfig) -> usize {
    if config.is_empty() {
        2 // add_current + no_bookmarks
    } else {
        1 + config.named_groups().len() + config.ungrouped().len()
    }
}

/// Get bookmark items for a specific group
pub fn get_bookmarks_group_items(
    config: &termide_config::BookmarksConfig,
    group_name: &str,
) -> Vec<DropdownItem> {
    config
        .grouped()
        .get(group_name)
        .map(|bookmarks| {
            bookmarks
                .iter()
                .map(|b| DropdownItem::new(b.display_name(), &b.path))
                .collect()
        })
        .unwrap_or_default()
}
