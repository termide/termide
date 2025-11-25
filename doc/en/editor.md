# Text Editor

The text editor panel provides a functional editor for working with text files with syntax highlighting support for various programming languages.

## Key Features

- **Syntax Highlighting**: Automatic highlighting for popular programming languages (Rust, Python, JavaScript, C/C++, Go, etc.)
- **Search and Replace**: Text search with case-sensitivity support and replacement of found matches
- **Edit History**: Undo and Redo actions
- **Clipboard**: Copy, cut, and paste via system clipboard
- **Auto-save**: Prompt to save when closing a file with unsaved changes

## Navigation

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `↑` / `↓`         | Move cursor up/down                        |
| `←` / `→`         | Move cursor left/right                     |
| `Home`            | Go to beginning of line                    |
| `End`             | Go to end of line                          |
| `PageUp` / `PageDown` | Scroll by one page                      |
| `Ctrl+Home`       | Go to beginning of document                |
| `Ctrl+End`        | Go to end of document                      |

## Editing

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+S`          | Save file                                  |
| `Ctrl+Z`          | Undo last action                           |
| `Ctrl+Y`          | Redo undone action                         |
| `Backspace`       | Delete character to the left of cursor     |
| `Delete`          | Delete character to the right of cursor    |
| `Enter`           | Insert new line                            |
| `Tab`             | Insert indent (4 spaces)                   |

## Search and Replace

### Interactive Search Modal (Ctrl+F)

Press `Ctrl+F` to open an interactive search modal with live preview:

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+F`          | Open search modal                          |
| Type text         | Live search updates as you type            |
| `Tab`             | Go to next match                           |
| `Shift+Tab`       | Go to previous match                       |
| `F3`              | Go to next match                           |
| `Shift+F3`        | Go to previous match                       |
| `Enter`           | Close modal, keep current match selected   |
| `Escape`          | Close search modal                         |
| Mouse click       | Click navigation buttons or `[X]` to close |

**Features:**
- Live search preview as you type
- Match counter display (e.g., "3 of 12")
- Navigation buttons: ◄ Prev, Next ►
- `[X]` close button in modal title
- Search query is preserved when modal is closed

**Search behavior outside modal:**
- `F3` / `Shift+F3` - Navigate through matches with modal closed
- `Tab` / `Shift+Tab` - Navigate matches when search is active
- Any navigation/editing key - Deactivates search mode
- Reopening with `F3` restores the last search query

### Interactive Replace Modal (Ctrl+H)

Press `Ctrl+H` to open an interactive replace modal with two input fields:

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+H`          | Open replace modal                         |
| Type in Find      | Live search updates as you type            |
| `Tab`             | Next match (in Find) or move to Replace field |
| `Shift+Tab`       | Previous match (in Find) or move to Find field |
| `Up` / `Down`     | Navigate between Find and Replace fields   |
| `F3`              | Go to next match                           |
| `Shift+F3`        | Go to previous match                       |
| `Enter`           | Replace current match and move to next     |
| `Escape`          | Close replace modal                        |
| Mouse click       | Click buttons (Replace, All, Prev, Next) or `[X]` |

**Features:**
- Two input fields: Find and Replace
- Live search preview as you type in Find field
- Match counter display (e.g., "3 of 12")
- Four buttons: Replace, All, ◄ Prev, Next ►
- `[X]` close button in modal title
- Both find and replace text are preserved when modal is closed

**Replace button actions:**
- **Replace** - Replace current match and move to next
- **All** - Replace all matches and close modal
- **◄ Prev** - Navigate to previous match
- **Next ►** - Navigate to next match

## Clipboard

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+C`          | Copy selected text                         |
| `Ctrl+X`          | Cut selected text                          |
| `Ctrl+V`          | Paste from system clipboard                |

## Mouse Support

- **Single click**: Set cursor to click position
- **Double click**: Select word under cursor
- **Triple click**: Select entire line
- **Hold + move**: Text selection
- **Scroll wheel**: Scroll editor content

## Status Bar Information

When working in the editor, the status bar displays:
- File name and modification indicator (*)
- Current cursor position (line:column)
- Search information (number of matches)
- File type (plain text / read-only)
