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

/// Insert, Delete, Page Up/Down and F1–F12 as xterm sends them, with the
/// modifiers as a second parameter (`CSI 3 ; 5 ~` for Ctrl+Delete, `CSI 1 ;
/// 2 P` for Shift+F1); `None` for other keys.
pub(crate) fn function_key_bytes(code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
    let modifier = arrow_modifier_param(mods);
    let tilde = |n: u8| match modifier {
        Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
        None => format!("\x1b[{n}~").into_bytes(),
    };
    let ss3 = |c: char| match modifier {
        Some(m) => format!("\x1b[1;{m}{c}").into_bytes(),
        None => format!("\x1bO{c}").into_bytes(),
    };
    Some(match code {
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => ss3(['P', 'Q', 'R', 'S'][usize::from(n - 1)]),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => return None,
    })
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

/// The control byte xterm sends for `Ctrl` with `c`, or `None` for a key
/// that has none (`Ctrl+1`, `Ctrl+.`), which then goes as the plain key.
/// The digits 2–8 stand in for the punctuation of the US layout, and a
/// host terminal without a keyboard protocol reports 0x1C–0x1F as
/// `Ctrl+4`..`Ctrl+7`, so `Ctrl+]` may arrive as `Ctrl+5`.
fn control_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => c as u8 & 0x1f,
        ' ' | '@' | '`' | '2' => 0x00,
        '[' | '{' | '3' => 0x1b,
        '\\' | '|' | '4' => 0x1c,
        ']' | '}' | '5' => 0x1d,
        '^' | '~' | '6' => 0x1e,
        '_' | '/' | '-' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// A character or Backspace as the legacy encoding sends it: Ctrl turns a
/// character into its control byte ([`control_byte`]), and Alt puts Escape before the result, as
/// terminals with xterm's metaSendsEscape do. Readline, Emacs and Vim read
/// Alt this way; without the Escape, Alt+D in the shell typed a plain `d`.
pub(crate) fn legacy_key_bytes(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(5);
    if mods.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }
    match code {
        KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => match control_byte(c) {
            Some(byte) => bytes.push(byte),
            None => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        },
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

/// A mouse event at 1-based cell (`x`, `y`) as xterm reports it. SGR
/// (`CSI < b ; x ; y M/m`) names the released button; the X10 encoding
/// (`CSI M b x y`, each byte offset by 32) has release as button 3 and no
/// room for a coordinate past 223, where xterm sends nothing.
pub(crate) fn mouse_report(
    kind: crossterm::event::MouseEventKind,
    mods: KeyModifiers,
    x: u16,
    y: u16,
    sgr: bool,
) -> Option<Vec<u8>> {
    use crossterm::event::{MouseButton, MouseEventKind};

    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (code, release) = match kind {
        MouseEventKind::Down(b) => (button(b), false),
        MouseEventKind::Up(b) if sgr => (button(b), true),
        MouseEventKind::Up(_) => (3, true),
        MouseEventKind::Drag(b) => (32 + button(b), false),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        _ => return None,
    };
    let code = code + mouse_modifier_bits(mods);
    if sgr {
        let suffix = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }
    let byte = |v: u16| u8::try_from(v.saturating_add(32)).ok();
    Some(vec![0x1b, b'[', b'M', code + 32, byte(x)?, byte(y)?])
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

    /// SGR reports which button was released; X10 says 3 and cannot place
    /// a column past 223, so nothing goes rather than a wrong cell.
    #[test]
    fn mouse_reports_name_the_released_button_and_stay_in_range() {
        let none = KeyModifiers::NONE;
        let up = |b| MouseEventKind::Up(b);
        assert_eq!(
            mouse_report(up(MouseButton::Left), none, 5, 7, true).unwrap(),
            b"\x1b[<0;5;7m"
        );
        assert_eq!(
            mouse_report(up(MouseButton::Right), KeyModifiers::CONTROL, 300, 2, true).unwrap(),
            b"\x1b[<18;300;2m"
        );
        assert_eq!(
            mouse_report(MouseEventKind::Down(MouseButton::Middle), none, 1, 1, true).unwrap(),
            b"\x1b[<1;1;1M"
        );
        assert_eq!(
            mouse_report(up(MouseButton::Right), none, 223, 1, false).unwrap(),
            [0x1b, b'[', b'M', 35, 255, 33]
        );
        assert_eq!(
            mouse_report(MouseEventKind::ScrollUp, none, 224, 1, false),
            None
        );
        assert_eq!(
            mouse_report(MouseEventKind::Moved, none, 1, 300, false),
            None
        );
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

    /// Ctrl with punctuation and digits sends xterm's control bytes, not
    /// the low five bits of the character (`Ctrl+/` was 0x0F, `^O`).
    #[test]
    fn control_chords_send_xterms_control_bytes() {
        let ctrl = |c: char| legacy_key_bytes(KeyCode::Char(c), KeyModifiers::CONTROL);
        for (keys, byte) in [
            ("@ `2", 0x00),
            ("[{3", 0x1b),
            ("\\|4", 0x1c),
            ("]}5", 0x1d),
            ("^~6", 0x1e),
            ("_/-7", 0x1f),
            ("?8", 0x7f),
            ("aA", 0x01),
            ("zZ", 0x1a),
        ] {
            for c in keys.chars() {
                assert_eq!(ctrl(c), [byte], "Ctrl+{c}");
            }
        }
        // No control byte: the key itself.
        assert_eq!(ctrl('1'), b"1");
        assert_eq!(ctrl('.'), b".");
        assert_eq!(
            legacy_key_bytes(
                KeyCode::Char('/'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ),
            [0x1b, 0x1f]
        );
    }

    #[test]
    fn function_keys_carry_their_modifiers() {
        let key = |code, mods| function_key_bytes(code, mods).unwrap();
        let none = KeyModifiers::NONE;
        assert_eq!(key(KeyCode::Insert, none), b"\x1b[2~");
        assert_eq!(key(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(key(KeyCode::Delete, KeyModifiers::CONTROL), b"\x1b[3;5~");
        assert_eq!(key(KeyCode::PageUp, KeyModifiers::SHIFT), b"\x1b[5;2~");
        assert_eq!(key(KeyCode::PageDown, none), b"\x1b[6~");
        assert_eq!(key(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(key(KeyCode::F(1), KeyModifiers::SHIFT), b"\x1b[1;2P");
        assert_eq!(key(KeyCode::F(4), KeyModifiers::ALT), b"\x1b[1;3S");
        assert_eq!(key(KeyCode::F(5), KeyModifiers::SHIFT), b"\x1b[15;2~");
        assert_eq!(key(KeyCode::F(12), none), b"\x1b[24~");
        assert_eq!(function_key_bytes(KeyCode::F(13), none), None);
        assert_eq!(function_key_bytes(KeyCode::Tab, none), None);
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
