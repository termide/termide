# Markdown Preview

TermIDE renders Markdown files (`.md`, `.markdown`) as a read-only preview panel
instead of opening the raw source.

## Opening

- **`F3`** on a `.md` file — open the rendered preview.
- **`Enter`** or **`F4`** — open the raw source in the editor (as for any text
  file).

## Preview ↔ Source

The preview and the source editor are two views of the same file. Switch
between them in place — the panel is replaced, not stacked:

- **`Ctrl+E`** (configurable via `[viewer.keybindings] toggle_view`).
- The clickable **`Edit`** chip in the status bar.

In the preview, `Edit: No` means you are viewing the rendered document; clicking
it (or `Ctrl+E`) opens the **editable** source. In the source editor, the same
toggle returns to the preview. Switching back to the preview is blocked while
the source has unsaved changes — save first.

## What is rendered

Parsed with `pulldown-cmark` and drawn as text pseudographics:

- Headings (prefixed with `#` markers, accent colour, bold).
- Bold, italic, strikethrough, and inline `code`.
- Bulleted and ordered lists, including nesting.
- Block quotes, prefixed with `│`.
- Fenced code blocks, syntax-highlighted with the same engine as the editor.
- Tables, drawn with box-drawing borders. Columns are sized to their content
  and long cells wrap onto extra lines instead of being cut off.
- Horizontal rules and links (underlined, clickable). A web address written
  out in the text is a link too, as GitHub renders one; inside code it stays
  text.
- Images as a clickable `🖼` pictogram followed by the alt text (no terminal
  graphics protocol).
- Embedded ```` ```mermaid ```` code blocks, rendered as the diagram itself
  (text pseudographics) instead of raw source. Unsupported diagram kinds fall
  back to normal code-block highlighting. See [Mermaid diagrams](mermaid.md) for
  the standalone `.mmd` viewer and the list of supported diagram types.
- Embedded HTML — both block (`<p align>`, `<div>`, `<details>`/`<summary>`,
  `<table>`, `<img>`) and inline (`<kbd>`, `<b>`, `<sub>`, `<br>`, …) — rendered
  through the same engine as the standalone [HTML preview](html.md) instead of
  shown as literal angle-bracket text.

## Navigation, selection, links

The preview has a movable cursor and supports text selection:

- `↑`/`↓`/`←`/`→` (or `k`/`j`/`h`/`l`) — move the cursor.
- `PageUp`/`PageDown` (or `Space`) — page up/down; `Home`/`End` — line ends;
  `g`/`G` — document start/end.
- Hold **`Shift`** with movement, or **drag with the mouse**, to select text.
- **`Ctrl+A`** selects the whole document.
- **`Ctrl+C`** copies the selection (or the cursor's line when nothing is
  selected) to the clipboard.
- **`Ctrl+F`** searches; **`Ctrl+R`** reloads from disk; **`Ctrl+G`** opens a
  typed path or `http(s)://` URL in the matching viewer (see the
  [HTML preview](html.md) for the URL-fetch policy).
- Mouse wheel scrolls.
- **Follow a link** (click or `Enter`): web links open in the viewer by default,
  image links in the image preview, a link to another local file in the viewer
  for its type (a sibling `.md` in this preview), and `#heading` anchors jump
  within the page.
  `O` opens the link externally; `[`/`]` are history back/forward. See the
  [HTML preview](html.md) for the link-open settings and fetch policy.

The panel is saved with the project layout and reopens at the same file.
