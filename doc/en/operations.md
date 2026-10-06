# Operations Panel

The Operations panel surfaces every background file-system operation
(copy, move, upload, download, delete, batch transfer) and every
background command. Each operation gets its own card with a progress
bar, source/destination, transferred bytes and elapsed time.

## Opening and closing

The panel opens automatically when the first background operation
starts. It can also be opened from the application menu.

Operations keep running when you switch to another project, and the
panel goes along with you: it is shown in the project you switch to,
not left behind in the one you left. A command's card names the
project it runs in when that is not the current one.

It takes only the rows its cards need and leaves the rest of the column
to the panels next to it, growing and shrinking as operations start and
finish; with nothing to show it is a header and a one-line notice. It
never claims more than half of the column — beyond that the cards
scroll. Resizing it by hand (`Alt+Shift+=` / `Alt+Shift+-`, or dragging
its border) switches it to an ordinary fixed height until it is closed.

Closing follows the standard panel rules (`Alt+X`, `F10`, or `Esc`
when nothing is selected). With an operation highlighted, `Esc`
cancels that operation instead of closing the panel — see the
keybindings below.

## Keybindings

| Shortcut                       | Action                                            |
|--------------------------------|---------------------------------------------------|
| `↑` / `↓` (or `k` / `j`)       | Move the cursor between operations                |
| `Home` / `End` (or `g` / `G`)  | Jump to the first / last operation                |
| `Space`                        | Pause or resume the selected operation            |
| `Esc` / `Delete` / `Backspace` | Cancel the selected operation (asks to confirm)   |

Cancelling an operation always asks for confirmation first,
regardless of which key (or the popup menu) triggers it. `Esc` only
cancels when an operation is highlighted; with no selection it falls
through to the application's default close-panel-on-Esc.

## Per-operation popup menu

Clicking the bracketed type icon (`[↑]`, `[↓]`, `[⧉]`, `[➜]`, …) in
an operation card's border opens a popup menu scoped to that
operation:

- **Pause** / **Resume** — same effect as `Space`. Toggles the paused
  state of the operation.
- **Cancel** — same as `Esc` / `Delete`; asks for confirmation first.

While an operation is paused a small `⏸` indicator appears between
the bracketed type icon and the label (for example
`[↑] ⏸ Copy (upload) 23%`).

## Cancel cleanup

Cancelling an operation that has already moved bytes does **not**
silently leave junk behind. The panel asks what to do with the
remnants:

### Remote upload (single file)

If the cancelled operation is a remote upload, the panel surfaces a
modal:

```
Upload was cancelled

Delete partial upload 'filename'?
```

Choosing **Yes** (default) issues a remote delete of the in-flight
file. **No** leaves it on the server, so you can pick up the transfer
later — for example by re-running the same copy and letting your
server-side resume mechanism handle the seek.

### Local / remote batch

For a batch operation that has already produced files at the
destination, the modal offers two paths:

- **Delete copied** — removes everything the batch wrote so far.
- **Keep copied** — leaves the partial set in place.

The list of paths to clean up is built from the batch's tracked
destinations, so only files that came from this cancelled batch are
affected; pre-existing files at the destination stay untouched.
