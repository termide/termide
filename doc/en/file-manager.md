# File Manager

The file manager panel provides an intuitive interface for navigating the file system and performing operations on files and directories.

Remote filesystems (SFTP / FTP / FTPS) appear in the same panel as
local paths — see [Remote Filesystems](vfs.md) for URL syntax and
authentication setup.

## Navigation

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `↑` / `↓`         | Move cursor up/down                        |
| `Enter`           | Enter directory, preview media, or open file |
| `Backspace`       | Go to parent directory                     |
| `~`               | Go to home directory                       |
| `PageUp` / `PageDown` | Scroll list by one page                |
| `Home` / `End`    | Go to beginning/end of list                |
| `.`               | Toggle hidden files visibility             |
| `→` / `l`       | Expand directory (tree view)                |
| `←` / `h`       | Collapse directory (tree view)              |
| `/`              | In-tree incremental search                  |
| `Ctrl+/`          | Open directory switcher                    |
| `Ctrl+G`          | Go to path/URL                             |
| `Alt+K`           | Add bookmark                               |
| `Tab`             | Go to next panel                           |
| `Shift+Tab`       | Go to previous panel                       |

## File Selection

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Insert`          | Toggle selection of current file           |
| `Shift + ↑ / ↓`   | Select multiple consecutive files          |
| `Ctrl+A`          | Select all files and directories in panel  |
| `Escape`          | Clear all selections                       |

In tree view, selecting a directory with `Insert` cascades the selection to all files within it. Collapsing a selected directory keeps the selection on its children.

## File Operations

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+N`          | Create new file                            |
| `D` / `F7`        | Create new directory                       |
| `Delete` / `F8`   | Delete selected files/directories          |
| `C` / `F5`        | Copy selected files/directories            |
| `M` / `F6`        | Move/rename files/directories              |
| `E` / `F4`        | Open file in editor                        |
| `R` / `F2`        | Rename file/directory                      |
| `V` / `F3`        | View file (preview without executing)      |
| `Ctrl+R`          | Refresh current directory contents         |
| `Space`           | Show file/directory information            |

