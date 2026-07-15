//! Keybindings configuration for termide.
//!
//! Supports configurable keyboard shortcuts via config.toml sections like:
//! ```toml
//! [general.keybindings]
//! toggle_menu = "Alt+M"
//! new_terminal = "Alt+T"
//!
//! [editor.keybindings]
//! save = "Ctrl+S"
//! copy_files = ["C", "F5"]  # multiple bindings
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use termide_keyboard::{cyrillic_to_latin_opt, unshifted_punctuation, KeyNormalizer};

mod sections;
mod vim;
pub use sections::*;
pub use vim::*;

/// A keybinding that can be either a single key or multiple alternatives.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum KeyBinding {
    /// Single keybinding: "Ctrl+S"
    Single(String),
    /// Multiple alternatives: ["C", "F5"]
    Multiple(Vec<String>),
}

impl KeyBinding {
    /// Check if a key event matches this binding.
    ///
    /// **Convenience wrapper**: canonicalizes `event` with a default
    /// (no-caps) `KeyNormalizer` before strict comparison. Callers that
    /// already hold a `KeyChord` should compare against `chord.canonical`
    /// directly via [`ParsedKeyBinding::matches`] for clarity and to
    /// honour the active terminal capabilities.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        let normalizer = KeyNormalizer::default();
        let canonical = normalizer.canonicalize(*event);
        self.matches_canonical(&canonical)
    }

    /// Strict match against an already-canonical event.
    pub fn matches_canonical(&self, canonical: &KeyEvent) -> bool {
        match self {
            KeyBinding::Single(s) => parse_keybinding(s)
                .map(|p| p.matches(canonical))
                .unwrap_or(false),
            KeyBinding::Multiple(bindings) => bindings.iter().any(|s| {
                parse_keybinding(s)
                    .map(|p| p.matches(canonical))
                    .unwrap_or(false)
            }),
        }
    }

    /// Parse into a list of ParsedKeyBindings.
    pub fn parse(&self) -> Vec<ParsedKeyBinding> {
        match self {
            KeyBinding::Single(s) => parse_keybinding(s).into_iter().collect(),
            KeyBinding::Multiple(bindings) => bindings
                .iter()
                .filter_map(|s| parse_keybinding(s).ok())
                .collect(),
        }
    }

    /// Get the first keybinding as a display string.
    pub fn display(&self) -> &str {
        match self {
            KeyBinding::Single(s) => s.as_str(),
            KeyBinding::Multiple(v) => v.first().map(|s| s.as_str()).unwrap_or(""),
        }
    }
}

/// A parsed keybinding ready for runtime matching.
///
/// Always stored in canonical form: parse-time normalization rewrites
/// shifted punctuation glyphs (`+`, `_`, `?`, …) into `Shift+<unshifted>`
/// and Cyrillic letters into Latin. This keeps matching a strict
/// equality check with no alternative-paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParsedKeyBinding {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
}

impl ParsedKeyBinding {
    /// Strict canonical equality. Both operands must be canonical
    /// (parse-time normalization for `self`, `KeyNormalizer::canonicalize`
    /// for `event`).
    pub fn matches(&self, event: &KeyEvent) -> bool {
        let key_eq = match (&self.key, &event.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a == b,
        };
        key_eq && self.modifiers == event.modifiers
    }
}

/// Parse a keybinding string like "Ctrl+Shift+S" into a canonical
/// `ParsedKeyBinding`.
///
/// Canonicalization rules applied at parse time (mirroring
/// `KeyNormalizer::canonicalize` for `KeyEvent`):
/// - Cyrillic letter → Latin equivalent on the same physical key.
/// - Shifted-glyph punctuation (`+`, `_`, `!`, …) → `Shift+<unshifted>`.
///
/// As a result, two strings that name the same physical chord parse to
/// the same `ParsedKeyBinding`: e.g. `"Alt++"` ≡ `"Alt+Shift+="`,
/// `"Ctrl+Й"` ≡ `"Ctrl+Q"`.
pub fn parse_keybinding(s: &str) -> Result<ParsedKeyBinding, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty keybinding".to_string());
    }

    // The literal `+` key is awkward because `+` is also our modifier
    // separator. Split off the key first by looking at the trailing
    // characters:
    //   "+"         → key='+', no modifiers.
    //   "Alt++"     → key='+', mods="Alt".
    //   "Ctrl+S"    → key='S', mods="Ctrl".
    //   "Alt+"      → invalid (legacy behaviour).
    let (mods_part, key_part) = if s == "+" {
        ("", "+")
    } else if let Some(stripped) = s.strip_suffix('+') {
        if let Some(prefix) = stripped.strip_suffix('+') {
            (prefix, "+")
        } else {
            return Err("Empty keybinding".to_string());
        }
    } else if let Some(idx) = s.rfind('+') {
        (&s[..idx], &s[idx + 1..])
    } else {
        ("", s)
    };

    let mut modifiers = KeyModifiers::empty();
    if !mods_part.is_empty() {
        for part in mods_part.split('+') {
            let lower = part.trim().to_lowercase();
            match lower.as_str() {
                "" => {} // Tolerate empty segments like in "Alt++Ctrl".
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                other => return Err(format!("Unknown modifier: {}", other)),
            }
        }
    }

    let key = parse_key(key_part.trim())?;

    let mut parsed = ParsedKeyBinding { key, modifiers };
    canonicalize_parsed(&mut parsed);
    Ok(parsed)
}

