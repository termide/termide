//! Hotkey table: maps action names to key bindings.
//!
//! Each panel and the global handler have their own HotkeyTable loaded from
//! the `[hotkeys.*]` config section. The table provides `matches(action, key)`
//! for checking if a normalized key event matches a configured action.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

use termide_config::{KeyBinding, ParsedKeyBinding};

/// A table mapping action names to their configured key bindings.
#[derive(Debug, Clone, Default)]
pub struct HotkeyTable {
    bindings: HashMap<String, Vec<ParsedKeyBinding>>,
}

impl HotkeyTable {
    /// Create an empty table.
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Insert an action with its key bindings.
    pub fn insert(&mut self, action: impl Into<String>, binding: &Option<KeyBinding>) {
        if let Some(kb) = binding {
            let parsed = match kb {
                KeyBinding::Single(s) => {
                    if let Ok(p) = termide_config::parse_keybinding(s) {
                        vec![p]
                    } else {
                        vec![]
                    }
                }
                KeyBinding::Multiple(v) => v
                    .iter()
                    .filter_map(|s| termide_config::parse_keybinding(s).ok())
                    .collect(),
            };
            if !parsed.is_empty() {
                self.bindings.insert(action.into(), parsed);
            }
        }
    }

    /// Insert an action with a raw key binding string (e.g. "Ctrl+Shift+D").
    pub fn insert_raw(&mut self, action: impl Into<String>, key_str: &str) {
        if let Ok(parsed) = termide_config::parse_keybinding(key_str) {
            self.bindings.insert(action.into(), vec![parsed]);
        }
    }

    /// Check if a key event matches the given action name.
    ///
    /// Convenience wrapper: canonicalizes `key` with a default
    /// (no-caps) `KeyNormalizer` before strict comparison. Callers
    /// holding a `KeyChord` should use `matches_canonical` directly so
    /// the active terminal capabilities are honoured.
    pub fn matches(&self, action: &str, key: &KeyEvent) -> bool {
        let normalizer = termide_keyboard::KeyNormalizer::default();
        let canonical = normalizer.canonicalize(*key);
        self.matches_canonical(action, &canonical)
    }

    /// Strict match against an already-canonical event.
    ///
    /// Single pass — both the binding (canonicalized at parse time)
    /// and the event (canonicalized by the dispatcher) are in canonical
    /// form, so no alternative-paths are needed.
    pub fn matches_canonical(&self, action: &str, canonical: &KeyEvent) -> bool {
        let Some(bindings) = self.bindings.get(action) else {
            return false;
        };
        bindings.iter().any(|b| b.matches(canonical))
    }

    /// Find the first action whose name starts with `prefix` that matches the key.
    pub fn find_match(&self, prefix: &str, key: &KeyEvent) -> Option<String> {
        for action in self.bindings.keys() {
            if action.starts_with(prefix) && self.matches(action, key) {
                return Some(action.clone());
            }
        }
        None
    }

    /// Get display string for an action (for the help window).
    /// Returns empty string if action has no bindings.
    pub fn display(&self, action: &str) -> String {
        if let Some(bindings) = self.bindings.get(action) {
            bindings
                .iter()
                .map(format_binding)
                .collect::<Vec<_>>()
                .join(" / ")
        } else {
            String::new()
        }
    }
}

/// Format a parsed key binding to human-readable string.
fn format_binding(binding: &ParsedKeyBinding) -> String {
    let mut parts = Vec::new();

    if binding.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if binding.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if binding.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }

    let key_name = match binding.key {
        KeyCode::Char(c) => {
            if c == ' ' {
                "Space".to_string()
            } else {
                c.to_uppercase().to_string()
            }
        }
        KeyCode::F(n) => format!("F{}", n),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        _ => format!("{:?}", binding.key),
    };

    parts.push(&key_name);
    // Can't push &key_name after parts borrows — rebuild
    let mut result = String::new();
    if binding.modifiers.contains(KeyModifiers::CONTROL) {
        result.push_str("Ctrl+");
    }
    if binding.modifiers.contains(KeyModifiers::ALT) {
        result.push_str("Alt+");
    }
    if binding.modifiers.contains(KeyModifiers::SHIFT) && !matches!(binding.key, KeyCode::BackTab) {
        result.push_str("Shift+");
    }
    result.push_str(&key_name);
    result
}
