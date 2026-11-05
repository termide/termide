# CSV Table Viewer

Termide shows delimited text files (`.csv`, `.tsv`) as a table: column
headers, a grid of cells and a cell cursor, instead of the raw text. The table
is read-only; edit the file as text in the editor.

## Opening

- **`F3`** on a `.csv` / `.tsv` file in the file manager — open the table.
- **`Enter`** or **`F4`** — open the raw text in the editor (as for any text
  file).
- Open… (`Ctrl+G`), a bookmark or a followed link to such a file opens the
  table too.

The file is not loaded into memory. A background pass finds where its rows
start, and only the rows around the screen are read from disk, so a file of
any size opens at once. The first rows show as soon as they are found. While
the pass runs, `Loading…` shows in the corner and the row count in the status
bar ends in `…`.

## Table ↔ Source

The table and the source editor are two views of the same file. Switch between
them in place — the panel is replaced, not stacked:

- **`Ctrl+E`** (configurable via `[viewer.keybindings] toggle_view`).
- The clickable **`Edit`** chip in the status bar.

Switching back to the table is blocked while the source has unsaved changes —
save first.

## Layout of the file

The viewer guesses two things, shown in the status bar and switchable there
with a click or from the panel `[≡]` menu:

- **Delimiter** — `,`, `;`, `Tab` or `|`: the one that splits the first
  records into the same number of fields most often. `.tsv` files always use
  `Tab`. Switching cycles through the four and reads the file again.
- **Header** — whether the first record names the columns. It is taken for a
  header when none of its fields is empty or a number. Without a header the
  columns are named `A`, `B`, `C`, … as in a spreadsheet.

Quoted fields may hold the delimiter and line breaks; a line break inside a
cell shows as `↵`. Rows of different lengths are shown as they are, and a file
that is not valid UTF-8 is read with the bad bytes replaced.

## Keys

| Key | Action |
|-----|--------|
| `↑` `↓` `←` `→` | Move the cell cursor |
| `PageUp` / `PageDown` | Move by a screen |
| `Home` / `End` | First cell / last row |
| `s` | Sort by the cursor column: ascending, descending, file order |
| `y`, `Ctrl+C` | Copy the cell under the cursor |
| `Ctrl+R` | Read the file again |
| `Ctrl+E` | Switch to the source in the editor |

A click on a cell moves the cursor there; a click on a column header sorts by
it, as `s` does. Numbers sort by value and before text; empty cells sort last.
A sort reads the column through once more in the background: the table keeps
its current order, with `Sorting…` in the corner, until the sort is ready.
The panel `[≡]` menu also copies the whole row, tab-separated, ready to paste
into a spreadsheet.