/// Apply parse-time canonicalization in-place: Cyrillic→Latin and
/// shifted-glyph punctuation → `Shift+<unshifted>`.
fn canonicalize_parsed(parsed: &mut ParsedKeyBinding) {
    if let KeyCode::Char(c) = parsed.key {
        if let Some(latin) = cyrillic_to_latin_opt(c) {
            parsed.key = KeyCode::Char(latin);
        }
    }
    if let KeyCode::Char(c) = parsed.key {
        if let Some(unshifted) = unshifted_punctuation(c) {
            parsed.key = KeyCode::Char(unshifted);
            parsed.modifiers |= KeyModifiers::SHIFT;
        }
    }
}

/// Parse a key name into a KeyCode.
fn parse_key(s: &str) -> Result<KeyCode, String> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        // Special keys
        "enter" | "return" => Ok(KeyCode::Enter),
        "esc" | "escape" => Ok(KeyCode::Esc),
        "tab" => Ok(KeyCode::Tab),
        "space" => Ok(KeyCode::Char(' ')),
        "backspace" | "bs" => Ok(KeyCode::Backspace),
        "delete" | "del" => Ok(KeyCode::Delete),
        "insert" | "ins" => Ok(KeyCode::Insert),
        "home" => Ok(KeyCode::Home),
        "end" => Ok(KeyCode::End),
        "pageup" | "pgup" => Ok(KeyCode::PageUp),
        "pagedown" | "pgdn" | "pgdown" => Ok(KeyCode::PageDown),
        "up" => Ok(KeyCode::Up),
        "down" => Ok(KeyCode::Down),
        "left" => Ok(KeyCode::Left),
        "right" => Ok(KeyCode::Right),

        // Function keys
        "f1" => Ok(KeyCode::F(1)),
        "f2" => Ok(KeyCode::F(2)),
        "f3" => Ok(KeyCode::F(3)),
        "f4" => Ok(KeyCode::F(4)),
        "f5" => Ok(KeyCode::F(5)),
        "f6" => Ok(KeyCode::F(6)),
        "f7" => Ok(KeyCode::F(7)),
        "f8" => Ok(KeyCode::F(8)),
        "f9" => Ok(KeyCode::F(9)),
        "f10" => Ok(KeyCode::F(10)),
        "f11" => Ok(KeyCode::F(11)),
        "f12" => Ok(KeyCode::F(12)),

        // Single character (works for ASCII and multi-byte Unicode)
        _ => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(KeyCode::Char(c)),
                _ => Err(format!("Unknown key: {}", s)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_key() {
        let kb = parse_keybinding("A").unwrap();
        assert_eq!(kb.key, KeyCode::Char('A'));
        assert_eq!(kb.modifiers, KeyModifiers::empty());
    }

    #[test]
    fn test_parse_ctrl_key() {
        let kb = parse_keybinding("Ctrl+S").unwrap();
        assert_eq!(kb.key, KeyCode::Char('S'));
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn test_parse_ctrl_shift_key() {
        let kb = parse_keybinding("Ctrl+Shift+S").unwrap();
        assert_eq!(kb.key, KeyCode::Char('S'));
        assert_eq!(kb.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    #[test]
    fn test_parse_alt_key() {
        let kb = parse_keybinding("Alt+F").unwrap();
        assert_eq!(kb.key, KeyCode::Char('F'));
        assert_eq!(kb.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn test_parse_function_key() {
        let kb = parse_keybinding("F5").unwrap();
        assert_eq!(kb.key, KeyCode::F(5));
        assert_eq!(kb.modifiers, KeyModifiers::empty());
    }

    #[test]
    fn test_parse_shift_function_key() {
        let kb = parse_keybinding("Shift+F3").unwrap();
        assert_eq!(kb.key, KeyCode::F(3));
        assert_eq!(kb.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn test_parse_special_keys() {
        assert_eq!(parse_keybinding("Enter").unwrap().key, KeyCode::Enter);
        assert_eq!(parse_keybinding("Escape").unwrap().key, KeyCode::Esc);
        assert_eq!(parse_keybinding("Tab").unwrap().key, KeyCode::Tab);
        assert_eq!(parse_keybinding("Space").unwrap().key, KeyCode::Char(' '));
        assert_eq!(
            parse_keybinding("Backspace").unwrap().key,
            KeyCode::Backspace
        );
        assert_eq!(parse_keybinding("Delete").unwrap().key, KeyCode::Delete);
        assert_eq!(parse_keybinding("Insert").unwrap().key, KeyCode::Insert);
        assert_eq!(parse_keybinding("Home").unwrap().key, KeyCode::Home);
        assert_eq!(parse_keybinding("End").unwrap().key, KeyCode::End);
        assert_eq!(parse_keybinding("PageUp").unwrap().key, KeyCode::PageUp);
        assert_eq!(parse_keybinding("PageDown").unwrap().key, KeyCode::PageDown);
    }

    #[test]
    fn test_parse_arrow_keys() {
        assert_eq!(parse_keybinding("Up").unwrap().key, KeyCode::Up);
        assert_eq!(parse_keybinding("Down").unwrap().key, KeyCode::Down);
        assert_eq!(parse_keybinding("Left").unwrap().key, KeyCode::Left);
        assert_eq!(parse_keybinding("Right").unwrap().key, KeyCode::Right);
    }

    #[test]
    fn test_parse_case_insensitive() {
        let kb1 = parse_keybinding("ctrl+s").unwrap();
        let kb2 = parse_keybinding("CTRL+S").unwrap();
        let kb3 = parse_keybinding("Ctrl+S").unwrap();
        assert_eq!(kb1.modifiers, kb2.modifiers);
        assert_eq!(kb2.modifiers, kb3.modifiers);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_keybinding("").is_err());
        assert!(parse_keybinding("InvalidKey").is_err());
        assert!(parse_keybinding("Ctrl+").is_err());
    }

    #[test]
    fn test_keybinding_matches() {
        let kb = KeyBinding::Single("Ctrl+S".to_string());
        let event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(kb.matches(&event));

        let wrong_event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::empty());
        assert!(!kb.matches(&wrong_event));
    }

    #[test]
    fn test_keybinding_matches_respects_caps_lock_state() {
        use crossterm::event::{KeyEventKind, KeyEventState};

        let search = KeyBinding::Single("Ctrl+F".to_string());
        let search_content = KeyBinding::Single("Ctrl+Shift+F".to_string());

        let make_event = |code, mods, state| KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state,
        };

        // Without Caps Lock bit: strict match stays strict.
        assert!(search.matches(&make_event(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            KeyEventState::NONE,
        )));
        assert!(!search.matches(&make_event(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventState::NONE,
        )));
        assert!(search_content.matches(&make_event(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventState::NONE,
        )));

        // With Caps Lock reported: matching only honours the bit when
        // the active terminal advertises REPORT_EVENT_TYPES. The
        // convenience `KeyBinding::matches` defaults to a no-caps
        // normalizer, so the Shift attached to the letter is **not**
        // dropped — `Ctrl+F` does not match `Char('F') + Ctrl|Shift`
        // unless the caller canonicalizes with `event_types: true` first.
        let normalizer = KeyNormalizer::new(termide_keyboard::KeyboardCaps {
            event_types: true,
            ..Default::default()
        });
        let canon = |code, mods, state| normalizer.canonicalize(make_event(code, mods, state));
        // After canonicalize: spurious Shift dropped, so only the no-Shift
        // binding (`Ctrl+F`) matches. Caps Lock is inherently ambiguous
        // with intentional Shift; we resolve in favour of the no-Shift
        // binding (the common case for hotkey use during caps-lock-on
        // typing) and accept that the Shift variant cannot fire while
        // caps lock is engaged on the terminal that reported the bit.
        assert!(search.matches_canonical(&canon(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventState::CAPS_LOCK,
        )));
        assert!(!search_content.matches_canonical(&canon(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventState::CAPS_LOCK,
        )));
    }

    #[test]
    fn test_keybinding_multiple_matches() {
        let kb = KeyBinding::Multiple(vec!["C".to_string(), "F5".to_string()]);

        let event_c = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::empty());
        let event_f5 = KeyEvent::new(KeyCode::F(5), KeyModifiers::empty());
        let event_d = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::empty());

        assert!(kb.matches(&event_c));
        assert!(kb.matches(&event_f5));
        assert!(!kb.matches(&event_d));
    }

    #[test]
    fn test_keybinding_matches_shifted_punctuation() {
        use crossterm::event::{KeyEventKind, KeyEventState};

        let make_event = |code, mods| KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };

        // After Phase 3, the canon distinguishes physical-`=` and
        // physical-`+` (Shift+=) presses. The convenience matcher
        // canonicalizes the event before comparing, and parse-time
        // canonicalization rewrites shifted-glyph strings, so these
        // pairs are equivalent:
        //   "Ctrl+Alt+="  ≡  Char('=') + Ctrl|Alt
        //   "Ctrl+Alt++"  ≡  "Ctrl+Alt+Shift+="  ≡  Char('+') + Ctrl|Alt
        //                                          (canonicalized to
        //                                           Char('=') + Ctrl|Alt|Shift)
        let grow_unshifted = KeyBinding::Single("Ctrl+Alt+=".to_string());
        let grow_shifted = KeyBinding::Single("Ctrl+Alt+Shift+=".to_string());
        let grow_literal_plus = KeyBinding::Single("Ctrl+Alt++".to_string());

        // Unshifted binding matches unshifted event only.
        assert!(grow_unshifted.matches(&make_event(
            KeyCode::Char('='),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));
        assert!(!grow_unshifted.matches(&make_event(
            KeyCode::Char('+'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));

        // Shifted binding (`Ctrl+Alt+Shift+=`) matches the physical
        // `Shift+=` press, however the terminal reported the chord:
        // - `Char('+') + Ctrl|Alt` (REPORT_ALTERNATE_KEYS path);
        // - `Char('+') + Ctrl|Alt|Shift` (terminals that don't strip);
        // - `Char('=') + Ctrl|Alt|Shift` (already-canonical form).
        for ev in [
            make_event(
                KeyCode::Char('+'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
            make_event(
                KeyCode::Char('+'),
                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
            ),
            make_event(
                KeyCode::Char('='),
                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
            ),
        ] {
            assert!(
                grow_shifted.matches(&ev),
                "Ctrl+Alt+Shift+= should match {ev:?}"
            );
        }

        // `Ctrl+Alt++` parses to the same canonical form as `Ctrl+Alt+Shift+=`.
        assert!(grow_literal_plus.matches(&make_event(
            KeyCode::Char('+'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));

        // Non-matching modifier set: no false positives.
        assert!(!grow_unshifted.matches(&make_event(KeyCode::Char('+'), KeyModifiers::ALT,)));
        assert!(!grow_unshifted.matches(&make_event(KeyCode::Char('+'), KeyModifiers::CONTROL,)));

        // Bindings that explicitly request Shift are unaffected: the
        // shifted-equivalent path doesn't fire for letters, and it
        // doesn't strip Shift when the binding asks for it.
        let ctrl_shift_f = KeyBinding::Single("Ctrl+Shift+F".to_string());
        assert!(ctrl_shift_f.matches(&make_event(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )));
        assert!(!ctrl_shift_f.matches(&make_event(KeyCode::Char('f'), KeyModifiers::CONTROL,)));
        assert!(!ctrl_shift_f.matches(&make_event(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )));
    }

    #[test]
    fn test_cyrillic_to_latin_for_vim() {
        // Vim navigation chars on ru-layout map back to Latin via the
        // shared keyboard helper.
        assert_eq!(termide_keyboard::cyrillic_to_latin_opt('л'), Some('k'));
        assert_eq!(termide_keyboard::cyrillic_to_latin_opt('о'), Some('j'));
        assert_eq!(termide_keyboard::cyrillic_to_latin_opt('п'), Some('g'));
        assert_eq!(termide_keyboard::cyrillic_to_latin_opt('Н'), Some('Y'));
        // Latin letters are pass-through.
        assert_eq!(termide_keyboard::cyrillic_to_latin_opt('a'), None);
        assert_eq!(termide_keyboard::cyrillic_to_latin_opt('1'), None);
    }
}
