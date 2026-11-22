# Keybindings

Termide canonicalizes every key event before matching it against a
binding. The canonicalization runs once at the dispatch boundary and
its result, together with the original raw event, travels through the
panel pipeline as a `KeyChord`. Panels and modals pick whichever form
they need:

- **`canonical`** — for hotkey matching, vim command interpretation,
  Settings keybinding capture.
- **`raw`** — for text input (`InsertChar`), terminal-panel PTY
  passthrough, search-buffer typing.

Text typed inside the editor or sent to a program running in the
terminal panel is **never** rewritten by canonicalization, so Cyrillic,
shifted glyphs, and locale-specific characters reach the destination
unchanged. The one exception is a `Ctrl` chord on a letter of another
layout sent to the terminal: a Cyrillic letter has no control code, so
`Ctrl+С` goes out as the `Ctrl+C` (`^C`) of its physical key.

A text field (a modal input, a find bar, the agent prompt) matches its own
`Ctrl` shortcuts — select all, copy, cut, paste, undo — on the canonical
form too, while the letters typed into it stay raw.

## What canonicalization fixes

| Quirk | Behaviour |
| --- | --- |
| Cyrillic letters on the same physical key as Latin (`й`/`q`, `ь`/`m`, …) | Mapped to Latin so a binding `Alt+M` fires whether the active layout is QWERTY or ЙЦУКЕН. |
| `REPORT_ALTERNATE_KEYS` shifted-glyph rewrite | Crossterm rewrites `Shift+Ctrl+=` → `Char('+') + Ctrl` (Shift stripped, char swapped). The normalizer reverses that — `Char('+') + Ctrl` → `Char('=') + Ctrl + Shift`. |
| Caps Lock spurious Shift on letters | When `REPORT_EVENT_TYPES` flagged `KeyEventState::CAPS_LOCK`, the Shift bit attached to letters is dropped before matching. |
| VTE `Ctrl+/` collapsing to `Ctrl+7` | Only when Kitty proto is **not** active: `Ctrl+7` → `Ctrl+/`. |

## Universal vs Enhanced bindings

Some chords cannot be encoded by every terminal. Termide groups
defaults into two tiers and warns at startup if the active terminal
cannot deliver any of the configured Enhanced-tier chords.

### Universal tier (works on every VT100+ terminal)

- `Alt+letter`, `Ctrl+letter` (letters → ASCII control 0x01–0x1A).
- `F1`–`F12` and `F1`–`F12` with **a single** modifier (`Shift+F*`,
  `Alt+F*`, `Ctrl+F*`).
- Arrows with **a single** modifier (`Shift+Up`, `Ctrl+Up`, `Alt+Up`),
  and with `Ctrl+Shift` (`Ctrl+Shift+Up`, sent as `\e[1;6A`).
- `Home`, `End`, `PgUp`, `PgDn` + a single modifier.
- `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert` + a single
  modifier.
- `Alt+digit`.
- `Alt+punctuation` (`Alt+/`, `Alt+,`, `Alt+.`, …).

### Enhanced tier (requires Kitty keyboard protocol)