## Search

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+F`          | Search files by name (glob patterns)       |
| `Ctrl+Shift+F`    | Search in file contents                    |
| `Ctrl+Shift+H`    | Search & replace in file contents          |
| `/`              | In-tree incremental search (filter as you type) |

These searches use an **inline bar docked at the top of the panel** (not a
floating modal), with a separator line above the results. The bar and the
results are two **zones**: `Tab` switches between them (like the git-status
panel). In the bar zone, arrow keys move between the fields and toggles. In the
results zone the cursor lands on the **entry rows** (files/folders, or file
groups), like the diff panel: `↑` / `↓` move between them, `PageUp` / `PageDown`
page, `←` / `→` collapse / expand the entry at the cursor, and `Enter` opens it
(a folder/file header toggles or opens). A mouse click selects a row, a
double-click opens it (or toggles a group), and the wheel scrolls. `Esc` exits
the search (it does not close the panel).

### File Search (Ctrl+F)

An inline bar with a single `Find:` field that filters files by glob in real
time; results (relative paths with git-status colors) appear below the
separator as a tree. `Tab` into the results; `↑` / `↓` move across files **and
folders**, `←` / `→` collapse / expand a folder, `Enter` opens the file
(placing the cursor on it in the tree).

Matching is a **case-insensitive substring** by default; the `[Aa] Case` and
`[.*] Regex` toggles change it: `[Aa]` makes matching case-sensitive, and
`[.*]` treats the query as a regular expression over the file name (click a
toggle, or focus it and press `Enter` / `Space`).

`Ctrl+R` re-runs the search against the current directory contents.

### Content Search (Ctrl+Shift+F)

An inline bar with `Find:` (glob mask, defaults to `*`) and `Text:` (the content
query) fields:
- Matching is **literal by default**; toggle `[.*] Regex` for regular
  expressions and `[Aa] Case` for case sensitivity (click, or focus the toggle
  and press `Enter` / `Space`).
- Searches only in text files (binary files are skipped); large files are
  skipped (configurable limit in settings).
- Results are **grouped by file** below the separator, like the diff panel: the
  cursor moves between **file headers** (`[▼]`/`[▶]` collapse marker + path +
  match count), with up to 5 match lines shown under each (line number + matched
  line, hit highlighted) and a `+ N more` row when a file has more. `←` / `→`
  collapse / expand a file, `Enter` opens it at its first match.

### Content Replace (Ctrl+Shift+H)

`Ctrl+Shift+H` opens the same content bar with an extra `Repl:` field. Once a
replacement is typed, every shown match renders as a `-old/+new` preview.

Each file header gets a **selection checkbox** (`[ ]` right of the collapse
triangle), and replacement applies **only to checked files** — nothing is
checked by default. Toggle the file at the cursor with `Space` or a click; the **`[ ] Select all`**
checkbox button (or `a`) selects/clears every file at once. The bar's
right-hand status shows the live `selected / total files · matches` count.

Press `Enter` in the `Repl:` field (or activate the **Replace** button) to
replace every match in the **selected** files — after a confirmation showing how
many occurrences in how many files (nothing selected shows a hint instead). With
`[.*] Regex` on, the replacement supports `$1` / `${name}` capture groups;
otherwise it is inserted verbatim. Replacements are written to disk.

### In-tree Search (/)

Press `/` to start incremental search within the current directory tree:
- Filters the file list as you type, showing only matching entries
- Parent directories of matching files remain visible for context
- Press `Enter` to confirm and navigate to the match
- Press `Escape` to cancel and restore the full tree
- Works together with the tree view — matched directories auto-expand

## Clipboard

| Shortcut           | Action                                     |
|-------------------|--------------------------------------------|
| `Ctrl+C`          | Copy paths of selected items               |
| `Ctrl+X`          | Cut paths of selected items                |
| `Ctrl+V`          | Paste files from clipboard                 |

### Target directory rule

Create-new-file, create-new-directory and paste-from-clipboard all
land **at the cursor's tree level**, not always in the panel's root:

- Cursor on a top-level entry → the action targets the panel's
  current directory.
- Cursor inside an expanded subdirectory → the action targets that
  subdirectory.

This matches the visual position of the cursor — what you see is where
the file is created or pasted.

## Git Integration

The file manager displays file status in Git repositories:

- **File status colors** — new, modified, deleted, and untracked files are color-coded
- **Nested git status** — directories show aggregated status of their children (e.g., a directory containing modified files is highlighted)
- **Tree view integration** — git status propagates through the directory tree, making it easy to locate changes in deep hierarchies

## Media Preview

The file manager can preview images and videos using console image viewers.

**File opening logic:**

| File type | Action |
|-----------|--------|
| Raster images (PNG, JPG, JPEG, GIF, WebP, BMP, TIFF) | ImagePanel (native graphics) or xdg-open fallback |
| Vector images (SVG, ICO) | xdg-open (system viewer) |
| Videos (MP4, MKV, AVI, MOV, WebM, FLV, WMV, M4V) | xdg-open (system player) |
| Binary files | xdg-open (system default) |
| Text files | Editor panel |
| Executable files | Run in terminal |

**Shortcuts:**
- `Enter` → smart open (see table above)
- `F3` → view file (like Enter, but executables open in editor instead of running)
- `O` / `Alt+Enter` → force open with xdg-open (system default application)
- `F4` → always open in editor

**Native Graphics:**
termide automatically detects if the parent terminal supports graphics protocols (Kitty, Sixel, iTerm2). When supported, raster images are rendered directly in the ImagePanel without external tools.

**Supported terminals:**
- Kitty, WezTerm, iTerm2, Ghostty, foot - full graphics support
- Other terminals - fallback to xdg-open

## Mouse Support

- **Single click**: Select a file or directory
- **Double click**: Enter directory or open file
- **Scroll wheel**: Scroll through file list

## Display

### Font Modifiers

- **Italic** — symlinks (files and directories)
- **Bold** — executable files

### Sorting Order

Files are displayed in groups, each sorted alphabetically:
1. Directories
2. Executable files
3. Regular files

### Symlinks

- Copy modal includes a "Create symlink" checkbox — creates a symbolic link instead of copying
- When "Create symlink" is enabled, a "Use relative target" checkbox appears and makes the new link point to a relative path instead of an absolute one
- Navigating into a symlink directory follows the link to the target
- Pressing `Space` on a symlink opens the properties modal with a **Target** row that shows the raw link contents (as stored in the link, not resolved through `canonicalize`) with `~` shortening for the home directory
- The status bar shows `→ target` after the file name for the currently selected symlink; this also works for symlinks nested inside expanded subdirectories of the tree view

### Notes

- `..` (parent directory) cannot be selected — it is only for navigation
- Input modals support `~/` expansion to home directory
- The status bar shows a **Size:** field (to the right of **Owner:**) for the entry under the cursor: a plain byte size for files, and `<size> (N items)` for directories — the recursive size plus the immediate (non-recursive) child count. The count appears immediately while the recursive size fills in. The properties modal (`Space`) shows the same on its **Size** line (e.g. `1.2 MB (42 items)`).

## Configuration

The file manager reads its settings from `[file_manager]` in
`config.toml`. The most user-visible options:

| Key                                | Default | Description                                                                                                  |
|------------------------------------|---------|--------------------------------------------------------------------------------------------------------------|
| `extended_view_width`              | `50`    | Minimum panel width (columns) before the extended view shows the size / modified-time columns.               |
| `content_search_max_file_size_mb`  | `1`     | Maximum file size considered by `Ctrl+Shift+F` content search. Larger files are skipped.                     |
| `dir_size_in_wide_view`            | `true`  | Compute and show directory sizes in the Size column of the extended view. Local filesystems only.            |
| `dir_size_budget_ms`               | `100`   | Per-directory time budget (ms) for that walk. Trees that don't finish render a `-` marker. `0` disables it.  |

The walks share a process-wide cache, so two panels viewing the same
directory don't double-walk. Pressing `Space` on a directory also
publishes its exact (unbounded) size into that cache, so the
wide-view column picks the result up on the next redraw.
