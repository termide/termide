# Text Editor

The text editor panel provides a functional editor for working with text files with syntax highlighting support for various programming languages.

## Key Features

- **Syntax Highlighting**: Automatic highlighting for popular programming languages (Rust, Python, JavaScript, C/C++, Go, etc.)
- **Git Diff Visualization**: Real-time visualization of changes compared to HEAD with background-colored line numbers (green for added, yellow for modified) and auto-contrast foreground, deletion markers showing count of deleted lines
- **Search and Replace**: Text search with case-sensitivity support and replacement of found matches
- **Edit History**: Undo and Redo actions
- **Clipboard**: Copy, cut, and paste via system clipboard
- **Auto-save**: Prompt to save when closing a file with unsaved changes
- **Word Navigation**: Move cursor by words with Ctrl+Left/Right, select by words with Ctrl+Shift+Left/Right
- **Auto-Indentation**: New lines automatically inherit the indentation of the current line; smart indent adds an extra level after `{`, `(`, `[`, `:`
- **Auto-Close Brackets**: Automatically insert matching closing brackets and quotes (`()`, `[]`, `{}`, `""`, `''`); typing a closing bracket skips over existing one; backspace between a pair deletes both

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
| `Ctrl+Left`       | Move cursor to previous word               |
| `Ctrl+Right`      | Move cursor to next word                   |
| `Ctrl+Shift+Left` | Select to previous word                    |
| `Ctrl+Shift+Right`| Select to next word                        |
| `Ctrl+Up`         | Jump to previous paragraph/symbol boundary |
| `Ctrl+Down`       | Jump to next paragraph/symbol boundary     |
| `Ctrl+Shift+Up`   | Select to previous paragraph/symbol boundary |
| `Ctrl+Shift+Down` | Select to next paragraph/symbol boundary   |

## Editing

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+S`          | Save file                                  |
| `Ctrl+Shift+S`    | Save As (with executable checkbox)         |
| `Ctrl+Z`          | Undo last action                           |
| `Ctrl+Y` / `Ctrl+Shift+Z` | Redo undone action               |
| `Ctrl+D`          | Duplicate current line or selection        |
| `Backspace`       | Delete character to the left of cursor     |
| `Delete`          | Delete character to the right of cursor    |
| `Ctrl+Backspace`  | Delete word to the left (Alt+Backspace also)|
| `Ctrl+Delete`     | Delete word to the right                   |
| `Enter`           | Insert new line (with auto-indentation)    |
| `Tab`             | Insert indent (configurable, default 4)    |
| `Ctrl+/`          | Toggle comment (line/block)                |

## Search and Replace

### Inline Search Bar (Ctrl+F)

Press `Ctrl+F` to open a find bar **docked at the top of the editor** (like the
file manager), with a separator line below it; the buffer stays visible and
matches highlight as you type. `Tab` switches focus between the bar and the
buffer zone: in the buffer zone the cursor moves and scrolls normally while the
bar stays open; `Tab` returns to the bar.

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+F`          | Open the find bar                          |
| Type text         | Live search updates as you type            |
| `F3` / `Enter`    | Go to next match                           |
| `Shift+F3`        | Go to previous match                       |
| `Tab`             | Switch between the bar and the buffer zone |
| Arrows            | Move between fields/buttons (bar zone)     |
| `Escape`          | Close the bar                              |
| Mouse click       | Click the buttons / toggles                |

**Features:**
- Live search preview as you type; match counter (e.g., "3 of 12")
- Navigation buttons: ◄ Prev, Next ►
- `[.*] Regex` and `[Aa] Case` toggles — click them, or focus the buttons row
  and press `Enter` / `Space`. Regex is **off by default** (literal search).
- `Ctrl+R` re-runs the search against the current buffer (refresh).
- The query is preserved when the bar is closed

**Search behavior with the bar closed:**
- `F3` / `Shift+F3` step through matches in the buffer
- Any navigation/editing key deactivates search mode
- Reopening with `F3` restores the last query

### Inline Replace Bar (Ctrl+H)

