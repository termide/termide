//! Terminal-capability-aware key event canonicalization.
//!
//! `KeyNormalizer::canonicalize` is the **only** place where a `KeyEvent`
//! is rewritten for hotkey-matching purposes. The pipeline (in
//! `crates/app`) constructs a `KeyChord { raw, canonical }` once per
//! event and propagates both forms downstream — panels and modals
//! pick whichever they need (`canonical` for hotkey matching, `raw`
//! for text input and PTY passthrough).
//!
//! Quirks handled here, in order:
//!
//! 1. Cyrillic → Latin: a binding `Alt+M` fires whether the active
//!    layout reports `Alt+M` or `Alt+Ь` (ru-layout `M`).
//! 2. REPORT_ALTERNATE_KEYS undo: when crossterm receives a Kitty
//!    chord like `Shift+Ctrl+=`, it rewrites the event to
//!    `Char('+') + Ctrl` (Shift stripped, codepoint swapped). We
//!    invert that — `'+'` → `'='` + Shift — so the matcher compares
//!    against the logical chord the user pressed.
//! 3. Caps Lock: when `REPORT_EVENT_TYPES` flagged the event with
//!    `KeyEventState::CAPS_LOCK`, the spurious Shift attached to
//!    letters is dropped.
//! 4. VTE quirk: GNOME Terminal sends `Ctrl+/` as `Ctrl+7`. We
//!    rewrite when Kitty proto is **not** active (on Kitty-capable
//!    terminals the event arrives correctly and the rewrite would
//!    be wrong).

use crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};

use crate::{cyrillic_to_latin, unshifted_punctuation};

/// Snapshot of the terminal's keyboard-protocol capabilities.
///
/// Constructed once at startup (see `KeyboardCaps::detect`) and threaded
/// through `KeyNormalizer`. The fields mirror the Kitty keyboard
/// enhancement flags that termide pushes in `src/main.rs`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyboardCaps {
    /// All four (or all-three-we-use) Kitty enhancement flags acknowledged.
    /// Implies `alt_keys`, `event_types`, and `disambiguate` are all true.
    pub kitty_full: bool,
    /// `REPORT_ALTERNATE_KEYS` active — terminal sends both base and
    /// shifted codepoints for chords like `Shift+=` and crossterm
    /// rewrites them to the shifted glyph.
    pub alt_keys: bool,
    /// `REPORT_EVENT_TYPES` active — events carry `KeyEventState::CAPS_LOCK`
    /// when the lock is engaged.
    pub event_types: bool,
    /// `DISAMBIGUATE_ESCAPE_CODES` active — Esc and modified keys come
    /// in unambiguous CSI-u form.
    pub disambiguate: bool,
    /// `REPORT_ALL_KEYS_AS_ESCAPE_CODES` active — even text-producing
    /// keys arrive as CSI-u, so macOS `Option+<letter>` reports
    /// `Char('f') + ALT` instead of the composed glyph `ƒ`. Implies the
    /// terminal also emits standalone modifier-key events, which
    /// `crates/core/src/event.rs` drops.
    pub all_keys: bool,
    /// `true` when termide ran inside an SSH session and skipped the
    /// detection probe (probe blocks indefinitely on some bridges).
    pub via_ssh: bool,
}

impl KeyboardCaps {
    /// Probe terminal capabilities. Returns `Default` (all false) when:
    /// - running inside SSH (probe is skipped to avoid hangs);
    /// - probe failed or returned `false`.
    ///
    /// `request_all_keys` carries `general.report_all_keys` from the
    /// config. It is honoured on macOS only: elsewhere `Option`/`Alt`
    /// does not compose text, so `Alt+<letter>` already arrives with the
    /// ALT bit and the flag would only cost dead-key/compose input.
    ///
    /// Call this **before** `enable_raw_mode` / `PushKeyboardEnhancementFlags` —
    /// the probe is a query/response handshake that needs cooked-mode
    /// readiness.
    pub fn detect(request_all_keys: bool) -> Self {
        let via_ssh =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        if via_ssh {
            return Self {
                via_ssh: true,
                ..Self::default()
            };
        }
        let supported = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        Self {
            kitty_full: supported,
            alt_keys: supported,
            event_types: supported,
            disambiguate: supported,
            all_keys: supported && request_all_keys && cfg!(target_os = "macos"),
            via_ssh: false,
        }
    }
}

/// Canonicalizes raw `KeyEvent`s into the form used for hotkey matching.
///
/// The transform is pure — given the same `caps`, `canonicalize` is
/// idempotent (`canonicalize(canonicalize(e)) == canonicalize(e)`),
/// which lets us canonicalize binding strings at parse time and
/// events at match time and compare with strict equality.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyNormalizer {
    pub caps: KeyboardCaps,
}

