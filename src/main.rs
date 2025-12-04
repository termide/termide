mod app;
mod clipboard;
mod config;
mod constants;
mod editor;
mod event;
mod fs_watcher;
mod git;
mod i18n;
mod keyboard;
mod layout_manager;
mod logger;
mod panels;
mod rename_pattern;
mod session;
mod state;
mod syntax_highlighter;
mod system_monitor;
mod theme;
mod ui;
mod xdg_dirs;

use anyhow::Result;
use crossterm::{
    event::{DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

use app::App;
use panels::file_manager::FileManager;

fn main() -> Result<()> {
    // Load config first to get language setting
    let config = config::Config::load().unwrap_or_default();

    // Initialize translation system with language from config
    i18n::init_with_language(&config.language);

    // Check for git on the system
    let git_available = git::check_git_available();
    let t = i18n::t();
    if git_available {
        eprintln!("{}", t.git_detected());
    } else {
        eprintln!("{}", t.git_not_found());
    }

    // Initialize terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Get terminal size (might be useful later)
    let _size = terminal.size()?;

    // Create application
    let mut app = App::new();

    // Try to load session, fallback to default layout on error
    if let Err(_e) = app.load_session() {
        // Session file doesn't exist or is corrupted - use default layout
        // Set file manager (always in separate column)
        app.set_file_manager(Box::new(FileManager::new()));

        // Add welcome panel as first panel group
        app.add_panel(Box::new(panels::welcome::Welcome::new()));
    }

    // Run application
    let result = app.run(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableFocusChange
    )?;
    terminal.show_cursor()?;

    // Print error if there was one
    if let Err(err) = result {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}