- `Ctrl+punctuation` (`Ctrl+-`, `Ctrl+=`, `Ctrl+,`, `Ctrl+.`) —
  except `Ctrl+/` and `Ctrl+\`, which reach termide on legacy terminals
  too (see the note under the defaults below).
- `Ctrl+Shift+letter`.
- `Ctrl+Alt+anything`.
- `Alt+Shift+letter` and `Alt+Shift+arrow` — VTE in legacy mode emits
  `\eL` for `Alt+Shift+l`, indistinguishable from `Alt+L`; an
  `Alt+Shift+...` binding cannot match.
- `Super` / `Meta` / `Hyper` modifiers.

Enhanced-tier defaults that termide ships are kept because they are
de-facto standards across editors:

| Binding | Action | Section | Universal alternative |
|---|---|---|---|
| `Ctrl+Alt+R` | `replace_all` | `editor` | `Alt+R` |
| `Ctrl+.` | `toggle_comment` | `editor` | `Ctrl+/` |
| `Ctrl+Shift+Z` | `redo` | `editor` | `Ctrl+Y` |
| `Ctrl+Shift+S` | `save_as` | `editor` | — |
| `Ctrl+Shift+R` | `reload` | `editor` | — |
| `Ctrl+Shift+F` | `search_content` | `file_manager` | — |
| `Ctrl+Shift+H` | `search_replace` | `file_manager` | — |

A terminal without Kitty proto sends `Ctrl+Shift+<letter>` as plain
`Ctrl+<letter>`, so such a key reaches the action bound to that one
instead: `Ctrl+Shift+S` saves in place, `Ctrl+Shift+Z` undoes,
`Ctrl+Shift+R` replaces the current match while a replace is active, and
`Ctrl+Shift+F` opens the file manager's name search. On such a terminal,
termide logs a startup warning listing the affected bindings; rebind them
through Settings → Keybindings.

## macOS: Option is not Alt

On macOS every terminal treats `Option` as a **text-composition**
modifier by default — Ghostty `macos-option-as-alt=false`, kitty
`macos_option_as_alt no`, iTerm2 Option=Normal, Terminal.app "Use Option
as Meta key" off. `Option+F` therefore arrives as the composed glyph `ƒ`
with no ALT bit, and every `Alt+<character>` default is unreachable: the
`Alt+<letter>` global actions, `Alt+1`…`Alt+9` (go to project), `Alt+\`,
`Alt+-` and `Alt+=`, and the editor's `Alt+,`, `Alt+.` and `Alt+R`. How far that reaches depends on the terminal: on
Ghostty the keys that produce no text keep their ALT bit, so `Option+F9`,
`Option+Up/Down` and `Option+Backspace` still work, while Terminal.app
strips the modifier from those too and delivers a bare `Up`.

Termide's remedy is the Kitty `REPORT_ALL_KEYS_AS_ESCAPE_CODES` flag,
which makes the terminal report `Option+F` as `Alt+F` even while the
system keeps composing. It is pushed at startup on macOS only, on
terminals that advertise the Kitty keyboard protocol, and is controlled
by:

```toml
[general]
report_all_keys = true  # default
```

It is also *Option as Alt* in the Input group of the settings window's
General page; a change takes effect when termide starts again.

The flag has one cost: dead-key and IME composition no longer reaches
termide, so `Option+E` `E` → `é` stops working inside the application.
Users who need composed input should set `report_all_keys = false` and
either rebind the affected actions or switch `Option` to `Alt` in the
terminal itself (Ghostty `macos-option-as-alt = true`, Terminal.app "Use
Option as Meta key"). The flag also makes the terminal report standalone
modifier presses; those are dropped at the event boundary.

When `Alt+<key>` bindings cannot fire, termide logs a startup warning to
the Journal naming the remedy for the terminal at hand.

### Ghostty rebinds Option+Left / Option+Right

Independently of the composition problem above, Ghostty ships these
macOS defaults:

```
keybind = alt+arrow_left=esc:b
keybind = alt+arrow_right=esc:f
```

They implement the readline word-motion convention, and they fire before
the key ever reaches termide: `Option+Left` arrives as the two bytes
`ESC b`, which parse as `Alt+B`, and `Option+Right` as `Alt+F`. So on
Ghostty those two chords do not merely fail to reach `prev_group` /
`next_group` — they trigger whatever is bound to `Alt+B` and `Alt+F`,
which by default are *Add bookmark* and *New file manager*.

Nothing in termide can undo this: Ghostty applies the keybind before the
keyboard protocol gets a say, so `Option+Left` arrives as the two bytes
`ESC b` and is indistinguishable from a real `Alt+B`. Ghostty's keybind
prefixes (`global:`, `all:`, `unconsumed:`, `performable:`) and key tables
have no way to scope a binding to one application.

Termide used to ship `Alt+A` / `Alt+D` as alternatives for these two
actions, which sidestepped the problem without touching Ghostty. Those
letters went to actions users reach for first — `Alt+D` detaches and
`Alt+A` opens the agent panel — so the fix now belongs in Ghostty's config.

To get `Option+Left` / `Option+Right` back, clear the bindings in
`~/.config/ghostty/config`:

```
keybind = alt+arrow_left=unbind
keybind = alt+arrow_right=unbind
```

Reload with `Cmd+Shift+,` or restart Ghostty. Note that this is a
terminal-wide setting, so it also removes readline word motion in your
shell. Restore that on the shell side — for zsh, in `~/.zshrc`:

```zsh
bindkey "^[[1;3D" backward-word   # Option+Left
bindkey "^[[1;3C" forward-word    # Option+Right
```

or for bash, in `~/.inputrc`:

```
"\e[1;3D": backward-word
"\e[1;3C": forward-word
```

Those are the standard xterm sequences Ghostty sends for `Alt+Left` /
`Alt+Right` once its own bindings are gone; `cat -v` followed by the
keypress confirms what your terminal actually emits.

`Option+Up` / `Option+Down` carry no such Ghostty binding and reach
`prev_panel` / `next_panel` normally.

### Alt+Shift+= and Alt+Shift+- do not reach termide on macOS

`panel_grow_vertical` and `panel_shrink_vertical` are the one pair of
defaults macOS keeps out of reach. Option composes `Option+Shift+=` into a
single glyph — `±` on a Latin layout, `«` on a Russian one — and the
terminal reports that glyph in place of the key. The mapping is
layout-specific, so it cannot be reversed the way `+` → `Shift+=` is on
other platforms.

Resize vertically with the mouse, or rebind both actions through
Settings → Keybindings to chords without `Shift`.

### Terminal.app cannot use the remedy at all

Terminal.app does not implement the Kitty keyboard protocol, so
`report_all_keys` has nothing to switch on and every `Alt+<letter>` binding
stays unreachable until **Settings → Profiles → Keyboard → "Use Option as
Meta key"** is enabled. With it on, `Option+F` reaches termide as `Alt+F`.

Its arrow keys need separate attention even then. Terminal.app ships
profile key mappings that send `ESC b` / `ESC f` for `Option+Left` /
`Option+Right` — the same collision Ghostty has, arriving as `Alt+B` /
`Alt+F` — and it drops the modifier entirely from `Option+Up` /
`Option+Down`, which arrive as bare `Up` / `Down`. Both are edited in the
key-mapping list on that same Keyboard tab. Measured on macOS 26.5:

```
Option+Left   -> Alt+Char('b')      Option+Up   -> Up   (no ALT)
Option+Right  -> Alt+Char('f')      Option+Down -> Down (no ALT)
```

`Option+Up` / `Option+Down` reach termide unchanged once Option-as-Meta is
on; only the horizontal pair needs the Ghostty unbind above.

### macOS: `Option+Z` cannot be bound

`Alt+<letter>` is universal-tier and works on macOS once the Kitty protocol is
active — with one exception. `Option+Z` is the only combination on the US
layout that composes an **uppercase** glyph, `Ω` (U+03A9). The terminal
classifies it as a shifted character and reports it as:

```
Char('Ω') + SHIFT      ← no ALT bit at all
```

where `Option+Q` and `Option+T` correctly report `Char('q') + ALT` and
`Char('t') + ALT`. Canonicalization cannot recover this: the ALT bit never
arrived, and mapping `Ω` back to `z` would misfire for anyone typing Greek.

So an `Alt+Z` binding simply never matches on macOS. Pick another letter — this
is why `detach_instance` defaults to `Alt+D`.

### macOS reserves some function keys

`F11` is bound to *Show Desktop* by macOS Mission Control and never
reaches the terminal, so the `F11` alternative for *Toggle stack* does
not work out of the box; use `Alt+Backspace`.

More broadly, unless **System Settings → Keyboard → "Use F1, F2, etc.
keys as standard function keys"** is enabled, the F-row sends media keys
and no `F<n>` binding is reachable at all. For most global actions that
only costs the F-key half of a pair — `Alt+M` still opens the menu when
`F9` does not. But these defaults are bound to a function key and
nothing else, so they are genuinely unreachable until the setting is
turned on:

| Action | Binding | Section |
|---|---|---|
| Maximize the panel in its group | `Alt+F11` | `general` |
| Delete line | `F8` | `editor` |
| Find next | `F3` | `editor` |
| Find previous | `Shift+F3` | `editor` |
| Go to definition | `F12` | `editor` |
| Find references | `Shift+F12`, `F24` | `editor` |
| Rename symbol | `F4` | `editor` |
| Toggle breakpoint | `Ctrl+F8` | `editor` |
| Start debugging | `F5` | `editor` |
| Next change | `F7` | `editor` |
| Previous change | `Shift+F7` | `editor` |
| Unpack archives | `Alt+F9` | `file_manager` |
| View file | `F3` | `git_status` |
| Edit file | `F4` | `git_status` |
| Clear the filter | `Shift+F3` | `database` |

The agent panel's own `F2`–`F8` are fixed keys and go the same way; other
ways in stand for them: `/rename` for `F2`, `/usage` for `F3`, `Esc` in the
empty prompt for `F4`, `/fork` for `F5`, the list of sessions on a fresh
session's banner for `F6`, `/new` for `F7` and `/clear` for `F8` (see
[Agent](agent.md)).

Termide logs a startup warning listing them when it starts on macOS.
Either enable the setting, or rebind these actions through
Settings → Keybindings.

## Terminal compatibility (2026)

| Terminal | Kitty keyboard protocol |
| --- | --- |
| kitty | full |
| foot 1.13+ | full |
| WezTerm | full |
| Ghostty | full |
| iTerm2 | full |
| rio | full |
| Windows Terminal Preview 1.25+ | full |
| alacritty | partial (CSI-u, no enhancement flags) |
| xterm | partial (manual config) |
| GNOME Terminal / Tilix / VTE | none (in progress) |
| Konsole | none (planned) |
| tmux | pass-through (depends on host terminal) |

If your terminal does not advertise Kitty proto and you rely on
Enhanced-tier chords, either switch to a supporting terminal or rebind
the affected actions to Universal-tier alternatives in
`config.toml` → `[*.keybindings]`.

## Conflict detection

Settings → Keybindings shows an inline warning when you assign a chord
already in use by another action. A chord the same action already
accepts is refused outright — it would only put a second spelling of a
key the action already answers to into the row — and the reason is shown
in its place. Three classes of conflict are detected:

- **Same section** — two actions in the same section share the chord;
  the second one becomes unreachable.
- **Cross-section shadow** — a global chord shadows a panel-local one;
  the panel binding never fires.
- **Cross-section ambient** — two panel-local bindings overlap; only
  the focused panel handles the event, so usually fine but worth
  noting.

When a chord has several holders, the warning names the one that matters
most: an action in the same section first, then a global action, which
takes the chord before any panel sees it. A command's hotkey counts as
much as a global action and is named as `commands.<name>`.

Same-section conflicts and cross-section shadows are also logged at
startup, a same-section clash even when the chord is bound in other
sections as well.

## Customising defaults

Override any binding in `config.toml`. Strings are parsed in canonical
form, so `"Alt++"` ≡ `"Alt+Shift+="` and `"Ctrl+Й"` ≡ `"Ctrl+Q"`:

```toml
[general.keybindings]
panel_grow_vertical = "Alt+Shift+="
panel_shrink_vertical = "Alt+Shift+-"
open_projects = "Alt+\\"

