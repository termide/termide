# Plugins

A plugin is a program that termide runs in the background while a project is
open and talks to over JSON-RPC on the program's stdin and stdout. It hears
what happens in the project's editors and repository, reads and edits the open
buffers, adds commands with keys of their own, marks lines in the editor,
shows items in the status bar, asks in termide's windows, opens panels of its
own and terminals, and gives the coding agent tools.

Plugins are global: they are installed in your config directory, and a project
can only switch off a plugin you installed or switch on one you allowed it to.
An opened repository never decides which program runs.

## Installing

Copy the plugin's directory into `plugins/`, beside your `config.toml`:

| Platform | Directory |
|---|---|
| Linux | `~/.config/termide/plugins/<name>/` |
| macOS | `~/Library/Application Support/termide/plugins/<name>/` |
| Windows | `%APPDATA%\termide\plugins\<name>\` |

The directory's name is the plugin's name: letters, digits, `-` and `_`. It
holds a `plugin.toml`:

```toml
description = "Formats on save"
command = "./run.py"          # relative to this directory
args = ["--quiet"]
[env]
LOG_LEVEL = "info"
```

`command` is either an absolute path, a path relative to the plugin directory
(it contains a `/`), or a bare name such as `python3`. A bare name is looked up
only in the absolute entries of `PATH`. The plugin runs with its own directory
as the working directory and learns the project's root from `initialize`.

Plugins are read when termide starts. After installing, removing or changing
one, **Plugins: read the plugins directory again** in the command palette
(`Ctrl+P`) takes it in without a restart: every plugin starts afresh, and the
language features of one installed since reach the editors opened after it.
**Plugins: show where each stands**
lists the installed plugins and their state in the current project, and
**Plugins: restart <name>** starts one afresh.

## Turning plugins on and off

The global `config.toml` sets each plugin's mode:

```toml
[plugins.formatter]
mode = "on"       # the default: runs in every project, a project may turn it off
[plugins.deploy]
mode = "opt-in"   # runs only in projects that turn it on
[plugins.experiment]
mode = "off"      # runs nowhere

