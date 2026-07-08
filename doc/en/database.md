# Database Viewer

TermIDE includes a built-in, **read-only** database browser for **SQLite**,
**PostgreSQL** and **MySQL/MariaDB**. It connects over the native wire protocol
(no external client tools required), lists a database's tables, and shows rows
in a scrollable grid with sorting and per-column filtering.

The viewer never writes: it only issues `SELECT` and catalog queries.

## Opening a connection

Connections are ordinary **bookmarks** whose path is a database URL. Add a
bookmark (Bookmarks menu → Add) and put the connection URL in the *Path* field:

| Engine | URL example |
|--------|-------------|
| SQLite | `sqlite:///absolute/path/to/app.db` |
| PostgreSQL | `postgres://user@host:5432/mydb` |
| MySQL / MariaDB | `mysql://user@host:3306/mydb` |

Activating a database bookmark opens the Database panel and connects in the
background — the UI never blocks while connecting.

There are two more ways to open a database without a bookmark:

- **Open a local SQLite file directly.** Press `Enter` or `F3` on a `.db` /
  `.sqlite` / `.sqlite3` / `.db3` file in the file manager — it opens in the
  database viewer instead of the hex viewer.
- **Windows ▸ Open…** (or `Ctrl+G` in a viewer) accepts a connection URL
  (`sqlite://` / `postgres://` / `mysql://`) as well as a file path, a
  directory, or an `http(s)://` address, and routes it to the matching viewer.

### Passwords

As with SFTP/FTP bookmarks, the password is **not** treated specially: the
bookmark stores exactly the URL you enter. You can either rely on a
password-less authentication path (PostgreSQL `~/.pgpass`/peer/trust, a Unix
socket, `PGPASSWORD`, …) or include the password directly in the URL
(`postgres://user:secret@host/db`). If you embed it, note that the bookmark is
saved verbatim to `bookmarks.toml`, so treat that file accordingly.

## Layout

```
┌─ Database ──────────────────────────────────────────┐
│ Table ▾ users                                        │
│ id │ name  │ score │ active                          │
│  1 │ alice │ 1.5   │ true                             │
│  2 │ bob   │ 2.25  │ false                            │
│  3 │ carol │ NULL  │ true                             │
└──────────────────────────────────────────────────────┘
```

A **table selector** sits on top; the **data grid** fills the rest. When the
connection URL omits a database (PostgreSQL/MySQL), a **database selector**
appears to its left; the viewer auto-selects the first database so data shows
immediately, and you can switch to another via that selector. The grid
has a 2D cell cursor: the highlighted cell is the target for sorting, filtering
and copying. A **vertical scrollbar** on the right shows how far through the
whole table you are (across pages), and a **horizontal scrollbar** along the
bottom appears when the columns are wider than the panel. The shared status bar
shows the current range, total row count, active sort and filter, e.g.
`app.db · users · rows 1–200 of 1203 · sort: name ↑ · filter: 1`.

## Keys

| Key | Action |
|-----|--------|
| `Tab` | Switch focus between the table selector and the grid |
| `Enter` / `Space` (on selector) | Open the table dropdown |
| `↑ ↓ ← →` | Move the cell cursor (auto-scrolls rows and columns) |
| `PageUp` / `PageDown` | Move a screenful; loads the next/previous window at the edges |
| `Home` / `End` | Jump to the first / last row |
| `S` | Sort by the current column — cycles ascending → descending → unsorted |
| `Ctrl+F` / `F3` | Filter the current column (opens the filter dialog) |
| `Alt+F` | Clear all filters |
| `Space` / `F12` (on grid) | Show the full current row (key/value), with copy options |
| `Ctrl+C` | Copy the current cell value |
| `Ctrl+Y` | Copy the current row as tab-separated values |
| `Ctrl+R` | Refresh the table list and reload the current view |

These action keys are configurable under `[database.keybindings]` in the config
and appear in the Help panel. Navigation keys (Tab, arrows, PageUp/Down,
Home/End) are fixed.

**Mouse:** click the table selector to open it, click a column header to cycle
its sort, and click a data cell to move the cursor there.

## Sorting

Press `s` on a column to sort by it server-side. The cycle is
**ascending → descending → unsorted**; "unsorted" issues no `ORDER BY`, which is
the cheapest option on large tables. The sorted column shows a `↑`/`↓` arrow in
its header. One column at a time.

> On PostgreSQL, paging through an *unsorted* table can return rows in a
> slightly different order between pages (the engine's natural order isn't
> stable across `OFFSET`s). Apply a sort if you need a stable order.

## Filtering

`Ctrl+F` (or `F3`) opens the filter dialog, which lists **every column** — one
row each with the column name, an **operator selectbox** and a value. Within a
row, ←/→ switch focus between the operator and the value, and ↑/↓ move between
rows; press Enter on the operator to open its dropdown (the first option means
"no condition"). The available operators depend on the column's type:

- **Text:** contains, starts with, ends with, =, ≠, is null, is not null
- **Numeric / date:** =, ≠, >, ≥, <, ≤, is null, is not null
- **Boolean:** =, ≠, is null, is not null

Typing a value auto-selects a default operator if the column was still `—`, so
you can just type. Bottom buttons: **Apply**, **Clear filters**, **Cancel**.
Applying replaces the whole filter set; conditions on different columns combine
with **AND**; an operator left without a value is ignored. `contains`/`starts
with`/`ends with` are case-insensitive. `Alt+F` clears all filters without
opening the dialog. Values are always sent as bound parameters; if a filter
produces a database error (e.g. a non-numeric value on a numeric column) it is
shown as a non-fatal warning in the status bar and the current data stays.

## Row detail

Press `Space` on a row to open a detail dialog listing every column as
key/value — useful for long text, JSON or wide rows that don't fit the grid. The
dialog can copy the row as **TSV**, **JSON**, or an **INSERT** statement.

## Pagination

Rows are fetched in windows; only the current window is held in memory, so even
very large tables stay responsive. Moving past the end of a window loads the
next one automatically. The total row count (respecting the active filter) is
fetched in the background and appears in the status bar once ready.

## Connection recovery

If the database connection drops or a query fails fatally, the panel
shows a recovery dialog: **Reconnect** opens a fresh connection to the
same URL, **Close panel** closes the viewer. `Esc` dismisses the dialog
and leaves the last results visible. The panel never retries on its own,
so there is no error loop.

## Limitations (current)

- Read-only — no editing, no arbitrary SQL.
- Exotic column types (JSON, arrays, UUID, timestamps, unsigned integers) are
  shown as empty/`NULL` in this version; common scalar types render fully.
- Database selection is supported when the URL omits it; schema selection
  (PostgreSQL) still defaults to the current schema.
- No in-app password prompt yet — use a password-less auth path or include the
  password in the URL (see above).