[editor.keybindings]
trigger_completion = ["Ctrl+J", "Ctrl+Space"]
toggle_comment = ["Ctrl+/", "Ctrl+."]
replace_all = ["Ctrl+Alt+R", "Alt+R"]

[file_manager.keybindings]
switch_directory = "Ctrl+\\"

[terminal.keybindings]
switch_directory = "Ctrl+\\"
```

Note: `Ctrl+/` and `Ctrl+\` work even on legacy terminals (e.g. VTE)
through `KeyNormalizer` quirks — VTE sends those as `\x1F` and `\x1C`
control bytes, which crossterm parses as `Ctrl+7` / `Ctrl+4` and the
normalizer rewrites back to the slash / backslash chord.

Multiple alternatives are supported for any action: list them in an
array. The help window shows them all; a menu shows the first.

Three actions are configured in one section and used in another. The git
log opens a file outside termide with `open_external` from
`[file_manager.keybindings]` (`O` and `Alt+Enter` by default), so a key
rebound or cleared there changes both panels. The editor swaps itself for
the hex view of its file with `toggle_hex` from `[viewer.keybindings]`
(`Ctrl+L`), and switches between viewing and editing with `toggle_view`
from the same section (`Ctrl+E`).

The same shape is editable without touching the file: in Settings →
Keybindings each row lists every key its action accepts, `Left`/`Right`
move a cursor between them, `Enter` captures a chord for the key under
the cursor, and the `[+]` drawn at the end of the focused row appends a
new alternative. See [ui.md](ui.md#settings-modal).

### Unbound actions

Some actions ship without a binding. For `prev_project` / `next_project`
every chord still free collides with a desktop or shell shortcut somewhere:
`Ctrl+Alt+Left` / `Ctrl+Alt+Right` switch workspaces in several Linux
desktops, and `Alt+.` inserts the last argument in bash. `goto_panel_1` …
`goto_panel_9` gave `Alt+1` … `Alt+9` to `goto_project_1` …
`goto_project_9`, which switch to the open project with that number. Bind them
to what is free on your system. The editor and terminal actions in the
table have no key either; the panel menu (`Alt+K`) offers them where they apply:

| Action | Does |
|--------|------|
| `prev_project` / `next_project` | Switch to the previous / next open project, in the order the `Projects` menu lists them |
| `goto_panel_1` … `goto_panel_9` | Move the focus to the panel group (column) with that number |
| `fold_all` / `unfold_all` (`[editor.keybindings]`) | Fold / unfold every block of the file |
| `stage_change` / `unstage_change` / `revert_change` (`[editor.keybindings]`) | Stage, unstage or revert the change at the cursor or on the selected lines |
| `send_to_agent` (`[editor.keybindings]`, `[terminal.keybindings]`) | Put the selection (in a terminal also the last command or the last lines) into the agent's prompt |
| `copy_command_output` (`[terminal.keybindings]`) | Copy the last command's output |

```toml
[general.keybindings]
prev_project = "Shift+F5"
next_project = "Shift+F6"
goto_panel_1 = "Shift+F1"
```

To give the digits back to panels, move the projects off them in the same
file. A `goto_panel_N` bound to `Alt+N` while `goto_project_N` is not set is
taken for a copy of the old default saved by an older version, and gives way:

```toml
[general.keybindings]
goto_panel_1 = "Alt+1"
goto_project_1 = "Shift+F1"
```

The `goto_project_N` key is shown beside the project in the `Projects` menu.

### All actions

Every action each section accepts, with its keys out of the box. Settings → Keybindings lists the same names. A key in `config.toml` replaces the whole list of its action; `[]` leaves the action without a key.

#### `[general.keybindings]`

| Action | Default keys |
|---|---|
| `toggle_menu` | `Alt+M`, `F9` |
| `new_file_manager` | `Alt+F` |
| `new_terminal` | `Alt+T` |
| `new_editor` | `Alt+E` |
| `new_journal` | `Alt+L` |
| `open_help` | `Alt+H`, `F1` |
| `open_preferences` | `Alt+P` |
| `open_projects` | `Alt+\` |
| `new_project` | `Alt+N` |
| `open_git_status` | `Alt+G` |
| `open_bookmark_add` | `Alt+B` |
| `open_outline` | `Alt+O` |
| `open_agent` | `Alt+A` |
| `open_diagnostics` | `Alt+I` |
| `open_git_log` | `Alt+C` |
| `close_panel` | `Alt+W`, `Alt+X`, `F10` |
| `toggle_stack` | `Alt+Backspace`, `F11` |
| `swap_left` | `Alt+PageUp` |
| `swap_right` | `Alt+PageDown` |
| `move_first` | `Alt+Home` |
| `move_last` | `Alt+End` |
| `resize_smaller` | `Alt+-` |
| `resize_larger` | `Alt+=` |
| `toggle_fullscreen_panel` | `Alt+F11` |
| `panel_grow_vertical` | `Alt+Shift+=` |
| `panel_shrink_vertical` | `Alt+Shift+-` |
| `panel_action_menu` | `Alt+K`, `Shift+F10` |
| `prev_group` | `Alt+Left` |
| `next_group` | `Alt+Right` |
| `prev_panel` | `Alt+Up` |
| `next_panel` | `Alt+Down` |
| `goto_panel_1` | — (no key) |
| `goto_panel_2` | — (no key) |
| `goto_panel_3` | — (no key) |
| `goto_panel_4` | — (no key) |
| `goto_panel_5` | — (no key) |
| `goto_panel_6` | — (no key) |
| `goto_panel_7` | — (no key) |
| `goto_panel_8` | — (no key) |
| `goto_panel_9` | — (no key) |
| `prev_project` | — (no key) |
| `next_project` | — (no key) |
| `goto_project_1` | `Alt+1` |
| `goto_project_2` | `Alt+2` |
| `goto_project_3` | `Alt+3` |
| `goto_project_4` | `Alt+4` |
| `goto_project_5` | `Alt+5` |
| `goto_project_6` | `Alt+6` |
| `goto_project_7` | `Alt+7` |
| `goto_project_8` | `Alt+8` |
| `goto_project_9` | `Alt+9` |
| `quit` | `Alt+Q` |
| `detach_instance` | `Alt+D` |
| `open_command_palette` | `Ctrl+P` |
| `open_path` | `Ctrl+G` |
| `copy` | `Ctrl+C` |
| `cut` | `Ctrl+X` |
| `paste` | `Ctrl+V` |

#### `[editor.keybindings]`

| Action | Default keys |
|---|---|
| `save` | `F2`, `Ctrl+S` |
| `save_as` | `Ctrl+Shift+S` |
| `reload` | `Ctrl+Shift+R` |
| `undo` | `Ctrl+Z` |
| `redo` | `Ctrl+Y`, `Ctrl+Shift+Z` |
| `duplicate_line` | `Ctrl+D` |
| `delete_line` | `F8` |
| `toggle_comment` | `Ctrl+/`, `Ctrl+.` |
| `search` | `Ctrl+F` |
| `search_next` | `F3` |
| `search_prev` | `Shift+F3` |
| `replace` | `Ctrl+H` |
| `replace_current` | `Ctrl+R` |
| `replace_all` | `Ctrl+Alt+R`, `Alt+R` |
| `select_all` | `Ctrl+A` |
| `trigger_completion` | `Ctrl+J`, `Ctrl+Space` |
| `show_hover` | `Ctrl+K` |
| `goto_definition` | `F12` |
| `find_references` | `Shift+F12`, `F24` |
| `rename_symbol` | `F4` |
| `code_action` | `Alt+Enter` |
| `toggle_breakpoint` | `Ctrl+F8` |
| `start_debug` | `F5` |
| `next_change` | `F7` |
| `prev_change` | `Shift+F7` |
| `stage_change` | — (no key) |
| `unstage_change` | — (no key) |
| `revert_change` | — (no key) |
| `send_to_agent` | — (no key) |
| `fold` | `Alt+,` |
| `unfold` | `Alt+.` |
| `fold_all` | — (no key) |
| `unfold_all` | — (no key) |

#### `[file_manager.keybindings]`

| Action | Default keys |
|---|---|
| `rename` | `F2`, `R` |
| `view` | `F3`, `V` |
| `edit` | `F4`, `E` |
| `copy` | `F5`, `C` |
| `move_item` | `F6`, `M` |
| `create_dir` | `F7`, `D` |
| `create_file` | `F`, `Ctrl+N`, `Shift+F4` |
| `delete` | `Delete`, `F8` |
| `delete_permanently` | `Shift+Delete`, `Shift+F8` |
| `info` | `F12`, `Space` |
| `pack` | `P`, `Alt+F5` |
| `unpack` | `Alt+F9` |
| `search` | `Ctrl+F`, `Alt+F7` |
| `search_content` | `Ctrl+Shift+F` |
| `search_replace` | `Ctrl+Shift+H` |
| `refresh` | `Ctrl+R` |
| `go_parent` | `Backspace` |
| `go_home` | `~` |
| `switch_directory` | `Ctrl+\`, `Alt+F1` |
| `go_to_path` | `Ctrl+G` |
| `toggle_selection` | `Insert` |
| `select_all` | `Ctrl+A` |
| `select_group` | `+` |
| `unselect_group` | `-` |
| `invert_selection` | `*` |
| `open_external` | `O`, `Alt+Enter` |
| `toggle_hidden` | `.` |

#### `[git_status.keybindings]`

| Action | Default keys |
|---|---|
| `stage` | `S` |
| `unstage` | `U` |
| `view` | `F3` |
| `edit` | `F4` |
| `info` | `Space`, `F12` |
| `revert` | `Backspace`, `Delete` |
| `refresh` | `Ctrl+R` |
| `toggle_folds` | `Ctrl+O` |

#### `[git_diff.keybindings]`

| Action | Default keys |
|---|---|
| `toggle_collapse` | `Enter` |
| `toggle_folds` | `Ctrl+O` |
| `edit` | `F4`, `E` |
| `refresh` | `F5`, `Ctrl+R` |
| `scroll_half_up` | `Ctrl+U` |
| `scroll_half_down` | `Ctrl+D` |
| `toggle_stage` | `Space` |
| `discard` | `D` |
| `undo_discard` | `U` |
| `select_lines` | `V` |
| `prev_hunk` | `[` |
| `next_hunk` | `]` |
| `keep_all` | `A` |
| `reject_all` | `X` |

#### `[git_log.keybindings]`

| Action | Default keys |
|---|---|
| `info` | `Space` |
| `view_diff` | `D` |
| `refresh` | `Ctrl+R` |

#### `[terminal.keybindings]`

| Action | Default keys |
|---|---|
| `scroll_up` | `Shift+PageUp` |
| `scroll_down` | `Shift+PageDown` |
| `scroll_top` | `Shift+Home` |
| `scroll_bottom` | `Shift+End` |
| `search` | `Ctrl+F` |
| `switch_directory` | `Ctrl+\` |
| `send_to_agent` | — (no key) |
| `prev_prompt` | `Ctrl+Shift+Up` |
| `next_prompt` | `Ctrl+Shift+Down` |
| `copy_command_output` | — (no key) |

#### `[database.keybindings]`

| Action | Default keys |
|---|---|
| `sort` | `S` |
| `filter` | `Ctrl+F`, `F3` |
| `clear_filter` | `Shift+F3` |
| `detail` | `Space`, `F12` |
| `copy_row` | `Ctrl+Y` |
| `refresh` | `Ctrl+R` |

#### `[viewer.keybindings]`

| Action | Default keys |
|---|---|
| `toggle_hex` | `Ctrl+L` |
| `toggle_view` | `Ctrl+E` |
| `open_external` | `O`, `Alt+Enter` |