Press `Ctrl+H` to open the find bar with a Replace field added.

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+H`          | Open the find/replace bar                  |
| Type in Find      | Live search updates as you type            |
| `Tab` / arrows    | Move between Find, Replace and the buttons |
| `F3`              | Go to next match                           |
| `Shift+F3`        | Go to previous match                       |
| `Enter` (in Find) | Go to next match                           |
| `Enter` (in Repl) | Replace the current match                  |
| `Escape`          | Close the bar                              |
| Mouse click       | Click buttons (◄ Prev, Next ►, Replace, Replace all) or toggles |

**Features:**
- Two input fields: Find and Replace; live preview + match counter
- Button row, in order: `[Aa]` / `[.*]` toggles, **◄ Prev**, **Next ►**,
  **Replace**, **Replace all** (focus the row with `Tab` / arrows, then
  `Enter` / `Space`, or click):
  - **◄ Prev** / **Next ►** — navigate matches
  - **Replace** — replace the current match and move to the next
  - **Replace all** — replace every match (the status bar reports the count)
- `[.*] Regex` and `[Aa] Case` toggles. With regex on, the Replace field
  supports `$1` / `${name}` capture groups; off (the default) it is literal.
- Both find and replace text are preserved when the bar is closed.

The configurable `replace_current` and `replace_all` editor keybindings still
act on the active search directly, with or without the bar (see
[Keybindings](keybindings.md)).

## Status bar

The status bar uses a uniform `Label: value` layout. The mode toggles come
first with a fixed width so flipping them never shifts the fields to their
right, shared info follows, and the variable-width position sits last:

- **`View: Text`** — switches to the hex viewer (same as `Ctrl+L`). The
  edit/view mode carries over: leaving an editable buffer opens an editable
  hex editor, and vice versa.
- **`Edit: Yes`/`Edit: No`** — toggles read-only, also bound to `Ctrl+E`
  (`[viewer.keybindings] toggle_view`).
- **`Highlight: <lang>`** — opens the language picker (below).
- **`Tab: <n>`** — sets the tab size.
- **`EOL`** / **`Encoding`** — informational.
- **`Pos: <line>:<col>`** — opens go-to-line; accepts a line (`12`) or a
  line and column (`12:4`).

## Overriding the Detected Language

The language is auto-detected from the file extension. If it's wrong (or absent
for an unknown extension), click the **file-type indicator** in the status bar to
open a dropdown of all available highlighters — built-in languages plus your
configured custom languages, with an **Auto-detect** entry to go back to
extension-based detection. Navigate with the arrows or the mouse (the list
scrolls), `Enter` / click to apply, `Esc` to dismiss.

## Custom Syntax Highlighting

Built-in languages are highlighted with tree-sitter. For a file type that has
no tree-sitter grammar yet — for example a language you are still designing —
you can define a **lightweight keyword highlighter** in `config.toml` without
rebuilding TermIDE. It colours line/block comments, strings, numbers, and your
keyword/type word lists, reusing the active theme's palette.

```toml
[[highlight.custom_languages]]
name = "Alatyr"
extensions = ["al"]          # file extensions, without the leading dot
line_comment = "##"          # rest of the line after this is a comment
# block_comment = ["/*", "*/"]  # optional; matched within a single line
keywords = ["pub", "struct", "enum", "match", "if", "else", "and", "or", "not"]
types = ["u8", "i64", "usize", "ptr"]
```

Add more `[[highlight.custom_languages]]` blocks for other file types. A
tree-sitter grammar (when one exists) always takes precedence over a custom
entry for the same extension. This is a per-line tokenizer, so it does not
track multi-line constructs (a block comment must open and close on one line);
when the language stabilises, a real tree-sitter grammar gives richer,
scope-aware highlighting.

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
- **Shift+click** / **Alt+click**: Extend the current selection from its anchor (or the cursor, if no selection yet) to the click position. Alt+click exists because GNOME Terminal / VTE swallows Shift+click for its own native text selection; on terminals like kitty / alacritty / WezTerm both gestures work.
- **Scroll wheel**: Scroll editor content
- **Ctrl+Click**: Go to definition (LSP); or show color preview if cursor is on a hex color (e.g. `#ff0000`, `#abc`)

**Color preview:** When you hold Ctrl and click on a hex color value, a small popup appears showing a color swatch and the hex code. The popup stays visible while the mouse button is held and disappears on release.

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

## Auto-Indentation

When auto-indentation is enabled (default), pressing `Enter` creates a new line that inherits the indentation of the current line. Additionally, smart indent adds an extra level of indentation after lines ending with `{`, `(`, `[`, or `:`.

**Example:**
```
if condition {|        ← cursor here, press Enter
    |                  ← new line with extra indent
```

