//! Vim-aware navigation helpers for list panels.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// =============================================================================
// Vim-aware navigation helpers for list panels
// =============================================================================

/// Resolve the effective character for vim navigation: maps Cyrillic
/// glyphs that share a physical key with Latin letters back to Latin.
fn vim_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) => Some(termide_keyboard::cyrillic_to_latin(c)),
        _ => None,
    }
}

/// Check if key event is a "move up" action.
/// Returns true for Up arrow (without modifiers), or 'k'/'л' when vim_mode is enabled.
pub fn is_move_up(key: &KeyEvent, vim_mode: bool) -> bool {
    if key.code == KeyCode::Up && key.modifiers.is_empty() {
        return true;
    }
    vim_mode && key.modifiers.is_empty() && vim_char(key) == Some('k')
}

/// Check if key event is a "move down" action.
/// Returns true for Down arrow, or 'j' (any layout) when vim_mode is enabled.
pub fn is_move_down(key: &KeyEvent, vim_mode: bool) -> bool {
    if key.code == KeyCode::Down && key.modifiers.is_empty() {
        return true;
    }
    vim_mode && key.modifiers.is_empty() && vim_char(key) == Some('j')
}

/// Check if key event is a "go to start/home" action.
/// Returns true for Home key, or 'g' (any layout) when vim_mode is enabled.
pub fn is_go_home(key: &KeyEvent, vim_mode: bool) -> bool {
    if key.code == KeyCode::Home && key.modifiers.is_empty() {
        return true;
    }
    vim_mode && key.modifiers.is_empty() && vim_char(key) == Some('g')
}

/// Check if key event is a "go to end" action.
/// Returns true for End key, or 'G' (any layout, Shift) when vim_mode is enabled.
pub fn is_go_end(key: &KeyEvent, vim_mode: bool) -> bool {
    if key.code == KeyCode::End && key.modifiers.is_empty() {
        return true;
    }
    vim_mode && key.modifiers == KeyModifiers::SHIFT && vim_char(key) == Some('G')
}