[plugins.formatter.settings]
line_width = 100  # handed to the plugin in `initialize`
```

A project's `.termide/config.toml` may switch a plugin within its mode, and add
to its settings:

```toml
[plugins.deploy]
enabled = true
[plugins.formatter]
enabled = false
```

| Global `mode` | Project says nothing | `enabled = false` | `enabled = true` |
|---|---|---|---|
| `on` (or no entry) | runs | off | runs |
| `opt-in` | off | off | runs |
| `off` | off | off | off |

The **Plugins** tab of the settings (`Alt+P`) lists the installed plugins:
stored globally, `Enter` on one steps its mode; stored in the project, the
project's switch.

A `mode` in a project file, and an `enabled` in the global file, are ignored
with a warning in the log. Give `opt-in` to plugins that act on what is in a
project, such as one that runs the project's scripts, so that no repository
turns them on without your consent.

Each open project has its own copy of each plugin it runs, including projects
parked in the background. A plugin that crashes is restarted after 1, 2, 4…
seconds. After three crashes within a minute it stays off until its settings
change, it is restarted, the plugins directory is read again or termide
restarts, and its project's status bar says so.

## Writing a plugin

Each message is one line of JSON-RPC 2.0. Anything a plugin writes to stderr
goes to termide's log. In Python, `examples/plugins/sdk/termide_plugin.py` in
the repository speaks it for you, and `examples/plugins/` holds plugins built
with it.

**termide → plugin**

| Method | Kind | Params | Result |
|---|---|---|---|
| `initialize` | request | `protocolVersion` (1), `termide.version`, `plugin.name`, `plugin.dir`, `project.root`, `settings` | what the plugin contributes, see below |
| `initialized` | notification | | |
| `command/execute` | request | `id` | anything |
| `tool/call` | request | `name`, `arguments`, `progress` | `text`, `isError` |
| `$/cancelRequest` | notification | `id` of a request termide gave up on | |
| `shutdown` | request | | `null` |
| `exit` | notification | | |

The events a plugin subscribes to, notifications about its own project:

| Event | Params | When |
|---|---|---|
| `file/opened` | `path` | a file opened in an editor |
| `file/saved` | `path` | an editor wrote its file |
| `file/closed` | `path` | an editor of the current project closed |
| `editor/focused` | `path` | another file's editor became the active panel |
| `editor/changed` | `path`, `version` | a buffer changed, once the typing paused for 300 ms |
| `git/changed` | `root` | a repository's index, HEAD or refs moved: a commit, a checkout, a stage |
| `agent/started`, `agent/finished` | `panel`, and `ok` when it finished | an agent panel's run started or ended |

The events of the editors are watched only while a plugin of the current
project has asked for them.

`file/will_save` is a request rather than an event: a plugin that subscribes
to it is asked `{ "path", "version" }` when `Ctrl+S` is pressed in an editor
of its project, before the file is written, and may answer `{ "edits": [...] }`
(as in `editor/edit`) or `null` — formatting on save. termide asks each such
plugin in turn, three seconds each, applies the edits to the buffer if it has
not changed meanwhile, then writes the file. Saving on close and Save As do
not ask.

The result of `initialize`, and the params of a `contributions/changed`
notification sent later to replace it:

```json
{
  "subscriptions": ["file/saved"],
  "commands": [{ "id": "check", "title": "Check the project", "key": "Ctrl+Alt+K" }],
  "tools": [{
    "name": "lookup",
    "description": "Look a word up in the project glossary",
    "inputSchema": { "type": "object", "properties": { "word": { "type": "string" } } },
    "readOnly": true
  }]
}
```

Commands appear as `<plugin>: <title>` in the command palette (`Plugins`
category) and at the end of the Commands menu. A command's `key` runs it from
anywhere, if termide binds that key nowhere — the user's own commands
included — and it is `Ctrl` or `Alt` with another modifier (`Ctrl+Alt+K`,
`Ctrl+Shift+K`, `Alt+Shift+K`) or an F-key with one (`Shift+F7`): keys a
program in the terminal rarely wants. A key that does not qualify is left
unbound, with a line in the log.

Tools are offered to the agent sessions of the plugin's project as
`plugin-<plugin>__<tool>`. The agent's permission rules treat them as MCP
tools: they are asked about before running unless they are `readOnly`. While
a tool runs, the plugin may send `tool/progress` `{ "token", "text" }` with
the `progress` token of the call: the output so far, whole, shown in the
transcript as it comes. A call the user stops, or one past its ten minutes,
is followed by `$/cancelRequest`, as is a command past its five.

**plugin → termide**

| Method | Kind | Params |
|---|---|---|
| `contributions/changed` | notification | as the `initialize` result |
| `window/notify` | request | `message`, `level` (`"info"` or `"error"`) |
| `status/set` | request | `id`, `text` (an empty text removes the item) |

A plugin of a project in the background is not shown on screen: its
notifications go to the log. termide takes at most 256 messages from all
plugins per screen refresh; the rest wait for the next.

### Completion, hover and fixes

A plugin can answer in the editor as a language server does, beside the
servers or for a language that has none. It names the languages, by LSP id
(`rust`, `python`, … — `*` for all), in `providers` among its contributions:

```json
"providers": { "completion": ["python"], "hover": ["*"], "codeActions": ["python"],
               "definition": ["python"], "references": ["python"] }