**Split-bracket indent:** When pressing `Enter` between a matching pair of brackets (`{|}`, `(|)`, `[|]`), the editor creates three lines — the opening bracket, a new indented line for the cursor, and the closing bracket on its own line:

```
fn main() {|}          ← cursor between braces, press Enter
fn main() {
    |                  ← cursor here, indented
}
```

Enable/disable auto-indentation in your configuration file:
```toml
[editor]
auto_indent = true  # or false (default: true)
```

## Auto-Close Brackets

When auto-close brackets is enabled (default), typing an opening bracket or quote automatically inserts the matching closing character and places the cursor between them.

**Supported pairs:** `()`, `[]`, `{}`, `""`, `''`

**Behavior:**
- Typing `(` inserts `()` with cursor between them
- Typing `)` when the character at cursor is already `)` skips over it instead of inserting a duplicate
- Pressing `Backspace` between an empty pair (e.g., `()`) deletes both characters
- Quotes are not auto-closed after alphanumeric characters (to support apostrophes in words like "it's", "don't")

Enable/disable auto-close brackets in your configuration file:
```toml
[editor]
auto_close_brackets = true  # or false (default: true)
```

## Status Bar Information

When working in the editor, the status bar displays:
- File name and modification indicator (*)
- Current cursor position (line:column)
- Tab size (`Tab: N`) — **clickable**: opens a small input modal that overrides `tab_size` for this editor panel only. The override wins over the global `[editor].tab_size` and survives the per-frame config resync, but doesn't touch `config.toml`. Accepts 1..=16; empty/invalid input is ignored.
- Line ending format (LF / CRLF) and encoding (UTF-8)
- Search information (number of matches)
- File type (plain text / read-only)

## Git Diff Visualization

When editing files in a git repository with `show_git_diff` enabled, the editor displays real-time diff information compared to HEAD:

### Line Number Colors

Line numbers use background highlighting to show the status compared to HEAD. The text color is automatically selected for optimal contrast:

- **Green background** - Line was added (not in HEAD)
- **Yellow background** - Line was modified (changed from HEAD)
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

## Git Blame

When editing files in a git repository, the editor displays an inline blame annotation at the end of the cursor line, showing the author, age, commit hash, and summary of the last change.

### Inline Annotation

The annotation appears at the end of the current line in a dimmed color:

```
  some_function();                          nvn, 2 weeks ago • abc1234 Fix bug
```

### Controls

**Note:** Blame is **enabled by default** when opening a file in a git repository. The annotation loads asynchronously in the background and appears once the `git blame` process completes. Toggle via the Settings modal (`Alt+P` → Editor → Show Blame).

### Configuration

```toml
[editor]
show_blame = true
```

---

## LSP (Language Server Protocol)

TermIDE includes built-in LSP support for intelligent code assistance. When a language server is configured and available, you get:

- **Code Completion** - Context-aware suggestions as you type
- **Diagnostics** - Real-time error and warning indicators
- **Loading Status** - Spinner in panel title shows server status (starting/indexing)

