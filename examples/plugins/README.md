# Example plugins

Plugins for termide written with `sdk/termide_plugin.py`, a one-file Python
library (standard library only, Python 3.8+) that speaks the protocol of
[doc/en/plugins.md](../../doc/en/plugins.md) for you.

| Plugin | What it shows |
|---|---|
| `trim-on-save` | `file/will_save`: edits made before `Ctrl+S` writes the file |
| `todo-lint` | events, marks in the editor, a fix, a command with a key, a panel with buttons |
| `word-count` | events and a status bar item |

To install one, copy its directory into `plugins/` beside your `config.toml`
(`~/.config/termide/plugins/` on Linux, `~/Library/Application
Support/termide/plugins/` on macOS) and run **Plugins: read the plugins
directory again** from the command palette.

Each example carries its own copy of `termide_plugin.py`, so the directory
works on its own; a test checks the copies against `sdk/`. To write a plugin
of your own, copy `sdk/termide_plugin.py` beside a `run.py` like these:

```python
from termide_plugin import Plugin

plugin = Plugin()

@plugin.command("hello", "Say hello", key="Ctrl+Alt+H")
def hello():
    plugin.notify("Hello from " + plugin.name)

@plugin.on("file/saved")
def saved(params):
    plugin.status("saved", "saved " + params["path"].rsplit("/", 1)[-1])

plugin.run()
```

Handlers run on a pool of threads, so one may ask termide (`plugin.read`,
`plugin.confirm`, …) and wait for the answer. Columns are UTF-16 code units,
as in LSP: `utf16_len(text)` gives them.