impl KeyNormalizer {
    pub fn new(caps: KeyboardCaps) -> Self {
        Self { caps }
    }

    /// Returns the canonical form of `key` for matching purposes.
    ///
    /// The original event is **not** mutated — text input (`InsertChar`),
    /// PTY passthrough, and search-buffer typing should keep using the
    /// original raw event. Only matchers (`HotkeyTable::matches`,
    /// `ParsedKeyBinding::matches`, vim command interpretation, settings
    /// keybinding capture) consume the canonical form.
    pub fn canonicalize(&self, key: KeyEvent) -> KeyEvent {
        let mut k = key;

        // (a) Cyrillic → Latin.
        if let KeyCode::Char(c) = k.code {
            let latin = cyrillic_to_latin(c);
            if latin != c {
                k.code = KeyCode::Char(latin);
            }
        }

        // (b) REPORT_ALTERNATE_KEYS undo: '+' → '=' + Shift, '_' → '-' + Shift,
        //     etc. Crossterm under this flag strips Shift and emits the
        //     shifted glyph; we put the canonical chord back together.
        //
        //     We do this even when caps.alt_keys is false, because the
        //     transform is harmless for a user who literally typed the
        //     shifted glyph (their keypress chord is, by construction,
        //     `Shift+<unshifted>`, which is what we produce).
        if let KeyCode::Char(c) = k.code {
            if let Some(unshifted) = unshifted_punctuation(c) {
                k.code = KeyCode::Char(unshifted);
                k.modifiers |= KeyModifiers::SHIFT;
            }
        }

        // (c) Caps Lock: drop spurious Shift on letters when the terminal
        //     told us caps lock is active.
        if self.caps.event_types && k.state.contains(KeyEventState::CAPS_LOCK) {
            if let KeyCode::Char(c) = k.code {
                if c.is_alphabetic() {
                    k.modifiers.remove(KeyModifiers::SHIFT);
                }
            }
        }

        // (d) VTE quirk: only when Kitty proto is NOT acknowledged.
        //     On kitty/Ghostty/WezTerm this rewrite is wrong — the event
        //     would already arrive as `Ctrl+/`.
        //
        //     Strict modifier equality (`== CONTROL`, not
        //     `.contains(CONTROL)`): otherwise `Ctrl+Alt+-` — which VTE
        //     encodes as `\e\x1F` — gets parsed by crossterm as
        //     `Ctrl+Alt+7` and would falsely collapse to `Ctrl+Alt+/`,
        //     spuriously triggering whatever action is bound to
        //     `Ctrl+/` (e.g. `switch_directory`).
        if !self.caps.kitty_full
            && k.code == KeyCode::Char('7')
            && k.modifiers == KeyModifiers::CONTROL
        {
            k.code = KeyCode::Char('/');
        }

        // (e) VTE quirk: `Ctrl+\` is sent as ASCII `\x1C` (FS — File
        //     Separator), which crossterm decodes as `Ctrl+4` (the
        //     `0x1C..=0x1F → '4'..='7'` mapping). Symmetric to (d).
        //     Strict modifier equality so `Ctrl+Alt+\` (which VTE
        //     encodes as `\e\x1C` → `Ctrl+Alt+4`) is left alone.
        if !self.caps.kitty_full
            && k.code == KeyCode::Char('4')
            && k.modifiers == KeyModifiers::CONTROL
        {
            k.code = KeyCode::Char('\\');
        }

        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn make(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_with_state(code: KeyCode, mods: KeyModifiers, state: KeyEventState) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state,
        }
    }

