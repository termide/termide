//! The editing key grammar shared by every text field in the app: single-line
//! inputs ([`TextInput`]) and multi-line areas ([`TextArea`]) alike — the
//! [`InputBar`] fields, the agent's prompt box and the modal inputs all run
//! through it, so one gesture means the same thing wherever it is typed.
//!
//! - Typing, `Backspace`, `Delete`
//! - Character and word navigation (`Ctrl+arrows`), `Home`/`End`
//! - Selection: `Shift+arrows`, `Shift+Home`/`End`, `Ctrl+Shift+arrows`,
//!   `Ctrl+A`
//! - Clipboard: `Ctrl+C`/`Ctrl+X`/`Ctrl+V`
//! - Undo/redo: `Ctrl+Z`, `Ctrl+Y`, `Ctrl+Shift+Z`
//!
//! A `Ctrl` chord is matched on its layout-normalized form, so `Ctrl+Ф` is
//! `Ctrl+A` on a Russian layout; typed text keeps the key as it came.
//!
//! `Enter`, `Tab` and `Esc` belong to the host, not to the field: a prompt
//! sends on `Enter`, a search bar submits, a bar closes. They are deliberately
//! absent here, so a host keeps that decision.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use termide_core::KeyNormalizer;

use crate::{TextArea, TextInput};

/// What an editing key did to a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldEdit {
    /// The text changed.
    Edited,
    /// The cursor or the selection moved, or text went to the clipboard.
    Navigated,
    /// Not an editing key; the host handles it.
    NotHandled,
}

/// A `Ctrl` chord in the form the grammar matches: layout-normalized, as the
/// app's hotkeys are. Any other key is text, and stays raw.
fn shortcut_form(key: KeyEvent) -> KeyEvent {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        KeyNormalizer::default().canonicalize(key)
    } else {
        key
    }
}

/// Apply an editing key to a single-line input.
pub fn edit_text_input(input: &mut TextInput, key: KeyEvent) -> FieldEdit {
    let key = shortcut_form(key);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char(c) if !ctrl => {
            input.insert(c);
            FieldEdit::Edited
        }
        KeyCode::Char('a') if ctrl => {
            input.select_all();
            FieldEdit::Navigated
        }
        KeyCode::Char('c') if ctrl => copy_selection(input.selected_text()),
        KeyCode::Char('x') if ctrl => {
            let text = input.selected_text().map(str::to_string);
            if cut(text.as_deref()) {
                input.delete_selection();
                FieldEdit::Edited
            } else {
                FieldEdit::NotHandled
            }
        }
        KeyCode::Char('v') if ctrl => match crate::clipboard::paste_text_or_paths() {
            Some(text) => {
                input.paste(&text);
                FieldEdit::Edited
            }
            None => FieldEdit::NotHandled,
        },
        KeyCode::Char('z') if ctrl && shift => changed(input.redo()),
        KeyCode::Char('z') | KeyCode::Char('y') if ctrl => changed(input.undo()),
        KeyCode::Backspace => changed(input.backspace()),
        KeyCode::Delete => changed(input.delete()),
        KeyCode::Left if ctrl && shift => {
            input.move_word_left_with_selection();
            FieldEdit::Navigated
        }
        KeyCode::Right if ctrl && shift => {
            input.move_word_right_with_selection();
            FieldEdit::Navigated
        }
        KeyCode::Left if shift => moved(input.move_left_with_selection()),
        KeyCode::Right if shift => moved(input.move_right_with_selection()),
        KeyCode::Left if ctrl => {
            input.move_word_left();
            FieldEdit::Navigated
        }
        KeyCode::Right if ctrl => {
            input.move_word_right();
            FieldEdit::Navigated
        }
        KeyCode::Left => moved(input.move_left()),
        KeyCode::Right => moved(input.move_right()),
        KeyCode::Home if shift => {
            input.move_home_with_selection();
            FieldEdit::Navigated
        }
        KeyCode::End if shift => {
            input.move_end_with_selection();
            FieldEdit::Navigated
        }
        KeyCode::Home => {
            input.move_home();
            FieldEdit::Navigated
        }
        KeyCode::End => {
            input.move_end();
            FieldEdit::Navigated
        }
        _ => FieldEdit::NotHandled,
    }
}