### Triggering Completion

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+J` / `Ctrl+Space` | Manually trigger completion popup    |
| `Enter`           | Accept selected completion                 |
| `Escape`          | Close completion popup                     |
| `↑` / `↓`         | Navigate through suggestions               |
| Type characters   | Filter suggestions by typing               |

**Auto-completion:** When enabled (default), completion popup appears automatically:
- After typing identifier characters (letters, numbers, `_`)
- Immediately after trigger characters (`.`, `:`, `(`, `<`)

### Completion Popup

The completion popup displays:
- **Icon** - Indicates item type (function ƒ, variable v, class C, etc.)
- **Label** - The completion text
- **Scroll indicators** - ▲/▼ when list is scrollable

### Configuration

Configure LSP in your configuration file (`~/.config/termide/config.toml`):

```toml
[lsp]
enabled = true              # Enable/disable LSP (default: true)
auto_completion = true      # Auto-trigger completion on typing (default: true)
completion_delay_ms = 100   # Delay before auto-completion (default: 100)

[lsp.servers.rust]
command = "rust-analyzer"
args = []
root_markers = ["Cargo.toml"]

[lsp.servers.python]
command = "pylsp"
args = []
root_markers = ["pyproject.toml", "setup.py", "requirements.txt"]

[lsp.servers.typescript]
command = "typescript-language-server"
args = ["--stdio"]
root_markers = ["package.json", "tsconfig.json"]
```

### Find References

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Shift+F12`       | Find all references to symbol under cursor |

Opens a dedicated References panel listing all locations where the symbol is used. Click any reference to navigate to it.

### Rename Symbol

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `F4`              | Rename symbol under cursor                 |

Opens an input dialog to enter the new name. All occurrences across the project are updated via LSP WorkspaceEdit.

### Code Actions

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Alt+Enter`       | Show code actions / quick-fixes at cursor  |

Requests quick-fixes for the current line (for example "Import class" to add a `use` statement in PHP) and shows them in a popup. Use `↑`/`↓` to select, `Enter` to apply, `Esc` to dismiss. The chosen action's edit is applied across files, reloading any open editors. (`Ctrl+.` is bound to toggle-comment, so the default is `Alt+Enter`.)

### Server Status Indicator

When opening a file with LSP support, the panel title shows a loading spinner:
- `⠋ main.rs (starting)` — Server is starting
- `⠋ main.rs (indexing)` — Server is indexing the project
- `main.rs` — Server is ready

### Supported Languages

LSP works with any language server that implements the LSP protocol. Common examples:
- **Rust** - rust-analyzer
- **Python** - pylsp, pyright
- **TypeScript/JavaScript** - typescript-language-server
- **Go** - gopls
- **C/C++** - clangd

**Note:** You need to install the language server separately. TermIDE only provides the LSP client integration.

## Vim Mode

TermIDE includes an optional Vim-style editing mode with full Cyrillic keyboard support.

### Enabling Vim Mode

Enable Vim mode in your configuration file (`~/.config/termide/config.toml`):

```toml
vim_mode = true
```

### Available Modes

| Mode | Description | Indicator |
|------|-------------|-----------|
| Normal | Navigation and commands | (none) |
| Insert | Text input | `-- INSERT --` |
| Visual | Character selection | `-- VISUAL --` |
| Visual Line | Line selection | `-- VISUAL LINE --` |

### Mode Switching

| Key | Action |
|-----|--------|
| `Escape` | Return to Normal mode |
| `i` | Insert before cursor |
| `I` | Insert at line beginning |
| `a` | Append after cursor |
| `A` | Append at line end |
| `o` | Open line below |
| `O` | Open line above |
| `v` | Enter Visual mode |
| `V` | Enter Visual Line mode |

### Motion Keys (Normal/Visual)

| Key | Action |
|-----|--------|
| `h` / `←` | Move left |
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `l` / `→` | Move right |
| `w` | Next word start |
| `b` | Previous word start |
| `e` | Next word end |
| `0` | Line start |
| `$` | Line end |
| `^` | First non-blank character |
| `gg` | Go to first line |
| `G` | Go to last line |
| `{number}G` | Go to line number |

### Operators (Normal Mode)

| Key | Action |
|-----|--------|
| `d` | Delete (+ motion) |
| `dd` | Delete line |
| `D` | Delete to end of line |
| `c` | Change (+ motion) |
| `cc` | Change line |
| `C` | Change to end of line |
| `y` | Yank/copy (+ motion) |
| `yy` | Yank line |
| `p` | Paste after cursor |
| `P` | Paste before cursor |
| `x` | Delete character |
| `r` | Replace character |
| `u` | Undo |
| `Ctrl+R` | Redo |

### Cyrillic Keyboard Support

Vim mode works seamlessly with Cyrillic keyboard layouts. When typing in Russian or other Cyrillic layouts, all Vim commands are automatically translated:

- `о` (Russian) → `j` (move down)
- `л` (Russian) → `k` (move up)
- `н` (Russian) → `y` (yank)
- `в` (Russian) → `d` (delete)

This allows you to use Vim commands without switching keyboard layouts.

### Visual Mode Operations

In Visual or Visual Line mode:
- Use motion keys to extend selection
- `d` or `x` - Delete selection
- `y` - Yank selection
- `c` - Change selection (delete and enter Insert mode)
- `>` - Indent selection
- `<` - Unindent selection

### Search in Vim Mode

| Key | Action |
|-----|--------|
| `/` | Search forward |
| `?` | Search backward |
| `n` | Next match |
| `N` | Previous match |

**Note:** Standard editor shortcuts (`Ctrl+S`, `Ctrl+Z`, etc.) continue to work in all Vim modes.
