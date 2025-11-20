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

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+F`          | Open search dialog                         |
| `F3`              | Go to next match                           |
| `Shift+F3`        | Go to previous match                       |
| `Escape`          | Close search                               |
| `Ctrl+H`          | Open replace dialog                        |
| `Ctrl+R`          | Replace current match                      |
| `Ctrl+Alt+R`      | Replace all matches                        |

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