```

| Request | Params | Answer |
|---|---|---|
| `editor/completion` | `path`, `line`, `col` | `{ "items": [...] }`, LSP `CompletionItem`s (`label`, `insertText`, `detail`, `kind`, …) |
| `editor/hover` | `path`, `line`, `col` | `{ "markdown" }` |
| `editor/code_actions` | `path`, `line`, `col`, `end_line`, `end_col`, `diagnostics` (`line`, `col`, `end_line`, `end_col`, `message`, `source`) | `{ "actions": [{ "title", "edits" }] }`, edits as in `editor/edit` |
| `editor/definition`, `editor/references` | `path`, `line`, `col` | `{ "locations": [{ "path", "line", "col", "end_line"?, "end_col"? }] }`, paths absolute |

termide asks the plugins of the file's project with the server and shows one
answer: the completion items together, the hovers one under the other, the
fixes in one list, the places to go to (`F12`) and the references together. A
plugin has 1.5 seconds; one that answers later is left
out of that answer. A fix applies to the file asked about and nothing else,
in its open buffer, and its title starts with the plugin's name, so it is
clear who suggests it. At most 500 items, 50 fixes and 20 000 characters of
hover are taken from a plugin.

### The editor

A plugin reaches the editors of its own project, and only those:

| Method | Params | Result |
|---|---|---|
| `editor/active` | | `path`, `line`, `col`, `selection` (`{ line, col, end_line, end_col }` or `null`), `modified`, `version` of the active editor; `null` without one, or when the plugin's project is in the background |
| `editor/read` | `path` | `text`, `version`, `modified` of the open buffer, unsaved changes included |
| `editor/edit` | `path`, `edits`: `[{ "line", "col"?, "end_line"?, "end_col"?, "text" }]`, `version` (optional) | `{ "version" }` |
| `editor/open` | `path`, `line`, `col` (optional) | `null`; opens the file in the current project |

Positions count as LSP counts them: lines from 0, columns in UTF-16 code
units; an edit without an end inserts. `editor/edit` changes the open buffer
only — termide writes no file for a plugin, which can write files itself —
and the edits are one undo step, so `Ctrl+Z` takes them back. With `version`
the edit is refused as `stale` if the buffer changed since that version (as
`editor/read` or `editor/changed` reported it), so a plugin that computed
edits from the text does not apply them to a different one. A buffer of an
editor that only views its file is refused too. `editor/read` hands over at
most 16 MB, `editor/edit` at most 10 000 edits at once.

### Terminals

`terminal/run` `{ "command", "cwd" }` opens a terminal panel in the plugin's
project, which must be the current one, and types `command` in it, so the
user sees what runs; `cwd` (absolute) defaults to the project's root. The
command is one line with no control characters, and an error comes back
when no terminal could be opened.

### Marks in the editor

A plugin marks lines of files as a language server does, with
`editor/diagnostics` (request) `{ "path", "items" }`: the marks show in the
editor's gutter and under the text, their messages on the lines below, and in
the diagnostics panel, beside the language servers' and named after the
plugin. Each call replaces the plugin's marks on that file; an empty `items`
clears them. `path` is absolute.

```json
{ "path": "/home/me/proj/src/main.rs", "items": [
  { "line": 11, "message": "TODO left behind", "severity": "info" },
  { "line": 40, "col": 8, "end_col": 15, "severity": "error",
    "message": "unsafe call", "code": "S101" }
] }
```

Lines count from 0 and columns as LSP counts them (UTF-16 code units). Without
`col` the whole line is marked; with `col` alone, the word there. `severity`
is `error`, `warning` (the default), `info` or `hint`. A file opened later
shows the marks set before; a plugin's marks go when it stops.

### Windows

A plugin can ask the user something in one of termide's own windows. The
request is answered when the user closes the window: with the answer, or
`null` when it was dismissed (`Esc`).

| Method | Params | Answer |
|---|---|---|
| `window/confirm` | `title`, `message` | `true`, `false` or `null` |
| `window/choose` | `title`, `items` (strings) | the index picked, or `null` |
| `window/input` | `title`, `prompt`, `default` (optional) | the text, or `null` |
| `window/form` | `title`, `fields`: `[{ "name", "label", "type", "options"?, "default"? }]`, `type` one of `text`, `number`, `bool`, `select` | `{ name: value }` (switches as booleans, numbers as numbers), or `null` |
| `window/message` | `title`, `message` | `null` |

Every window is headed `🧩 <plugin> · <title>` by termide, so that it never
passes for termide's own questions; the plugin's words are cleaned of control
and invisible characters. There is no hidden input: a window asking for a
password is what a phishing plugin would show, so a secret belongs in the
plugin's `settings` or its environment. A window waits behind any other one
on screen and shows only while its plugin's project is current; a plugin has
one window at a time, and asking for another meanwhile is refused as busy.
When the plugin stops or its project closes, its windows go.

### Panels

A plugin can open panels of its own, drawn from Markdown it sends: headings,
lists, tables, code and links as in the Markdown viewer, with its cursor,
selection, search and copy. A link to `plugin:<action>` is a button: following
it (`Enter` on it, or a click) sends the action back to the plugin, which
answers by updating the panel.

| Method | Kind | Direction | Params / result |
|---|---|---|---|
| `panel/open` | request | plugin → termide | `title`, `markdown`, `menu` (optional); the result is `{ "panel" }`, its id |
| `panel/update` | request | plugin → termide | `panel`, `markdown`, `title` and `menu` (optional) |
| `panel/close` | request | plugin → termide | `panel` |
| `panel/action` | notification | termide → plugin | `panel`, `action` |
| `panel/closed` | notification | termide → plugin | `panel`: the user closed it |

`menu` is the panel's own entries of its `[≡]` menu, `[{ "label",
"action" }]` (at most 12): a chosen one is sent as `panel/action`, as a
`plugin:` link is. What shows who drew the panel is termide's: its icon is
always 🧩 and its title always starts with the plugin's name (`deploy:
Report`); the plugin's title and labels are cleaned of control and invisible
characters and cut to length.

A panel opens in the plugin's project, which must be the current one; it is
not saved with the layout. An update keeps the reader's place as far as the
new content reaches. When the plugin stops, its panels stay and say so; a
plugin started again does not get them back.

```python
# in the loop of the example below
elif method == "command/execute":            # a command "Show report"
    send({"jsonrpc": "2.0", "id": id, "result": None})
    send({"jsonrpc": "2.0", "id": "open", "method": "panel/open",
          "params": {"title": "Report", "markdown": "# Report\n\n[Refresh](plugin:refresh)"}})
