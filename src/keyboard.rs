//! Module for keyboard handling and layout translation
//!
//! Enables hotkeys to work with Cyrillic keyboard layout enabled.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Cyrillic to Latin mapping table (ЙЦУКЕН → QWERTY)
///
/// Converts Cyrillic character to corresponding Latin character
/// on the same physical key.
pub fn cyrillic_to_latin(ch: char) -> char {
    match ch {
        // Top row: ЙЦУКЕНГШЩЗХЪ → QWERTYUIOP[]
        'й' | 'Й' => 'q',
        'ц' | 'Ц' => 'w',
        'у' | 'У' => 'e',
        'к' | 'К' => 'r',
        'е' | 'Е' => 't',
        'н' | 'Н' => 'y',
        'г' | 'Г' => 'u',
        'ш' | 'Ш' => 'i',
        'щ' | 'Щ' => 'o',
        'з' | 'З' => 'p',
        'х' | 'Х' => '[',
        'ъ' | 'Ъ' => ']',

        // Middle row: ФЫВАПРОЛДЖЭ → ASDFGHJKL;'
        'ф' | 'Ф' => 'a',
        'ы' | 'Ы' => 's',
        'в' | 'В' => 'd',
        'а' | 'А' => 'f',
        'п' | 'П' => 'g',
        'р' | 'Р' => 'h',
        'о' | 'О' => 'j',
        'л' | 'Л' => 'k',
        'д' | 'Д' => 'l',
        'ж' | 'Ж' => ';',
        'э' | 'Э' => '\'',

        // Bottom row: ЯЧСМИТЬБЮ → ZXCVBNM,.
        'я' | 'Я' => 'z',
        'ч' | 'Ч' => 'x',
        'с' | 'С' => 'c',
        'м' | 'М' => 'v',
        'и' | 'И' => 'b',
        'т' | 'Т' => 'n',
        'ь' | 'Ь' => 'm',
        'б' | 'Б' => ',',
        'ю' | 'Ю' => '.',

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
    if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
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
        assert_eq!(cyrillic_to_latin('й'), 'q');
        assert_eq!(cyrillic_to_latin('Й'), 'q');
        assert_eq!(cyrillic_to_latin('ф'), 'a');
        assert_eq!(cyrillic_to_latin('я'), 'z');
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
