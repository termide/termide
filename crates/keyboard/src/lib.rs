//! Keyboard handling and layout translation.
//!
//! This crate provides utilities for keyboard event handling,
//! including translation between keyboard layouts (e.g., Cyrillic → Latin)
//! to ensure hotkeys work correctly regardless of active keyboard layout.

mod normalize;

pub use normalize::{KeyNormalizer, KeyboardCaps};

/// US-QWERTY shifted-punctuation → unshifted equivalent.
///
/// Returns `Some(unshifted)` when `c` is a shifted glyph that lives on
/// the same physical key as its unshifted counterpart on a US QWERTY
/// layout, otherwise `None`. Used by `ParsedKeyBinding::matches` so a
/// single binding like `Ctrl+Alt+=` fires whether the terminal reports
/// the event as `Char('=')` (unshifted) or as `Char('+')` — both the
/// Kitty protocol's `REPORT_ALTERNATE_KEYS` mode and a legacy terminal
/// encode `Shift+Ctrl+Alt+=` as the shifted glyph with the Shift
/// modifier dropped.
pub fn unshifted_punctuation(c: char) -> Option<char> {
    Some(match c {
        '+' => '=',
        '_' => '-',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '~' => '`',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        _ => return None,
    })
}

/// `Some(latin)` if `ch` is a Cyrillic character that lives on the
/// same physical key as `latin` on QWERTY, otherwise `None`.
///
/// Use this in callers that branch on whether a translation happened
/// (e.g. `is_move_up`, `KeyNormalizer::canonicalize`); use
/// `cyrillic_to_latin` when you want a fall-through translator.
pub fn cyrillic_to_latin_opt(ch: char) -> Option<char> {
    let translated = cyrillic_to_latin(ch);
    (translated != ch).then_some(translated)
}

/// Cyrillic to Latin mapping table (ЙЦУКЕН → QWERTY)
///
/// Converts Cyrillic character to corresponding Latin character
/// on the same physical key.
pub fn cyrillic_to_latin(ch: char) -> char {
    match ch {
        // Top row lowercase: йцукенгшщзхъ → qwertyuiop[]
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'х' => '[',
        'ъ' => ']',

        // Top row uppercase: ЙЦУКЕНГШЩЗХЪ → QWERTYUIOP[]
        'Й' => 'Q',
        'Ц' => 'W',
        'У' => 'E',
        'К' => 'R',
        'Е' => 'T',
        'Н' => 'Y',
        'Г' => 'U',
        'Ш' => 'I',
        'Щ' => 'O',
        'З' => 'P',
        'Х' => '{',
        'Ъ' => '}',

        // Middle row lowercase: фывапролджэ → asdfghjkl;'
        'ф' => 'a',
        'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'ж' => ';',
        'э' => '\'',

        // Middle row uppercase: ФЫВАПРОЛДЖЭ → ASDFGHJKL:"
        'Ф' => 'A',
        'Ы' => 'S',
        'В' => 'D',
        'А' => 'F',
        'П' => 'G',
        'Р' => 'H',
        'О' => 'J',
        'Л' => 'K',
        'Д' => 'L',
        'Ж' => ':',
        'Э' => '"',

        // Bottom row lowercase: ячсмитьбю → zxcvbnm,.
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        'б' => ',',
        'ю' => '.',

        // Bottom row uppercase: ЯЧСМИТЬБЮ → ZXCVBNM<>
        'Я' => 'Z',
        'Ч' => 'X',
        'С' => 'C',
        'М' => 'V',
        'И' => 'B',
        'Т' => 'N',
        'Ь' => 'M',
        'Б' => '<',
        'Ю' => '>',

        // Russian Ё / ё (top-left key on ru-layout) maps to backtick /
        // tilde on en-QWERTY.
        'ё' => '`',
        'Ё' => '~',
        // NOTE: do NOT map en-punctuation `.`→`/` or `,`→`.`. Those
        // are not Cyrillic → Latin translations; they would
        // accidentally collapse en-QWERTY chords (`Alt+.` matching
        // `Alt+/`, etc.).

        // No change for other characters
        _ => ch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cyrillic_to_latin() {
        // Lowercase
        assert_eq!(cyrillic_to_latin('й'), 'q');
        assert_eq!(cyrillic_to_latin('ф'), 'a');
        assert_eq!(cyrillic_to_latin('я'), 'z');
        // Uppercase preserves case
        assert_eq!(cyrillic_to_latin('Й'), 'Q');
        assert_eq!(cyrillic_to_latin('Ф'), 'A');
        assert_eq!(cyrillic_to_latin('Я'), 'Z');
        // Non-Cyrillic unchanged
        assert_eq!(cyrillic_to_latin('q'), 'q'); // Latin unchanged
        assert_eq!(cyrillic_to_latin('1'), '1'); // Numbers unchanged
    }

    #[test]
    fn test_cyrillic_to_latin_opt() {
        assert_eq!(cyrillic_to_latin_opt('й'), Some('q'));
        assert_eq!(cyrillic_to_latin_opt('Я'), Some('Z'));
        assert_eq!(cyrillic_to_latin_opt('a'), None);
        assert_eq!(cyrillic_to_latin_opt('1'), None);
    }

    // Tests for old `translate_hotkey` / `translate_all_chars` /
    // `normalize_for_matching` were removed along with those functions.
    // Equivalent coverage now lives in `normalize::tests` for
    // `KeyNormalizer::canonicalize`.
}