    #[test]
    fn canonicalize_cyrillic_to_latin() {
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(KeyCode::Char('й'), KeyModifiers::ALT));
        assert_eq!(canon.code, KeyCode::Char('q'));
        assert_eq!(canon.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn canonicalize_shifted_punctuation() {
        // Shifted glyph + no Shift modifier (REPORT_ALTERNATE_KEYS path)
        // → unshifted + Shift added.
        let n = KeyNormalizer::new(KeyboardCaps {
            alt_keys: true,
            ..Default::default()
        });
        let canon = n.canonicalize(make(KeyCode::Char('+'), KeyModifiers::CONTROL));
        assert_eq!(canon.code, KeyCode::Char('='));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    #[test]
    fn canonicalize_shifted_punctuation_with_shift_already_set() {
        // Idempotent: '+' with Shift already in modifiers stays equivalent.
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(
            KeyCode::Char('+'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(canon.code, KeyCode::Char('='));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    #[test]
    fn canonicalize_caps_lock_strips_shift_on_letter() {
        let n = KeyNormalizer::new(KeyboardCaps {
            event_types: true,
            ..Default::default()
        });
        let canon = n.canonicalize(make_with_state(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventState::CAPS_LOCK,
        ));
        // Letter case unchanged but spurious Shift dropped.
        assert_eq!(canon.code, KeyCode::Char('F'));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn canonicalize_caps_lock_keeps_shift_when_event_types_off() {
        // Without REPORT_EVENT_TYPES we cannot trust the CAPS_LOCK
        // bit, so we must not strip Shift.
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make_with_state(
            KeyCode::Char('F'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventState::CAPS_LOCK,
        ));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    #[test]
    fn canonicalize_vte_ctrl_7_when_legacy() {
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(KeyCode::Char('7'), KeyModifiers::CONTROL));
        assert_eq!(canon.code, KeyCode::Char('/'));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn canonicalize_vte_ctrl_7_unchanged_under_kitty() {
        // On kitty-capable terminals, Ctrl+7 means literally Ctrl+7.
        let n = KeyNormalizer::new(KeyboardCaps {
            kitty_full: true,
            ..Default::default()
        });
        let canon = n.canonicalize(make(KeyCode::Char('7'), KeyModifiers::CONTROL));
        assert_eq!(canon.code, KeyCode::Char('7'));
    }

    #[test]
    fn canonicalize_vte_ctrl_7_skipped_with_alt() {
        // Regression: `Ctrl+Alt+-` is encoded by VTE as `\e\x1F`, which
        // crossterm parses as `Ctrl+Alt+7`. Without strict modifier
        // equality this would collapse to `Ctrl+Alt+/`, which would
        // then falsely trigger any `Ctrl+/`-bound action via the panel
        // matcher. Make sure we don't apply the rewrite when Alt is
        // present.
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(
            KeyCode::Char('7'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(canon.code, KeyCode::Char('7'));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL | KeyModifiers::ALT);
    }

    #[test]
    fn canonicalize_vte_ctrl_7_skipped_with_shift() {
        // Same idea: any non-Control modifier disables the VTE rewrite.
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(
            KeyCode::Char('7'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(canon.code, KeyCode::Char('7'));
    }

    #[test]
    fn canonicalize_vte_ctrl_4_to_backslash_when_legacy() {
        // VTE sends Ctrl+\ as `\x1C`, crossterm parses as Ctrl+4. The
        // quirk rewrites it to Ctrl+\ on legacy terminals.
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(KeyCode::Char('4'), KeyModifiers::CONTROL));
        assert_eq!(canon.code, KeyCode::Char('\\'));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn canonicalize_vte_ctrl_4_unchanged_under_kitty() {
        // On kitty-capable terminals, Ctrl+4 means literally Ctrl+4.
        let n = KeyNormalizer::new(KeyboardCaps {
            kitty_full: true,
            ..Default::default()
        });
        let canon = n.canonicalize(make(KeyCode::Char('4'), KeyModifiers::CONTROL));
        assert_eq!(canon.code, KeyCode::Char('4'));
    }

    #[test]
    fn canonicalize_vte_ctrl_4_skipped_with_alt() {
        // `Ctrl+Alt+\` arrives as `\e\x1C` → crossterm `Ctrl+Alt+4`.
        // Don't collapse — that would falsely trigger any `Ctrl+\`
        // binding from `Ctrl+Alt+\` presses.
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let canon = n.canonicalize(make(
            KeyCode::Char('4'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(canon.code, KeyCode::Char('4'));
        assert_eq!(canon.modifiers, KeyModifiers::CONTROL | KeyModifiers::ALT);
    }

    #[test]
    fn canonicalize_idempotent_for_letter() {
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let ev = make(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(n.canonicalize(ev), ev);
        assert_eq!(n.canonicalize(n.canonicalize(ev)), n.canonicalize(ev));
    }

    #[test]
    fn canonicalize_idempotent_for_shifted_glyph() {
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let ev = make(KeyCode::Char('+'), KeyModifiers::CONTROL);
        let once = n.canonicalize(ev);
        let twice = n.canonicalize(once);
        assert_eq!(once, twice);
    }

    #[test]
    fn canonicalize_arrow_key_pass_through() {
        let n = KeyNormalizer::new(KeyboardCaps::default());
        let ev = make(KeyCode::Up, KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(n.canonicalize(ev), ev);
    }
}
