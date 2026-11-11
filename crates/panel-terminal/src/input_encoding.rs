//! Keyboard byte-sequence encoding and mouse-event routing for the terminal
//! panel. Pure functions over crossterm input types — no terminal state — so
//! they live apart from the `Terminal` panel and are unit-tested in `lib.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::terminal::{KeyboardProtocolMode, MouseTrackingMode};

/// Encode key modifiers as an xterm CSI parameter for modified arrow / Home /
/// End keys: final byte is preceded by `1;<param>`. Returns `None` when no
/// modifier is held, so callers fall back to the plain sequence.
///
/// xterm protocol:
///   2 = Shift, 3 = Alt, 4 = Shift+Alt,
///   5 = Ctrl,  6 = Ctrl+Shift, 7 = Ctrl+Alt, 8 = Ctrl+Shift+Alt.
pub(crate) fn arrow_modifier_param(mods: KeyModifiers) -> Option<u8> {
    let shift = mods.contains(KeyModifiers::SHIFT);
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match (shift, alt, ctrl) {
        (false, false, false) => None,
        (true, false, false) => Some(2),
        (false, true, false) => Some(3),
        (true, true, false) => Some(4),
        (false, false, true) => Some(5),
        (true, false, true) => Some(6),
        (false, true, true) => Some(7),
        (true, true, true) => Some(8),
    }
}

fn keyboard_modifier_param(mods: KeyModifiers) -> u8 {
    let mut value = 1;
    if mods.contains(KeyModifiers::SHIFT) {
        value += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        value += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        value += 4;
    }
    if mods.contains(KeyModifiers::SUPER) {
        value += 8;
    }
    value
}

fn encode_csi_u(codepoint: u32, mods: KeyModifiers) -> Vec<u8> {
    if mods.is_empty() {
        format!("\x1b[{codepoint}u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{}u", keyboard_modifier_param(mods)).into_bytes()
    }
}

/// The key as it goes to the PTY. `Ctrl` with a letter of another layout
/// (`Ctrl+С` on a Russian one) is that key's Latin control chord (`Ctrl+C`,
/// `^C`), as terminals send it: a non-ASCII codepoint has no control code,
/// and masking one would send some other control byte. Everything else goes
/// as it was typed.
pub(crate) fn pty_key(chord: termide_core::KeyChord) -> KeyEvent {
    let raw = chord.raw;
    if raw.modifiers.contains(KeyModifiers::CONTROL) {
        if let (KeyCode::Char(c), KeyCode::Char(latin)) = (raw.code, chord.canonical.code) {
            if !c.is_ascii() && latin.is_ascii() {
                return KeyEvent {
                    code: KeyCode::Char(latin),
                    ..raw
                };
            }
        }
    }
    raw
}

/// A character or Backspace as the legacy encoding sends it: Ctrl masks a
/// character to its control byte, and Alt puts Escape before the result, as
/// terminals with xterm's metaSendsEscape do. Readline, Emacs and Vim read
/// Alt this way; without the Escape, Alt+D in the shell typed a plain `d`.
pub(crate) fn legacy_key_bytes(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(5);
    if mods.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }
    match code {
        KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => bytes.push((c as u8) & 0x1f),
        KeyCode::Char(c) => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        KeyCode::Backspace => bytes.push(0x7f),
        _ => {}
    }
    bytes
}

