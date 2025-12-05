# TextInputHandler Migration Guide

## Overview

The `TextInputHandler` utility (`src/ui/modal/text_input.rs`) provides a reusable, well-tested solution for text input with UTF-8 cursor management in modal windows.

## ✅ Already Applied To

1. **InputModal** (-59 LOC)
2. **RenamePatternModal** (-33 LOC)
3. **SearchModal** (-26 LOC)
4. **ReplaceModal** (-56 LOC) - dual input handlers
5. **EditableSelectModal** (-57 LOC) - with saved_input rollback

**Total savings:** -231 LOC duplicate code removed

## 🎉 Migration Complete!

All modals with text input have been successfully migrated to use `TextInputHandler`.

### Summary

- **5 modals refactored**
- **Net reduction: -137 LOC** (154 insertions, 291 deletions)
- **All 71 tests passing** (67 passed, 4 ignored)
- **Zero compiler warnings, zero clippy errors**
- **Consistent UTF-8 handling** across all text input

## Migration Steps

### 1. Update Struct

```rust
// Before:
pub struct MyModal {
    input: String,
    cursor_pos: usize,
    // ...
}

// After:
use super::TextInputHandler;

pub struct MyModal {
    input_handler: TextInputHandler,
    // ...
}
```

### 2. Update Constructor

```rust
// Before:
Self {
    input: default.into(),
    cursor_pos: default.chars().count(),
}

// After:
Self {
    input_handler: TextInputHandler::with_default(default),
}
```

### 3. Update Rendering

```rust
// Before:
let byte_pos = self.input.chars()
    .take(self.cursor_pos)
    .map(|c| c.len_utf8())
    .sum::<usize>();
    
let input_line = Line::from(vec![
    Span::styled(&self.input[..byte_pos], ...),
    Span::styled("█", ...),
    Span::styled(&self.input[byte_pos..], ...),
]);

// After:
let input_line = Line::from(vec![
    Span::styled(self.input_handler.text_before_cursor(), ...),
    Span::styled("█", ...),
    Span::styled(self.input_handler.text_after_cursor(), ...),
]);
```

### 4. Update Key Handling

```rust
// Before:
KeyCode::Char(c) => {
    let byte_pos = self.input.chars()
        .take(self.cursor_pos)
        .map(|c| c.len_utf8())
        .sum::<usize>();
    self.input.insert(byte_pos, c);
    self.cursor_pos += 1;
}
KeyCode::Backspace => {
    if self.cursor_pos > 0 {
        self.cursor_pos -= 1;
        let byte_pos = ...;
        self.input.remove(byte_pos);
    }
}
KeyCode::Left => {
    if self.cursor_pos > 0 {
        self.cursor_pos -= 1;
    }
}
// ... etc

// After:
KeyCode::Char(c) => {
    self.input_handler.insert_char(c);
}
KeyCode::Backspace => {
    self.input_handler.backspace();
}
KeyCode::Left => {
    self.input_handler.move_left();
}
// ... etc
```

### 5. Update Result/Access

```rust
// Before:
if self.input.is_empty() { ... }
Ok(Some(ModalResult::Confirmed(self.input.clone())))

// After:
if self.input_handler.is_empty() { ... }
Ok(Some(ModalResult::Confirmed(self.input_handler.text().to_string())))
```

## Special Cases

### Dropdown/Saved Input (editable_select.rs)

Keep `saved_input: String` separate for rollback:

```rust
// Rollback on Escape
self.input_handler.set_text(self.saved_input.clone());

// Save current input
self.saved_input = self.input_handler.text().to_string();
```

### Multiple Inputs (replace.rs)

Create two instances:

```rust
pub struct ReplaceModal {
    find_input: TextInputHandler,
    replace_input: TextInputHandler,
    active_input: InputField, // enum to track which is active
}
```

## Final Impact

- **Modals migrated:** 5 (InputModal, RenamePatternModal, SearchModal, ReplaceModal, EditableSelectModal)
- **Total savings:** -231 LOC duplicate code removed
- **Net reduction:** -137 LOC (3 files: 154 insertions, 291 deletions)
- **Tests:** All 71 tests passing (67 passed, 4 ignored)
- **Quality:** Consistent UTF-8 handling across all text input modals

## Testing

After migration, verify:

1. `cargo test` - all tests pass
2. Manual testing of:
   - UTF-8 character input (e.g., Cyrillic, emoji)
   - Cursor navigation (Home/End/Left/Right)
   - Backspace/Delete
   - Text selection and paste (if applicable)

## References

- `src/ui/modal/text_input.rs` - TextInputHandler implementation
- `src/ui/modal/input.rs` - Example migration (simple case)
- `src/ui/modal/rename_pattern.rs` - Example with Ctrl+U/A/E support
