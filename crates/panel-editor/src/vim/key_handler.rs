//! Vim key event handling.

use crossterm::event::{KeyCode, KeyEvent};
use termide_keyboard::cyrillic_to_latin_opt;

use super::mode::VimMode;
use super::motions::VimMotion;
use super::normal_mode::handle_normal_mode;
use super::operators::VimOperator;
use super::state::VimState;
use super::visual_mode::{handle_visual_line_mode, handle_visual_mode};
use super::PanelDirection;

/// Translate Cyrillic characters to Latin for vim commands.
pub(super) fn translate_cyrillic_key(key: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(c) = key.code {
        if let Some(latin) = cyrillic_to_latin_opt(c) {
            return KeyEvent::new(KeyCode::Char(latin), key.modifiers);
        }
    }
    key
}

/// Result of handling a Vim key event.
#[derive(Debug, Clone)]
pub enum VimKeyResult {
    /// No action needed, key was consumed by Vim state.
    Consumed,
    /// Execute a motion (cursor movement).
    Motion { motion: VimMotion, count: usize },
    /// Execute a motion with selection (visual mode).
    MotionWithSelection { motion: VimMotion, count: usize },
    /// Execute an operator with a motion.
    OperatorMotion {
        operator: VimOperator,
        motion: VimMotion,
        count: usize,
    },
    /// Execute a linewise operator (dd, yy, cc).
    LinewiseOperator { operator: VimOperator, count: usize },
    /// Execute operator on visual selection.
    VisualOperator { operator: VimOperator },
    /// Enter insert mode at position.
    EnterInsert(InsertPosition),
    /// Exit to normal mode.
    ExitToNormal,
    /// Start visual mode.
    StartVisual,
    /// Start visual line mode.
    StartVisualLine,
    /// Delete character under cursor (x).
    DeleteChar { count: usize },
    /// Paste from register.
    Paste { after: bool, count: usize },
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Panel navigation (Ctrl+w h/j/k/l).
    PanelNavigation(PanelDirection),
    /// Pass through to standard editor (for insert mode).
    PassThrough,
    /// Key not recognized.
    Unhandled,
}

/// Position for entering insert mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    /// Insert before cursor (i).
    BeforeCursor,
    /// Insert after cursor (a).
    AfterCursor,
    /// Insert at line start (I).
    LineStart,
    /// Insert at line end (A).
    LineEnd,
    /// Open new line below (o).
    NewLineBelow,
    /// Open new line above (O).
    NewLineAbove,
}

/// Handle a key event in Vim mode.
///
/// # Arguments
/// * `state` - Current Vim state
/// * `key` - The key event to handle
///
/// # Returns
/// Result indicating what action to take
pub fn handle_vim_key(state: &mut VimState, key: KeyEvent) -> VimKeyResult {
    match state.mode {
        VimMode::Normal => handle_normal_mode(state, key),
        VimMode::Insert => handle_insert_mode(state, key),
        VimMode::Visual => handle_visual_mode(state, key),
        VimMode::VisualLine => handle_visual_line_mode(state, key),
    }
}

/// Handle key in insert mode.
fn handle_insert_mode(state: &mut VimState, key: KeyEvent) -> VimKeyResult {
    match key.code {
        KeyCode::Esc => {
            state.exit_to_normal();
            VimKeyResult::ExitToNormal
        }
        // All other keys pass through to standard editor
        _ => VimKeyResult::PassThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_event_ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    #[test]
    fn test_basic_motions() {
        let mut state = VimState::new();

        // h motion
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('h')));
        assert!(matches!(
            result,
            VimKeyResult::Motion {
                motion: VimMotion::Left,
                count: 1
            }
        ));

        // j motion
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('j')));
        assert!(matches!(
            result,
            VimKeyResult::Motion {
                motion: VimMotion::Down,
                count: 1
            }
        ));
    }

    #[test]
    fn test_count_prefix() {
        let mut state = VimState::new();

        // 5j should move down 5 times
        handle_vim_key(&mut state, key_event(KeyCode::Char('5')));
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('j')));
        assert!(matches!(
            result,
            VimKeyResult::Motion {
                motion: VimMotion::Down,
                count: 5
            }
        ));
    }

    #[test]
    fn test_insert_mode_entry() {
        let mut state = VimState::new();

        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('i')));
        assert!(matches!(
            result,
            VimKeyResult::EnterInsert(InsertPosition::BeforeCursor)
        ));

        state = VimState::new();
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('a')));
        assert!(matches!(
            result,
            VimKeyResult::EnterInsert(InsertPosition::AfterCursor)
        ));
    }

    #[test]
    fn test_insert_mode_escape() {
        let mut state = VimState::new();
        state.enter_insert();

        let result = handle_vim_key(&mut state, key_event(KeyCode::Esc));
        assert!(matches!(result, VimKeyResult::ExitToNormal));
        assert_eq!(state.mode, VimMode::Normal);
    }

    #[test]
    fn test_dd_delete_line() {
        let mut state = VimState::new();

        // First 'd' sets pending operator
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('d')));
        assert!(matches!(result, VimKeyResult::Consumed));
        assert_eq!(state.pending_operator, Some(VimOperator::Delete));

        // Second 'd' triggers linewise delete
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('d')));
        assert!(matches!(
            result,
            VimKeyResult::LinewiseOperator {
                operator: VimOperator::Delete,
                count: 1
            }
        ));
    }

    #[test]
    fn test_ctrl_u_d() {
        let mut state = VimState::new();

        let result = handle_vim_key(&mut state, key_event_ctrl('u'));
        assert!(matches!(
            result,
            VimKeyResult::Motion {
                motion: VimMotion::HalfPageUp,
                count: 1
            }
        ));

        let result = handle_vim_key(&mut state, key_event_ctrl('d'));
        assert!(matches!(
            result,
            VimKeyResult::Motion {
                motion: VimMotion::HalfPageDown,
                count: 1
            }
        ));
    }

    #[test]
    fn test_visual_mode() {
        let mut state = VimState::new();

        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('v')));
        assert!(matches!(result, VimKeyResult::StartVisual));
    }

    #[test]
    fn test_gg_document_start() {
        let mut state = VimState::new();

        // First 'g'
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('g')));
        assert!(matches!(result, VimKeyResult::Consumed));

        // Second 'g'
        let result = handle_vim_key(&mut state, key_event(KeyCode::Char('g')));
        assert!(matches!(
            result,
            VimKeyResult::Motion {
                motion: VimMotion::DocumentStart,
                count: 1
            }
        ));
    }
}
