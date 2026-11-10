//! Entering and leaving the terminal modes termide runs in.
//!
//! These live here rather than in `main` because a detached instance has to
//! re-enter them: a client that attaches later is looking at a terminal that
//! knows nothing of the alternate screen, mouse reporting or bracketed paste
//! the hosted process switched on when it started. Startup and reattach must
//! agree exactly, which is easiest to guarantee when they are the same code.

use crossterm::{
    cursor::{self, MoveToColumn},
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    style::Print,
    terminal::{
        disable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use std::io::{self, IsTerminal};

use termide_keyboard::KeyboardCaps;

/// Switch the terminal into the modes the UI assumes.
///
/// Safe to call more than once: every sequence here is idempotent from the
/// terminal's point of view, except the keyboard-enhancement push, which
/// stacks. That only matters when reattaching to the *same* terminal twice,
/// where one extra stack entry is harmless and is popped on exit anyway.
pub fn enter_terminal_modes(caps: &KeyboardCaps, title: Option<&str>) -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange,
        EnableBracketedPaste
    )?;

    if let Some(title) = title {
        execute!(stdout, SetTitle(title))?;
    }

    if caps.kitty_full {
        // REPORT_EVENT_TYPES exposes `KeyEventState::CAPS_LOCK` on every key
        // event, which the hotkey matcher uses to ignore the spurious Shift
        // modifier that Caps Lock attaches to letters.
        //
        // REPORT_ALTERNATE_KEYS is what makes shifted characters typable under
        // REPORT_ALL_KEYS_AS_ESCAPE_CODES, where the protocol's primary
        // codepoint is the key *without* modifiers.
        let mut flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;

        // REPORT_ALL_KEYS_AS_ESCAPE_CODES is what makes macOS `Option+<letter>`
        // reach us as `Alt+<letter>`. `KeyboardCaps::from_probe` restricts it to
        // macOS and honours `general.report_all_keys`, because it also stops
        // dead-key and IME composition from reaching the app.
        if caps.all_keys {
            flags |= KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        }

        execute!(stdout, PushKeyboardEnhancementFlags(flags))?;
    }

    Ok(())
}

/// `TERMIDE_VS16_WIDE=1|0` pins the answer of [`probe_variation_selector_width`]
/// for terminals that answer it wrongly or not at all.
pub const VS16_WIDTH_ENV: &str = "TERMIDE_VS16_WIDE";

/// The user's override of the variation-selector width, if set.
pub fn variation_selector_width_override() -> Option<bool> {
    match std::env::var(VS16_WIDTH_ENV).ok()?.trim() {
        "1" | "true" | "wide" => Some(true),
        "0" | "false" | "narrow" => Some(false),
        _ => None,
    }
}

/// Ask the host terminal whether U+FE0F widens the emoji before it.
///
/// `⏱️` is two columns in Ghostty, WezTerm and iTerm2 and one in `wcwidth`
/// terminals such as alacritty and foot; ratatui's frame diff and the
/// terminal panel's grid both have to agree with whichever terminal is
/// looking at us, or every row holding such an emoji is shifted by a column
/// and keeps stale cells. The glyph is drawn at the cursor, the cursor
/// position is read back, and the line is cleared from where the probe
/// started. Returns `None` when the terminal did not answer (not a tty, or a
/// relay that swallows queries). [`VS16_WIDTH_ENV`] short-circuits the probe.
pub fn probe_variation_selector_width() -> Option<bool> {
    if let Some(forced) = variation_selector_width_override() {
        return Some(forced);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return None;
    }
    let mut stdout = io::stdout();
    let (start_col, _) = cursor::position().ok()?;
    execute!(stdout, Print("\u{23F1}\u{FE0F}")).ok()?;
    let (end_col, _) = cursor::position().ok()?;
    let _ = execute!(
        stdout,
        MoveToColumn(start_col),
        Clear(ClearType::UntilNewLine)
    );
    Some(end_col.saturating_sub(start_col) >= 2)
}

/// Make every width computation in the process follow the host terminal's
/// answer to [`probe_variation_selector_width`]. An unanswered probe keeps
/// the UAX #11 default (wide), which is what upstream `unicode-width` and
/// most modern terminals do. Returns the value in effect; callers log it
/// once the logger exists (at startup the probe runs before it does).
pub fn adopt_variation_selector_width(widens: Option<bool>) -> bool {
    let widens = variation_selector_width_override()
        .or(widens)
        .unwrap_or(true);
    unicode_width::set_variation_selectors_change_width(widens);
    widens
}

/// Put the terminal back the way it was found.
///
/// Used on exit, from the panic hook, and by the attach client when its
/// connection ends — the client never enabled these itself, but it is the one
/// holding the terminal when an instance lets go of it.
pub fn leave_terminal_modes() {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        SetTitle("")
    );
    let _ = execute!(stdout, crossterm::cursor::Show);
}
