use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Clear},
};

/// Dropdown menu item
#[derive(Debug, Clone)]
pub struct DropdownItem {
    pub label: String,
    pub key: String,
}

impl DropdownItem {
    pub fn new(label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: key.into(),
        }
    }
}

/// Dropdown menu
pub struct Dropdown<'a> {
    items: &'a [DropdownItem],
    selected: usize,
    x: u16,
    y: u16,
}

impl<'a> Dropdown<'a> {
    pub fn new(items: &'a [DropdownItem], selected: usize, x: u16, y: u16) -> Self {
        Self {
            items,
            selected,
            x,
            y,
        }
    }

    pub fn render(&self, buf: &mut Buffer) {
        if self.items.is_empty() {
            return;
        }

        // Calculate dropdown dimensions
        let max_label_len = self
            .items
            .iter()
            .map(|item| item.label.len())
            .max()
            .unwrap_or(0);
        let width = (max_label_len + 6).min(40) as u16; // 6 = padding + number
        let height = (self.items.len() + 2) as u16; // +2 for borders

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

        // Create list of items
        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let style = if i == self.selected {
                    Style::default()
                        .bg(Color::Cyan)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let line = Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{}. ", i + 1), Style::default().fg(Color::Yellow)),
                    Span::styled(&item.label, style),
                ]);

                ListItem::new(line)
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .style(Style::default().bg(Color::DarkGray)),
        );

        list.render(area, buf);
    }
}

/// Menu item definitions
pub fn get_tools_items() -> Vec<DropdownItem> {
    vec![
        DropdownItem::new("Files", "files"),
        DropdownItem::new("Editor", "editor"),
        DropdownItem::new("Terminal", "terminal"),
    ]
}

pub fn get_help_items() -> Vec<DropdownItem> {
    vec![
        DropdownItem::new("Welcome", "welcome"),
        DropdownItem::new("Debug console", "debug"),
    ]
}
