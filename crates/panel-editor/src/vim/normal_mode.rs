//! Vim normal-mode key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::key_handler::{translate_cyrillic_key, InsertPosition, VimFold, VimKeyResult};
use super::motions::VimMotion;
use super::operators::VimOperator;
use super::state::VimState;
use super::PanelDirection;

/// Handle key in normal mode.
pub(super) fn handle_normal_mode(state: &mut VimState, key: KeyEvent) -> VimKeyResult {
    // Translate Cyrillic to Latin for vim commands
    let key = translate_cyrillic_key(key);

    // Check for Ctrl+w prefix for panel navigation
    if !state.partial_keys.is_empty() && state.partial_keys[0] == '\x17' {
        // Ctrl+W was pressed, waiting for h/j/k/l; whatever follows ends
        // the command, a count or an operator before it too.
        state.clear_pending();
        return match key.code {
            KeyCode::Char('h') => VimKeyResult::PanelNavigation(PanelDirection::Left),
            KeyCode::Char('j') => VimKeyResult::PanelNavigation(PanelDirection::Down),
            KeyCode::Char('k') => VimKeyResult::PanelNavigation(PanelDirection::Up),
            KeyCode::Char('l') => VimKeyResult::PanelNavigation(PanelDirection::Right),
            _ => VimKeyResult::Consumed,
        };
    }

    // Check for 'g' prefix
    if !state.partial_keys.is_empty() && state.partial_keys[0] == 'g' {
        state.partial_keys.clear();
        return match key.code {
            KeyCode::Char('g') => {
                // gg - go to document start (or line if count specified)
                let motion = match state.count {
                    Some(line) => VimMotion::GoToLine(line),
                    None => VimMotion::DocumentStart,
                };
                let operator = state.take_pending_operator();
                state.clear_pending();
                document_motion(operator, motion)
            }
            // gj - visual line down (respects word wrap)
            KeyCode::Char('j') => {
                let count = state.effective_count();
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::VisualDown,
                    count,
                }
            }
            // gk - visual line up (respects word wrap)
            KeyCode::Char('k') => {
                let count = state.effective_count();
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::VisualUp,
                    count,
                }
            }
            // Anything else drops the whole command, as Escape does.
            _ => {
                state.clear_pending();
                VimKeyResult::Consumed
            }
        };
    }

    // Check for 'z' prefix (folds)
    if !state.partial_keys.is_empty() && state.partial_keys[0] == 'z' {
        state.clear_pending();
        let fold = match key.code {
            KeyCode::Char('a') => VimFold::Toggle,
            KeyCode::Char('c') => VimFold::Close,
            KeyCode::Char('o') => VimFold::Open,
            KeyCode::Char('R') => VimFold::OpenAll,
            KeyCode::Char('M') => VimFold::CloseAll,
            _ => return VimKeyResult::Consumed,
        };
        return VimKeyResult::Fold(fold);
    }

    // Handle Ctrl+W (start panel navigation sequence)
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('w') => {
                state.push_partial_key('\x17'); // Ctrl+W marker
                return VimKeyResult::Consumed;
            }
            KeyCode::Char('u') => {
                let count = state.effective_count();
                state.clear_pending();
                return VimKeyResult::Motion {
                    motion: VimMotion::HalfPageUp,
                    count,
                };
            }
            KeyCode::Char('d') => {
                let count = state.effective_count();
                state.clear_pending();
                return VimKeyResult::Motion {
                    motion: VimMotion::HalfPageDown,
                    count,
                };
            }
            KeyCode::Char('r') => {
                state.clear_pending();
                return VimKeyResult::Redo;
            }
            _ => {
                state.clear_pending();
                return VimKeyResult::PassThrough;
            }
        }
    }

    match key.code {
        // Count prefix (1-9, or 0 after other digits)
        KeyCode::Char(ch @ '0'..='9') => {
            if state.accumulate_count(ch) {
                VimKeyResult::Consumed
            } else {
                // '0' at start means line start
                line_start(state)
            }
        }

        // Basic motions
        KeyCode::Char('h') | KeyCode::Left => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::Left,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::Left,
                    count,
                }
            }
        }
        // j - logical line down (by buffer lines)
        KeyCode::Char('j') => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                // j with operator is linewise
                VimKeyResult::LinewiseMotion {
                    operator: op,
                    motion: VimMotion::Down,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::Down,
                    count,
                }
            }
        }
        // Down arrow - visual line down (respects word wrap)
        KeyCode::Down => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::LinewiseMotion {
                    operator: op,
                    motion: VimMotion::Down,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::VisualDown,
                    count,
                }
            }
        }
        // k - logical line up (by buffer lines)
        KeyCode::Char('k') => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                // k with operator is linewise
                VimKeyResult::LinewiseMotion {
                    operator: op,
                    motion: VimMotion::Up,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::Up,
                    count,
                }
            }
        }
        // Up arrow - visual line up (respects word wrap)
        KeyCode::Up => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::LinewiseMotion {
                    operator: op,
                    motion: VimMotion::Up,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::VisualUp,
                    count,
                }
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::Right,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::Right,
                    count,
                }
            }
        }

        // Word motions
        KeyCode::Char('w') => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::WordForward,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::WordForward,
                    count,
                }
            }
        }
        KeyCode::Char('b') => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::WordBackward,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::WordBackward,
                    count,
                }
            }
        }
        KeyCode::Char('e') => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::WordEnd,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::WordEnd,
                    count,
                }
            }
        }

        // Line position motions
        KeyCode::Char('^') => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::FirstNonBlank,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::FirstNonBlank,
                    count,
                }
            }
        }
        KeyCode::Char('$') | KeyCode::End => {
            let count = state.effective_count();
            if let Some(op) = state.take_pending_operator() {
                state.clear_pending();
                VimKeyResult::OperatorMotion {
                    operator: op,
                    motion: VimMotion::LineEnd,
                    count,
                }
            } else {
                state.clear_pending();
                VimKeyResult::Motion {
                    motion: VimMotion::LineEnd,
                    count,
                }
            }
        }
        KeyCode::Home => line_start(state),

        // Document motions
        KeyCode::Char('g') => {
            state.push_partial_key('g');
            VimKeyResult::Consumed
        }
        KeyCode::Char('z') => {
            state.push_partial_key('z');
            VimKeyResult::Consumed
        }
        KeyCode::Char('G') => {
            let motion = match state.count {
                Some(line) => VimMotion::GoToLine(line),
                None => VimMotion::DocumentEnd,
            };
            let operator = state.take_pending_operator();
            state.clear_pending();
            document_motion(operator, motion)
        }

        // Operators
        KeyCode::Char('d') => {
            if state.pending_operator == Some(VimOperator::Delete) {
                // dd - delete line
                let count = state.effective_count();
                state.clear_pending();
                VimKeyResult::LinewiseOperator {
                    operator: VimOperator::Delete,
                    count,
                }
            } else {
                state.set_pending_operator(VimOperator::Delete);
                VimKeyResult::Consumed
            }
        }
        KeyCode::Char('y') => {
            if state.pending_operator == Some(VimOperator::Yank) {
                // yy - yank line
                let count = state.effective_count();
                state.clear_pending();
                VimKeyResult::LinewiseOperator {
                    operator: VimOperator::Yank,
                    count,
                }
            } else {
                state.set_pending_operator(VimOperator::Yank);
                VimKeyResult::Consumed
            }
        }
        KeyCode::Char('c') => {
            if state.pending_operator == Some(VimOperator::Change) {
                // cc - change line
                let count = state.effective_count();
                state.clear_pending();
                VimKeyResult::LinewiseOperator {
                    operator: VimOperator::Change,
                    count,
                }
            } else {
                state.set_pending_operator(VimOperator::Change);
                VimKeyResult::Consumed
            }
        }

        // Delete character (x)
        KeyCode::Char('x') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::DeleteChar { count }
        }

        // Paste
        KeyCode::Char('p') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::Paste { after: true, count }
        }
        KeyCode::Char('P') => {
            let count = state.effective_count();
            state.clear_pending();
            VimKeyResult::Paste {
                after: false,
                count,
            }
        }

        // Undo
        KeyCode::Char('u') => {
            state.clear_pending();
            VimKeyResult::Undo
        }

        // Insert mode entry
        KeyCode::Char('i') => {
            state.clear_pending();
            VimKeyResult::EnterInsert(InsertPosition::BeforeCursor)
        }
        KeyCode::Char('a') => {
            state.clear_pending();
            VimKeyResult::EnterInsert(InsertPosition::AfterCursor)
        }
        KeyCode::Char('I') => {
            state.clear_pending();
            VimKeyResult::EnterInsert(InsertPosition::LineStart)
        }
        KeyCode::Char('A') => {
            state.clear_pending();
            VimKeyResult::EnterInsert(InsertPosition::LineEnd)
        }
        KeyCode::Char('o') => {
            state.clear_pending();
            VimKeyResult::EnterInsert(InsertPosition::NewLineBelow)
        }
        KeyCode::Char('O') => {
            state.clear_pending();
            VimKeyResult::EnterInsert(InsertPosition::NewLineAbove)
        }

        // Visual mode entry
        KeyCode::Char('v') => {
            state.clear_pending();
            VimKeyResult::StartVisual
        }
        KeyCode::Char('V') => {
            state.clear_pending();
            VimKeyResult::StartVisualLine
        }

        // Escape clears pending operations
        KeyCode::Esc => {
            state.clear_pending();
            VimKeyResult::Consumed
        }

        // F-keys pass through (F3/Shift+F3 for search next/prev, etc.)
        KeyCode::F(_) => {
            state.clear_pending();
            VimKeyResult::PassThrough
        }

        _ => {
            state.clear_pending();
            VimKeyResult::Unhandled
        }
    }
}

/// A move to a line of the document (G, gg), or with an operator the whole
/// lines from the cursor's to that one.
fn document_motion(operator: Option<VimOperator>, motion: VimMotion) -> VimKeyResult {
    match operator {
        Some(operator) => VimKeyResult::LinewiseMotion {
            operator,
            motion,
            count: 1,
        },
        None => VimKeyResult::Motion { motion, count: 1 },
    }
}

/// A move to the line's first column (0, Home), or with an operator the
/// characters before the cursor.
fn line_start(state: &mut VimState) -> VimKeyResult {
    let operator = state.take_pending_operator();
    state.clear_pending();
    match operator {
        Some(operator) => VimKeyResult::OperatorMotion {
            operator,
            motion: VimMotion::LineStart,
            count: 1,
        },
        None => VimKeyResult::Motion {
            motion: VimMotion::LineStart,
            count: 1,
        },
    }
}