pub(crate) fn modern_key_bytes(key: &KeyEvent, mode: KeyboardProtocolMode) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => match mode {
            KeyboardProtocolMode::Legacy => None,
            KeyboardProtocolMode::CsiUCompat => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    None
                } else {
                    Some(encode_csi_u(c as u32, key.modifiers))
                }
            }
            KeyboardProtocolMode::ModifyOtherKeys2 => {
                if key.modifiers.is_empty() {
                    None
                } else {
                    Some(encode_csi_u(c as u32, key.modifiers))
                }
            }
        },
        KeyCode::Enter => {
            if key.modifiers.is_empty() || mode == KeyboardProtocolMode::Legacy {
                None
            } else {
                Some(encode_csi_u(13, key.modifiers))
            }
        }
        KeyCode::Tab => {
            if key.modifiers.is_empty() || mode == KeyboardProtocolMode::Legacy {
                None
            } else {
                Some(encode_csi_u(9, key.modifiers))
            }
        }
        KeyCode::Backspace => {
            if key.modifiers.is_empty() || mode == KeyboardProtocolMode::Legacy {
                None
            } else {
                Some(encode_csi_u(127, key.modifiers))
            }
        }
        KeyCode::Esc => {
            if key.modifiers.is_empty() || mode == KeyboardProtocolMode::Legacy {
                None
            } else {
                Some(encode_csi_u(27, key.modifiers))
            }
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseRoute {
    LocalSelection,
    LocalScrollback,
    Pty,
    Ignore,
}

pub(crate) fn mouse_modifier_bits(mods: KeyModifiers) -> u8 {
    let mut bits = 0;
    if mods.contains(KeyModifiers::SHIFT) {
        bits |= 4;
    }
    if mods.contains(KeyModifiers::ALT) {
        bits |= 8;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        bits |= 16;
    }
    bits
}

pub(crate) fn can_send_mouse_event(
    kind: crossterm::event::MouseEventKind,
    mode: MouseTrackingMode,
) -> bool {
    use crossterm::event::MouseEventKind;

    match kind {
        MouseEventKind::Down(_) | MouseEventKind::Up(_) => mode != MouseTrackingMode::None,
        MouseEventKind::Drag(_) => {
            matches!(
                mode,
                MouseTrackingMode::ButtonEvent | MouseTrackingMode::AnyEvent
            )
        }
        MouseEventKind::Moved => mode == MouseTrackingMode::AnyEvent,
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => mode != MouseTrackingMode::None,
        _ => false,
    }
}

pub(crate) fn mouse_route(
    kind: crossterm::event::MouseEventKind,
    is_inside: bool,
    selection_active: bool,
    mouse_tracking: MouseTrackingMode,
    alt_pressed: bool,
) -> MouseRoute {
    use crossterm::event::{MouseButton, MouseEventKind};

    match kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if mouse_tracking == MouseTrackingMode::None {
                MouseRoute::LocalScrollback
            } else if is_inside {
                MouseRoute::Pty
            } else {
                MouseRoute::Ignore
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if !is_inside {
                MouseRoute::Ignore
            } else if mouse_tracking != MouseTrackingMode::None && !alt_pressed {
                MouseRoute::Pty
            } else {
                MouseRoute::LocalSelection
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if selection_active {
                MouseRoute::LocalSelection
            } else if !is_inside {
                MouseRoute::Ignore
            } else if mouse_tracking != MouseTrackingMode::None && !alt_pressed {
                MouseRoute::Pty
            } else {
                MouseRoute::LocalSelection
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if selection_active {
                MouseRoute::LocalSelection
            } else if is_inside && mouse_tracking != MouseTrackingMode::None && !alt_pressed {
                MouseRoute::Pty
            } else {
                MouseRoute::Ignore
            }
        }
        MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => {
            if is_inside && mouse_tracking != MouseTrackingMode::None {
                MouseRoute::Pty
            } else {
                MouseRoute::Ignore
            }
        }
        MouseEventKind::Moved => {
            if is_inside && mouse_tracking == MouseTrackingMode::AnyEvent {
                MouseRoute::Pty
            } else {
                MouseRoute::Ignore
            }
        }
        _ => MouseRoute::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEventKind};

    #[test]
    fn mouse_route_prefers_pty_when_tracking_enabled() {
        assert_eq!(
            mouse_route(
                MouseEventKind::Down(MouseButton::Left),
                true,
                false,
                MouseTrackingMode::Normal,
                false,
            ),
            MouseRoute::Pty
        );
        assert_eq!(
            mouse_route(
                MouseEventKind::Drag(MouseButton::Left),
                true,
                false,
                MouseTrackingMode::ButtonEvent,
                false,
            ),
            MouseRoute::Pty
        );
        assert_eq!(
            mouse_route(
                MouseEventKind::Moved,
                true,
                false,
                MouseTrackingMode::AnyEvent,
                false,
            ),
            MouseRoute::Pty
        );
    }

    #[test]
    fn mouse_route_keeps_alt_drag_for_local_selection() {
        assert_eq!(
            mouse_route(
                MouseEventKind::Down(MouseButton::Left),
                true,
                false,
                MouseTrackingMode::ButtonEvent,
                true,
            ),
            MouseRoute::LocalSelection
        );
        assert_eq!(
            mouse_route(
                MouseEventKind::Drag(MouseButton::Left),
                true,
                true,
                MouseTrackingMode::ButtonEvent,
                true,
            ),
            MouseRoute::LocalSelection
        );
    }

    #[test]
    fn can_send_mouse_event_respects_tracking_mode() {
        assert!(!can_send_mouse_event(
            MouseEventKind::Moved,
            MouseTrackingMode::ButtonEvent
        ));
        assert!(can_send_mouse_event(
            MouseEventKind::Moved,
            MouseTrackingMode::AnyEvent
        ));
        assert!(can_send_mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            MouseTrackingMode::ButtonEvent
        ));
        assert!(!can_send_mouse_event(
            MouseEventKind::Drag(MouseButton::Left),
            MouseTrackingMode::Normal
        ));
    }

    #[test]
    fn mouse_modifier_bits_match_xterm_encoding() {
        assert_eq!(mouse_modifier_bits(KeyModifiers::empty()), 0);
        assert_eq!(mouse_modifier_bits(KeyModifiers::SHIFT), 4);
        assert_eq!(mouse_modifier_bits(KeyModifiers::ALT), 8);
        assert_eq!(mouse_modifier_bits(KeyModifiers::CONTROL), 16);
        assert_eq!(
            mouse_modifier_bits(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL),
            28
        );
    }

    #[test]
    fn a_control_chord_of_another_layout_reaches_the_pty_as_its_latin_key() {
        let chord = |c: char, mods: KeyModifiers| {
            termide_core::KeyChord::new(
                KeyEvent::new(KeyCode::Char(c), mods),
                &termide_core::KeyNormalizer::default(),
            )
        };
        // `Ctrl+С` on a Russian layout is `^C`, not whatever masking U+0441
        // would give.
        assert_eq!(
            pty_key(chord('с', KeyModifiers::CONTROL)).code,
            KeyCode::Char('c')
        );
        // Typed Cyrillic text, and a Latin chord, go as they are.
        assert_eq!(
            pty_key(chord('с', KeyModifiers::NONE)).code,
            KeyCode::Char('с')
        );
        assert_eq!(
            pty_key(chord('+', KeyModifiers::CONTROL)).code,
            KeyCode::Char('+')
        );
    }

    /// Alt sends Escape before the key, as xterm's metaSendsEscape and every
    /// common terminal do: readline's `M-d`, `M-.` and `M-DEL` read it so.
    #[test]
    fn legacy_bytes_prefix_escape_for_alt() {
        let key = |code: KeyCode, mods: KeyModifiers| legacy_key_bytes(code, mods);
        assert_eq!(key(KeyCode::Char('d'), KeyModifiers::NONE), b"d");
        assert_eq!(key(KeyCode::Char('d'), KeyModifiers::ALT), b"\x1bd");
        assert_eq!(key(KeyCode::Char('.'), KeyModifiers::ALT), b"\x1b.");
        assert_eq!(
            key(KeyCode::Char('ф'), KeyModifiers::ALT),
            "\x1bф".as_bytes()
        );
        assert_eq!(key(KeyCode::Char('x'), KeyModifiers::CONTROL), [0x18]);
        assert_eq!(
            key(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ),
            [0x1b, 0x18]
        );
        assert_eq!(key(KeyCode::Backspace, KeyModifiers::NONE), [0x7f]);
        assert_eq!(key(KeyCode::Backspace, KeyModifiers::ALT), [0x1b, 0x7f]);
    }

    #[test]
    fn modern_key_bytes_encode_ambiguous_keys() {
        let ctrl_i = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL);
        assert_eq!(
            modern_key_bytes(&ctrl_i, KeyboardProtocolMode::CsiUCompat),
            Some(b"\x1b[105;5u".to_vec())
        );

        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            modern_key_bytes(&shift_enter, KeyboardProtocolMode::CsiUCompat),
            Some(b"\x1b[13;2u".to_vec())
        );

        let plain_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(
            modern_key_bytes(&plain_a, KeyboardProtocolMode::CsiUCompat),
            None
        );

        let shifted_a = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(
            modern_key_bytes(&shifted_a, KeyboardProtocolMode::ModifyOtherKeys2),
            Some(b"\x1b[65;2u".to_vec())
        );
    }
}
