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
mod path_utils;
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
    event::{
        DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

use app::App;
use panels::file_manager::FileManager;

fn main() -> Result<()> {
    // Load config first to get language setting
    let config = config::Config::load().unwrap_or_default();

    // Initialize translation system with language from config
    i18n::init_with_language(&config.general.language);

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

    // Check if terminal supports enhanced keyboard protocol (kitty protocol)
    // This enables proper Alt+Cyrillic handling in modern terminals like Ghostty, Kitty, WezTerm
    let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);

    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    )?;

    if keyboard_enhanced {
        // Note: REPORT_ALL_KEYS_AS_ESCAPE_CODES causes modifier keys (Shift, Ctrl, Alt)
        // to generate separate events, which breaks combinations like Shift+Home.
        // We only use DISAMBIGUATE_ESCAPE_CODES and REPORT_ALTERNATE_KEYS.
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Get terminal size and use it to initialize app with correct dimensions
    let size = terminal.size()?;

    // Create application with terminal size to ensure proper panel layout
    let mut app = App::new_with_size(size.width, size.height);

    // Try to load session, fallback to default layout on error
    if let Err(_e) = app.load_session() {
        // Session file doesn't exist or is corrupted - use default layout
        // Add two FileManager panels in a 50/50 split
        app.add_panel(Box::new(FileManager::new()));
        app.add_panel(Box::new(FileManager::new()));
    }

    // Run application
    let result = app.run(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
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
