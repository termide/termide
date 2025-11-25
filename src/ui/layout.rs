use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    widgets::Block,
    Frame,
};
use std::any::Any;

use crate::{
    constants::DEFAULT_FM_WIDTH,
    panels::{editor::Editor, file_manager::FileManager, terminal_pty::Terminal, PanelContainer},
    state::{ActiveModal, AppState, LayoutMode},
};

use super::{menu::render_menu, modal::Modal, status_bar::StatusBar};

/// Render the main application layout
pub fn render_layout(frame: &mut Frame, state: &mut AppState, panels: &mut PanelContainer) {
    let size = frame.area();

    // Set application background
    let background = Block::default().style(Style::default().bg(state.theme.bg));
    frame.render_widget(background, size);

    // Split screen into menu (1 line), main area, and status bar (1 line)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Menu
            Constraint::Min(0),    // Main area
            Constraint::Length(1), // Status bar
        ])
        .split(size);

    // Render menu
    render_menu(frame, main_chunks[0], state);

    // Render main area depending on mode
    match state.layout_mode {
        LayoutMode::Single => {
            // In Single mode: render active panel (there's always at least Welcome)
            let active_index = state.active_panel;
            if let Some(panel) = panels.get_mut(active_index) {
                panel.render(
                    main_chunks[1],
                    frame.buffer_mut(),
                    true,
                    active_index,
                    state,
                );

                // Get information depending on panel type
                let (selected_count, file_info, disk_space, editor_info, terminal_info) =
                    if let Some(fm) = (&mut **panel as &mut dyn Any).downcast_mut::<FileManager>() {
                        (
                            Some(fm.get_selected_count()),
                            fm.get_current_file_info(),
                            fm.get_disk_space_info(),
                            None,
                            None,
                        )
                    } else if let Some(editor) =
                        (&mut **panel as &mut dyn Any).downcast_mut::<Editor>()
                    {
                        (None, None, None, Some(editor.get_editor_info()), None)
                    } else if let Some(terminal) =
                        (&mut **panel as &mut dyn Any).downcast_mut::<Terminal>()
                    {
                        (None, None, None, None, Some(terminal.get_terminal_info()))
                    } else {
                        (None, None, None, None, None)
                    };

                StatusBar::render(
                    frame.buffer_mut(),
                    main_chunks[2],
                    state,
                    &panel.title(),
                    selected_count,
                    file_info.as_ref(),
                    disk_space.as_ref(),
                    editor_info.as_ref(),
                    terminal_info.as_ref(),
                );
            }
        }
        LayoutMode::MultiPanel => {
            // Multi-panel mode - FM on the left + main panels on the right
            let panel_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(state.layout_info.fm_width.unwrap_or(DEFAULT_FM_WIDTH)), // FM
                    Constraint::Min(0), // Main panels
                ])
                .split(main_chunks[1]);

            // Render FM (panel 0) - always visible
            if let Some(fm_panel) = panels.get_mut(0) {
                let is_focused = state.active_panel == 0;
                fm_panel.render(panel_chunks[0], frame.buffer_mut(), is_focused, 0, state);
            }

            // Get list of visible main panels (indices > 0)
            let visible_main: Vec<usize> = panels
                .visible_indices()
                .into_iter()
                .filter(|&i| i > 0)
                .collect();

            // Render visible main panels or Welcome
            if !visible_main.is_empty() {
                // There are visible panels - create constraints for them with weights
                let weights: Vec<u32> = visible_main
                    .iter()
                    .map(|&i| state.get_panel_weight(i) as u32)
                    .collect();
                let total_weight: u32 = weights.iter().sum();

                let main_constraints: Vec<Constraint> = weights
                    .iter()
                    .map(|&w| Constraint::Ratio(w, total_weight))
                    .collect();

                let main_panel_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(main_constraints)
                    .split(panel_chunks[1]);

                // Render each visible main panel
                for (chunk_idx, &panel_index) in visible_main.iter().enumerate() {
                    if let Some(panel) = panels.get_mut(panel_index) {
                        let is_focused = state.active_panel == panel_index;
                        panel.render(
                            main_panel_chunks[chunk_idx],
                            frame.buffer_mut(),
                            is_focused,
                            panel_index,
                            state,
                        );
                    }
                }
            }
            // Note: Welcome is now added as a real panel when needed,
            // so fallback is not needed

            // Render status bar for active panel
            // (can be FM or one of the main panels)
            if let Some(active_panel) = panels.get_mut(state.active_panel) {
                // Get information depending on panel type
                let (selected_count, file_info, disk_space, editor_info, terminal_info) =
                    if let Some(fm) =
                        (&mut **active_panel as &mut dyn Any).downcast_mut::<FileManager>()
                    {
                        (
                            Some(fm.get_selected_count()),
                            fm.get_current_file_info(),
                            fm.get_disk_space_info(),
                            None,
                            None,
                        )
                    } else if let Some(editor) =
                        (&mut **active_panel as &mut dyn Any).downcast_mut::<Editor>()
                    {
                        (None, None, None, Some(editor.get_editor_info()), None)
                    } else if let Some(terminal) =
                        (&mut **active_panel as &mut dyn Any).downcast_mut::<Terminal>()
                    {
                        (None, None, None, None, Some(terminal.get_terminal_info()))
                    } else {
                        (None, None, None, None, None)
                    };

                StatusBar::render(
                    frame.buffer_mut(),
                    main_chunks[2],
                    state,
                    &active_panel.title(),
                    selected_count,
                    file_info.as_ref(),
                    disk_space.as_ref(),
                    editor_info.as_ref(),
                    terminal_info.as_ref(),
                );
            }
        }
    }

    // Render dropdowns and modals
    render_dropdowns_and_modals(frame, state);
}

/// Render modal windows
fn render_dropdowns_and_modals(frame: &mut Frame, state: &mut AppState) {
    // Render active modal window if it's open
    // Copy theme before getting mutable modal reference to avoid borrow checker issues
    let theme = state.theme;

    if let Some(modal) = state.get_active_modal_mut() {
        let area = frame.area();
        match modal {
            ActiveModal::Confirm(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Input(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Select(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Overwrite(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Conflict(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Info(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::RenamePattern(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::EditableSelect(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Search(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Replace(m) => m.render(area, frame.buffer_mut(), theme),
        }
    }
}