/// Apply an editing key to a multi-line text area: the same grammar, with the
/// arrows walking lines and the selection spanning them.
pub fn edit_text_area(area: &mut TextArea, key: KeyEvent) -> FieldEdit {
    let key = shortcut_form(key);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char(c) if !ctrl => {
            area.insert(c);
            FieldEdit::Edited
        }
        KeyCode::Char('a') if ctrl => {
            area.select_all();
            FieldEdit::Navigated
        }
        KeyCode::Char('c') if ctrl => copy_selection(area.selected_text().as_deref()),
        KeyCode::Char('x') if ctrl => {
            let text = area.selected_text();
            if cut(text.as_deref()) {
                area.delete_selection();
                FieldEdit::Edited
            } else {
                FieldEdit::NotHandled
            }
        }
        KeyCode::Char('v') if ctrl => match crate::clipboard::paste_text_or_paths() {
            Some(text) => {
                area.insert_str(&text);
                FieldEdit::Edited
            }
            None => FieldEdit::NotHandled,
        },
        KeyCode::Char('z') if ctrl && shift => changed(area.redo()),
        KeyCode::Char('z') | KeyCode::Char('y') if ctrl => changed(area.undo()),
        KeyCode::Backspace => changed(area.backspace()),
        KeyCode::Delete => changed(area.delete()),
        KeyCode::Left if ctrl && shift => moved(area.move_word_left_with_selection()),
        KeyCode::Right if ctrl && shift => moved(area.move_word_right_with_selection()),
        KeyCode::Left if shift => moved(area.move_left_with_selection()),
        KeyCode::Right if shift => moved(area.move_right_with_selection()),
        KeyCode::Up if shift => moved(area.move_up_with_selection()),
        KeyCode::Down if shift => moved(area.move_down_with_selection()),
        KeyCode::Left if ctrl => moved(area.move_word_left()),
        KeyCode::Right if ctrl => moved(area.move_word_right()),
        KeyCode::Left => moved(area.move_left()),
        KeyCode::Right => moved(area.move_right()),
        KeyCode::Up if !ctrl => moved(area.move_up()),
        KeyCode::Down if !ctrl => moved(area.move_down()),
        KeyCode::Home if ctrl && shift => {
            area.start_selection();
            area.move_to_start();
            FieldEdit::Navigated
        }
        KeyCode::End if ctrl && shift => {
            area.start_selection();
            area.move_to_end();
            FieldEdit::Navigated
        }
        KeyCode::Home if shift => {
            area.move_home_with_selection();
            FieldEdit::Navigated
        }
        KeyCode::End if shift => {
            area.move_end_with_selection();
            FieldEdit::Navigated
        }
        KeyCode::Home if ctrl => {
            area.move_to_start();
            FieldEdit::Navigated
        }
        KeyCode::End if ctrl => {
            area.move_to_end();
            FieldEdit::Navigated
        }
        KeyCode::Home => {
            area.move_home();
            FieldEdit::Navigated
        }
        KeyCode::End => {
            area.move_end();
            FieldEdit::Navigated
        }
        _ => FieldEdit::NotHandled,
    }
}

/// Put `text` on the clipboard. A copy is handled whether or not it succeeded:
/// the key belongs to the field either way, and a `Ctrl+C` that fell through
/// would go on to mean something else entirely.
fn copy_selection(text: Option<&str>) -> FieldEdit {
    match text {
        Some(text) => {
            let _ = crate::clipboard::copy(text);
            FieldEdit::Navigated
        }
        None => FieldEdit::NotHandled,
    }
}

/// Whether there was a selection to cut; the caller removes it.
fn cut(text: Option<&str>) -> bool {
    match text {
        Some(text) => {
            let _ = crate::clipboard::copy(text);
            true
        }
        None => false,
    }
}

/// A deletion or an undo that found nothing to do: nothing changed, but the key
/// belonged to the field and is still the field's to swallow.
fn changed(did: bool) -> FieldEdit {
    if did {
        FieldEdit::Edited
    } else {
        FieldEdit::Navigated
    }
}

/// A cursor or selection move that may have run into an edge.
fn moved(did: bool) -> FieldEdit {
    if did {
        FieldEdit::Navigated
    } else {
        FieldEdit::NotHandled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_chord_works_on_a_cyrillic_layout_and_text_stays_as_typed() {
        let mut input = TextInput::with_text("abc");
        // `Ctrl+Ф` is where `Ctrl+A` sits on a Russian layout: select all.
        let select_all = KeyEvent::new(KeyCode::Char('ф'), KeyModifiers::CONTROL);
        assert_eq!(
            edit_text_input(&mut input, select_all),
            FieldEdit::Navigated
        );
        assert_eq!(input.selected_text(), Some("abc"));
        // A typed Cyrillic letter is text, not its Latin twin.
        let typed = KeyEvent::new(KeyCode::Char('ф'), KeyModifiers::NONE);
        assert_eq!(edit_text_input(&mut input, typed), FieldEdit::Edited);
        assert_eq!(input.text(), "ф");
    }
}
