# Plugin design notes

Working notes for the plugin system. Status: implemented, phases as in §5;
`doc/en/plugins.md` describes what the code does.

## 1. What a plugin is

A plugin is a program that termide starts in the background and talks to over
an agreed protocol (section 4). It runs as its own process. Termide does not
load it into its own address space and does not embed a script engine.

## 2. Trust boundary

A plugin is code the user installed, so it is trusted as much as any binary on
`$PATH`, and termide does not sandbox it from the OS. The guarantee is narrower:
**a project never decides what code runs.** An opened repository can neither
add, replace nor locate a plugin.

- **Code lives only in the global directory**:
  `plugins/<name>/` beside the global `config.toml` (`~/.config/termide` on
  Linux, `~/Library/Application Support/termide` on macOS) holds `plugin.toml` (the manifest) and
  the executable. Installation means copying the directory there.
- **The manifest is read only from there.** The project layer never reads
  `plugin.toml` or any file in that directory.
- **The command is resolved against the plugin directory**, as an absolute path
  or one relative to `plugins/<name>/`; a bare name (`python3`) is looked up
  only in the absolute entries of termide's `PATH`. It is never looked up
  through the project's cwd or a relative `PATH` entry, so a planted
  `./plugin-bin` in a repository cannot stand in for it.
- **The process starts with cwd set to the plugin directory.** It learns the
  project root from `initialize`, not from its working directory.
- **Config holds only switches and data.** The global `config.toml` sets each
  plugin's mode, and the project `.termide/config.toml` may switch it within
  that mode. Both may hold plugin settings (plain values). Neither may hold a
  command, a path to an executable or anything else that names code.
  `Config::load_layers` must drop such keys from the project layer and report
  them in `warnings`, because the project layer is otherwise overlaid on the
  global config without a filter.

### Enabling

The global config gives each installed plugin a mode, and the mode bounds what
a project may do:

```toml
# ~/.config/termide/config.toml
[plugins.formatter]
mode = "on"       # default: runs everywhere, a project may turn it off
[plugins.deploy]
mode = "opt-in"   # sleeps until a project turns it on
[plugins.experiment]
mode = "off"      # off everywhere, no project can turn it on

# <project>/.termide/config.toml
[plugins.deploy]
enabled = true
[plugins.formatter]
enabled = false
```

| Global `mode` | Project says nothing | `enabled = false` | `enabled = true` |
|---|---|---|---|
| `on` (or no entry) | on | off | on |
| `opt-in` | off | off | on |
| `off` | off | off | off |

- **Only the user's global config decides** whether a project may wake a
  plugin. A repository can enable only what the user marked `opt-in`, and can
  never touch `off`.
- **A project's switch affects that project only.** Project A can use a plugin
  that project B turns off.
- **Each key is read from its own layer.** `mode` comes from `global_layer`, and
  a `mode` in the project file is dropped with a warning. `enabled` comes from
  the project file, and an `enabled` in the global file is ignored with a
  warning. Both layers are in `LayeredConfig`, so the generic merge and the
  project file's diff-saving stay untouched.
- **Unknown names** in either layer, naming no installed plugin, are ignored.
  (A warning for them was planned and is not implemented.)
- **Use `opt-in` for plugins that act on project content**, such as one that
  runs project scripts on save. Then no repository turns them on unasked.

## 3. Lifecycle

- **Instances:** one process per plugin per open project. An instance can
  hold several open projects, and a parked one keeps running (terminals,
  agents), so its plugins keep running too. A process starts when its project
  opens (a project restored from the last run waits until it is first loaded)
  and stops when the project closes or termide quits. Each project resolves
  modes against its own config. There is no shared daemon, so state never
  crosses projects. A detached instance keeps its plugins in the server half.