elif msg.get("id") == "open":                # the answer to panel/open
    panel = msg["result"]["panel"]
elif method == "panel/action" and msg["params"]["action"] == "refresh":
    send({"jsonrpc": "2.0", "id": "upd", "method": "panel/update",
          "params": {"panel": panel, "markdown": "# Report\n\nUpdated.\n\n[Refresh](plugin:refresh)"}})
```

### Example

A Python plugin that counts saves in the status bar:

```python
#!/usr/bin/env python3
import json, sys

def send(message):
    print(json.dumps(message), flush=True)

saves = 0
for line in sys.stdin:
    msg = json.loads(line)
    method, id = msg.get("method"), msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id, "result": {
            "subscriptions": ["file/saved"],
            "commands": [{"id": "reset", "title": "Reset the save count"}]}})
    elif method == "file/saved":
        saves += 1
        send({"jsonrpc": "2.0", "id": f"s{saves}", "method": "status/set",
              "params": {"id": "saves", "text": f"saved {saves}×"}})
    elif method == "command/execute":
        saves = 0
        send({"jsonrpc": "2.0", "id": id, "result": None})
        send({"jsonrpc": "2.0", "id": "r", "method": "status/set",
              "params": {"id": "saves", "text": ""}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": id, "result": None})
    elif method == "exit":
        break
```

Save it as `plugins/save-count/run.py`, make it executable, and add
`plugins/save-count/plugin.toml` with `command = "./run.py"`.
