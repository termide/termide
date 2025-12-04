# Text Editor

The text editor panel provides a functional editor for working with text files with syntax highlighting support for various programming languages.

## Key Features

- **Syntax Highlighting**: Automatic highlighting for popular programming languages (Rust, Python, JavaScript, C/C++, Go, etc.)
- **Git Diff Visualization**: Real-time visualization of changes compared to HEAD with color-coded line numbers (green for added, yellow for modified, red for deleted lines), deletion markers showing count of deleted lines
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
| `Ctrl+D`          | Duplicate current line or selection        |
| `Backspace`       | Delete character to the left of cursor     |
| `Delete`          | Delete character to the right of cursor    |
| `Enter`           | Insert new line                            |
| `Tab`             | Insert indent (configurable, default 4)    |

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
- **Replace** (`Ctrl+R`) - Replace current match and move to next
- **All** (`Ctrl+Alt+R`) - Replace all matches, show count, and close modal
- **◄ Prev** - Navigate to previous match
- **Next ►** - Navigate to next match

**Replace All Feedback:**
- After using "Replace All", the status bar shows how many replacements were made
- Example: "Replaced 5 occurrences"

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

**Note:** Mouse selection works correctly in word wrap mode, accounting for wrapped lines.

## Word Wrap

When word wrap is enabled (configurable in settings), long lines are automatically wrapped to fit the panel width. The editor properly handles:

- **Cursor positioning**: Cursor navigation and display work correctly across wrapped lines
- **Mouse selection**: Clicks and drags accurately select text even when lines span multiple visual rows
- **Line numbers**: Displayed for logical lines, not visual rows
- **Editing operations**: All editing commands (cut, copy, paste, undo/redo) work seamlessly with wrapped content

Enable/disable word wrap in your configuration file (`~/.config/termide/config.toml`):
```toml
[editor]
word_wrap = true  # or false
```

## Status Bar Information

When working in the editor, the status bar displays:
- File name and modification indicator (*)
- Current cursor position (line:column)
- Search information (number of matches)
- File type (plain text / read-only)

## Git Diff Visualization

When editing files in a git repository with `show_git_diff` enabled, the editor displays real-time diff information compared to HEAD:

### Line Number Colors

Line numbers are color-coded to show the status compared to HEAD:

- **Green** - Line was added (not in HEAD)
- **Yellow** - Line was modified (changed from HEAD)
- **Red marker (▶)** - Marks a deletion point (lines were deleted after this line)
- **Default color** - Line unchanged from HEAD

### Deletion Markers

When lines are deleted, a virtual line is inserted to visualize the deletion:

- Displays a horizontal line (`━`) spanning the editor width
- Shows deletion marker character (`▶`) in the line number area with red color
- Displays centered text: "N lines deleted" (e.g., "3 lines deleted")
- Styled in gray/disabled color to distinguish from actual content
- Does not affect line numbering (shows `▶` instead of a number)

**Example:**
```
  42 | function calculateTotal() {
 ▶   | ━━━━━━━ 5 lines deleted ━━━━━━━
  43 |     return result;
```

### How It Works

- **Automatic updates**: Diff updates when you save the file
- **Real-time comparison**: Compares current buffer content with HEAD version
- **Undo/Redo support**: Markers appear/disappear as you undo/redo deletions
- **Works with editing**: All normal editing operations work seamlessly with diff visualization

### Configuration

Enable or disable git diff visualization in your configuration file (`~/.config/termide/config.toml`):

```toml
# Show git diff colors on line numbers (default: true)
show_git_diff = true
```

**Notes:**
- Only works when editing files within a git repository
- Requires the file to exist in HEAD (new untracked files show all lines as added)
- Virtual deletion marker lines are visual-only and don't affect the file content