- **Startup:** termide sends `initialize` (protocol version, project root,
  termide version, the plugin's settings). The plugin answers with its
  capabilities: the events it subscribes to and the commands, status items and
  agent tools it contributes.
- **Crashes:** a crashed plugin is restarted with backoff. After N crashes in a
  window it is disabled for the session and the user is notified. A broken
  plugin never blocks termide, the same rule that agent hooks follow.
- **Exit:** termide sends `shutdown`, waits for a timeout, then kills the
  process.
- **Config change:** when a project's config is saved, its plugin set is
  resolved again, and plugins are started or stopped to match.

## 4. Protocol

JSON-RPC 2.0 over stdio with newline-delimited messages, as MCP and ACP use.
The MCP client's transport cannot carry requests from the other side, so the
plugin transport is a peer of its own in the same style. Stderr goes to the
log.

termide → plugin, as notifications, only for events the plugin subscribed to:
`file/opened`, `file/saved`, `git/changed`, agent events. A plugin receives
only the events of its own project.

plugin → termide, as requests: notify, open file, read buffer, apply edit, run
a command in a terminal panel, set a status item.

Contributions, declared in `initialize` (they can change later):

- commands for the palette and the Commands menu, which termide invokes as
  `command/execute`;
- agent tools, which the agent calls as `tool/call` over the same connection.
  The usual permission rules and hooks apply to them as to built-in tools.

## 5. Phases

1. Manifest, enable switches with the project-layer filter, lifecycle,
   protocol core, events (`file/opened`, `file/saved`), commands,
   notifications, status items, agent tools. Done; see `plugins.md` for the
   protocol as built. The agent announces a plugin's tools as it does an MCP
   server's, named `plugin <name>`.
2. Custom panels. Done: the plugin sends Markdown, termide renders it in a
   Markdown viewer marked as the plugin's, and a `plugin:<action>` link is
   handed back as `panel/action`. Markdown was chosen over a model of our own
   because the viewer already lays out headings, lists, tables, code and
   links, with the cursor, selection, search and copy, and every plugin
   author knows it.
   Windows (`window/confirm`, `choose`, `input`, `form`, `message`) are
   termide's own modals: the plugin picks the kind and the words, termide
   heads every one `🧩 <plugin>`, offers no hidden input, queues them one per
   plugin and answers the request when the window closes. What tells who
   speaks — a panel's icon and the plugin name leading its title, a window's
   heading — is never the plugin's to set.
3. Editor decorations. Done as diagnostics: `editor/diagnostics` sets the
   plugin's marks on a file, kept apart from the language servers' (which
   replace their own set at each publish) and shown together with them in
   the editor and the diagnostics panel, named after the plugin. They reuse
   the gutter, underline, message rows and panel the editor already has.

4. The editor and the project. Done: `editor/active`, `read`, `edit`, `open`
   on the editors of the plugin's own project, an edit applied to the open
   buffer only and as one undo step (`TextBuffer::begin_undo_group`), and
   refused as `stale` against a version; `terminal/run` in a visible
   terminal; events `file/closed`, `editor/focused`, `editor/changed`
   (watched only while asked for, told after 300 ms of quiet) and
   `git/changed`; command keys limited to strong chords nothing else binds;
   `$/cancelRequest` for what termide gives up on; `tool/progress` routed on
   the connection's thread; the palette's status, reload and restart; the
   plugin's whole process group ended with it; 256 messages per tick at most.

5. Saving and the rest. Done: `file/will_save` held behind an editor's save
   gate and answered per plugin in turn off the UI thread; `agent/started`
   and `agent/finished` from a typed `PanelEvent::AgentRun`; a Plugins tab
   in the settings writing the mode globally and the switch in the project,
   the split the loader enforces.

6. Language features. Done: plugins as extra sources beside the language
   servers (`termide_lsp::extra`): the manager asks them with the server and
   merges the answers on a thread, a plugin that has not answered in 1.5 s
   left out; with no extra source nothing changes. An editor keeps its
   language features on without a server while extra sources exist. A
   plugin's code action edits the file asked about only and is titled with
   its name.

Out of scope: an install command (copying the directory is enough), declared
capability permissions, and a shared daemon.
