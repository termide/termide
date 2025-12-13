//! Keyboard handling and layout translation.
//!
//! This crate provides utilities for keyboard event handling,
//! including translation between keyboard layouts (e.g., Cyrillic → Latin)
//! to ensure hotkeys work correctly regardless of active keyboard layout.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

        // No change for other characters
        _ => ch,
    }
}

/// Translate KeyEvent for hotkeys
///
/// Applies Cyrillic → Latin translation only when modifier
/// (Ctrl or Alt) is pressed, to not affect regular text input.
pub fn translate_hotkey(key: KeyEvent) -> KeyEvent {
    // Apply only if modifier is present (Ctrl or Alt)
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        if let KeyCode::Char(ch) = key.code {
            let translated = cyrillic_to_latin(ch);
            if translated != ch {
                // Create new KeyEvent with translated character
                return KeyEvent::new(KeyCode::Char(translated), key.modifiers);
            }
        }
    }
    key
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
    fn test_translate_hotkey_with_alt() {
        let key = KeyEvent::new(KeyCode::Char('й'), KeyModifiers::ALT);
        let translated = translate_hotkey(key);
        assert_eq!(translated.code, KeyCode::Char('q'));
        assert_eq!(translated.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn test_translate_hotkey_with_ctrl() {
        let key = KeyEvent::new(KeyCode::Char('ы'), KeyModifiers::CONTROL);
        let translated = translate_hotkey(key);
        assert_eq!(translated.code, KeyCode::Char('s'));
        assert_eq!(translated.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn test_no_translate_without_modifier() {
        let key = KeyEvent::new(KeyCode::Char('й'), KeyModifiers::NONE);
        let translated = translate_hotkey(key);
        assert_eq!(translated.code, KeyCode::Char('й')); // Unchanged
    }

    #[test]
    fn test_no_translate_shift_only() {
        let key = KeyEvent::new(KeyCode::Char('Й'), KeyModifiers::SHIFT);
        let translated = translate_hotkey(key);
        assert_eq!(translated.code, KeyCode::Char('Й')); // Unchanged
    }
}
