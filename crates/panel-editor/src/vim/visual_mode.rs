//! Vim visual and visual-line mode key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::key_handler::{translate_cyrillic_key, VimKeyResult};
use super::motions::VimMotion;
use super::operators::VimOperator;
use super::state::VimState;

/// Handle key in visual mode.
pub(super) fn handle_visual_mode(state: &mut VimState, key: KeyEvent) -> VimKeyResult {
    // Translate Cyrillic to Latin for vim commands
    let key = translate_cyrillic_key(key);

    // Check for Ctrl modifiers
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('u') => {
                let count = state.effective_count();
                state.clear_pending();
                return VimKeyResult::MotionWithSelection {
                    motion: VimMotion::HalfPageUp,
                    count,
                };
            }
            KeyCode::Char('d') => {
                let count = state.effective_count();
                state.clear_pending();
                return VimKeyResult::MotionWithSelection {
                    motion: VimMotion::HalfPageDown,
                    count,
                };
            }
            _ => {}
        }
    }

    match key.code {
        // Count prefix
        KeyCode::Char(ch @ '1'..='9') => {
            state.accumulate_count(ch);
            VimKeyResult::Consumed
        }
        KeyCode::Char(ch @ '0') => {
            if state.accumulate_count(ch) {
                VimKeyResult::Consumed
            } else {
                state.clear_pending();
                VimKeyResult::MotionWithSelection {
                    motion: VimMotion::LineStart,
                    count: 1,
                }
            }
        }

        // Motions extend selection
        KeyCode::Char('h') | KeyCode::Left => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::Left,
                count,
            }
        }
        // j - logical line down
        KeyCode::Char('j') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::Down,
                count,
            }
        }
        // Down arrow - visual line down
        KeyCode::Down => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::VisualDown,
                count,
            }
        }
        // k - logical line up
        KeyCode::Char('k') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::Up,
                count,
            }
        }
        // Up arrow - visual line up
        KeyCode::Up => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::VisualUp,
                count,
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::Right,
                count,
            }
        }
        KeyCode::Char('w') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::WordForward,
                count,
            }
        }
        KeyCode::Char('b') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::WordBackward,
                count,
            }
        }
        KeyCode::Char('e') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::WordEnd,
                count,
            }
        }
        KeyCode::Char('^') => {
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::FirstNonBlank,
                count: 1,
            }
        }
        KeyCode::Char('$') | KeyCode::End => {
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::LineEnd,
                count: 1,
            }
        }
        KeyCode::Home => {
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::LineStart,
                count: 1,
            }
        }
        KeyCode::Char('G') => {
            let count = state.count;
            state.clear_pending();
            if let Some(line_num) = count {
                VimKeyResult::MotionWithSelection {
                    motion: VimMotion::GoToLine(line_num),
                    count: 1,
                }
            } else {
                VimKeyResult::MotionWithSelection {
                    motion: VimMotion::DocumentEnd,
                    count: 1,
                }
            }
        }

        // Operators on selection
        KeyCode::Char('d') => {
            state.clear_pending();
            VimKeyResult::VisualOperator {
                operator: VimOperator::Delete,
            }
        }
        KeyCode::Char('y') => {
            state.clear_pending();
            VimKeyResult::VisualOperator {
                operator: VimOperator::Yank,
            }
        }
        KeyCode::Char('c') => {
            state.clear_pending();
            VimKeyResult::VisualOperator {
                operator: VimOperator::Change,
            }
        }

        // Switch to visual line mode
        KeyCode::Char('V') => {
            state.clear_pending();
            VimKeyResult::StartVisualLine
        }

        // Escape exits visual mode
        KeyCode::Esc => {
            state.exit_to_normal();
            VimKeyResult::ExitToNormal
        }

        _ => VimKeyResult::Consumed,
    }
}

/// Handle key in visual line mode.
pub(super) fn handle_visual_line_mode(state: &mut VimState, key: KeyEvent) -> VimKeyResult {
    // Translate Cyrillic to Latin for vim commands
    let key = translate_cyrillic_key(key);

    // Check for Ctrl modifiers
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('u') => {
                let count = state.effective_count();
                state.clear_pending();
                return VimKeyResult::MotionWithSelection {
                    motion: VimMotion::HalfPageUp,
                    count,
                };
            }
            KeyCode::Char('d') => {
                let count = state.effective_count();
                state.clear_pending();
                return VimKeyResult::MotionWithSelection {
                    motion: VimMotion::HalfPageDown,
                    count,
                };
            }
            _ => {}
        }
    }

    match key.code {
        // Count prefix
        KeyCode::Char(ch @ '1'..='9') => {
            state.accumulate_count(ch);
            VimKeyResult::Consumed
        }
        KeyCode::Char(ch @ '0') => {
            state.accumulate_count(ch);
            VimKeyResult::Consumed
        }

        // Vertical motions extend selection
        // j - logical line down
        KeyCode::Char('j') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::Down,
                count,
            }
        }
        // Down arrow - visual line down
        KeyCode::Down => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::VisualDown,
                count,
            }
        }
        // k - logical line up
        KeyCode::Char('k') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::Up,
                count,
            }
        }
        // Up arrow - visual line up
        KeyCode::Up => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::MotionWithSelection {
                motion: VimMotion::VisualUp,
                count,
            }
        }
        KeyCode::Char('G') => {
            let count = state.count;
            state.clear_pending();
            if let Some(line_num) = count {
                VimKeyResult::MotionWithSelection {
                    motion: VimMotion::GoToLine(line_num),
                    count: 1,
                }
            } else {
                VimKeyResult::MotionWithSelection {
                    motion: VimMotion::DocumentEnd,
                    count: 1,
                }
            }
        }

        // Operators on selection (linewise)
        KeyCode::Char('d') => {
            state.clear_pending();
            VimKeyResult::VisualOperator {
                operator: VimOperator::Delete,
            }
        }
        KeyCode::Char('y') => {
            state.clear_pending();
            VimKeyResult::VisualOperator {
                operator: VimOperator::Yank,
            }
        }
        KeyCode::Char('c') => {
            state.clear_pending();
            VimKeyResult::VisualOperator {
                operator: VimOperator::Change,
            }
        }

        // Switch to character-wise visual mode
        KeyCode::Char('v') => {
            state.clear_pending();
            VimKeyResult::StartVisual
        }

        // Escape exits visual mode
        KeyCode::Esc => {
            state.exit_to_normal();
            VimKeyResult::ExitToNormal
        }

        _ => VimKeyResult::Consumed,
    }
}
