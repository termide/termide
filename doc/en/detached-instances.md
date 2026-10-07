# Detached Instances

A detached instance keeps running after the terminal that started it is gone.
Close the SSH connection, come back hours later, attach again — the editors,
shells, LSP servers and long-running jobs are exactly where you left them.

This is the same guarantee `tmux` and `screen` give, without a second
multiplexer between you and TermIDE: there is no prefix key competing with
TermIDE's own bindings, and no second layer to configure for colours or mouse
reporting.

> Unix only (Linux, macOS, BSD). Windows has no `fork`/`setsid`, and its
> ConPTY model needs a different host, so these flags are not available there.

## Quick start

```bash
termide --detached            # start a instance, print its id
termide --list-instances       # see what is running
termide --attach              # attach to the most recent instance
termide --attach my-project   # attach to a specific one
```

`Alt+D` detaches again, leaving everything running — as does **Options →
Detach instance** in the menu. That entry is shown only in a detachable
instance; in an ordinary one there is nothing to detach from, so it is left
out rather than shown and refused.

A instance is named after the project directory it was started in, so starting
one in `~/src/my-project` gives you `my-project`. Start a second instance in the
same directory and it becomes `my-project-2`.

## Making every instance detachable

Remembering `--detached` at launch is the whole catch: a termide started
normally cannot be detached later. A running process is bound to its
terminal's PTY — file descriptors are open, children inherited them, the
controlling terminal is assigned — and nothing can move it into another one.
This is the same reason `tmux` cannot adopt a program that is already running.

If you work this way most of the time, turn it on permanently:

```toml
[general]
always_detachable = true
```

or tick **Always detachable (Unix)** in Settings (`Alt+P`) → General. Every
`termide` then starts in a host of its own, and `Alt+D` works everywhere.

Worth knowing before you enable it:

- **Closing a terminal stops meaning "stop termide".** The instance survives,
  and so do its LSP servers, watchers and shells. That is the point over SSH,
  and a surprise locally — check `--list-instances` occasionally.
- **`$EDITOR` launches are exempt.** With file arguments (`EDITOR=termide git
  commit`) the option is ignored: git waits for the editor to exit, and a
  detach would tell it the edit finished when it had not.
- The extra PTY costs a little throughput on heavy output, the same way tmux
  does.

## A typical remote workflow

```bash
ssh server
cd ~/src/my-project
termide --detached
termide --attach
# … work, start a build, run an agent in a terminal panel …
# press Alt+D, or just close the SSH connection
```

Later, from any machine:

```bash
ssh server
termide --attach my-project
```

Closing the SSH connection without detaching is safe. The instance notices the
client is gone and carries on; the next `--attach` picks it up.

## What survives, and why

Everything. The instance is not saved and restored — it never stops.

`termide --detached` starts a small host process that owns a PTY and runs an
ordinary TermIDE inside it. Your shells, LSP servers, watchers and background
jobs are children of that TermIDE, so they are untouched by a client coming and
going. Attaching connects a terminal to the host; detaching disconnects it.

This is a different thing from the saved project layout in
`~/.local/share/termide/projects/`, which records which panels were open so a
*new* TermIDE can reopen them. That still works as before, and still applies
when you start an instance for the first time.

## Reattaching from a different terminal

You can attach from a terminal that is nothing like the one you started in — a
different size, a different emulator, a different `TERM`. On attach, TermIDE
re-negotiates the alternate screen, mouse reporting, bracketed paste and
keyboard protocol against the terminal that is now looking at it, re-detects
colour support from the client's `TERM`, and repaints in full.

Resizing the terminal while attached works normally; the layout redistributes
the way it does in a local instance.

## Commands

| Command | What it does |
|---------|--------------|
| `termide --detached` | Start a detached instance and print its id |
| `termide --detached file.rs` | Same, opening files as usual |
| `termide --detached --restore` (`-r`) | Same, reopening the projects of the last run |
| `termide --attach` | Attach to the most recent instance |
| `termide --attach <ID>` | Attach to a named instance |
| `termide --attach <ID> --force` (`-f`) | Attach, taking over from a client that is already attached |
| `termide --kill <ID>` | End an instance, with everything running inside it |
| `termide --list-instances` | List instances: id, pid, uptime, state, project |

`--list-instances` also cleans up after instances whose host process is gone, so
a crash never leaves a phantom entry behind.

With shell completion loaded (`termide --completions <shell>`, see
[Installation](installation.md#shell-completions)), Tab after `--attach`
and `--kill` offers the ids from this table.

## Detaching

| Way | When to use it |
|-----|----------------|
| `Alt+D` | The normal way. Rebind it as `detach_instance` in the `[general.keybindings]` section. |
| Close the terminal | Safe. The instance notices and keeps running. |
| `Ctrl+Z` | Does **not** work, and cannot: termide reads keys in raw mode, so the key never reaches the tty line discipline to become a SIGTSTP. `Alt+D` is the binding that does what you meant. |
| `Ctrl+\` three times | Emergency only — if TermIDE itself has stopped responding. Handled by the client, so it works even when the app does not. |

Ending a instance is the same as ending any TermIDE: quit it (`Alt+Q`) while
attached. That stops the host process too, and removes the instance.

When you cannot attach to quit it — the instance is wedged, or you simply want
it gone — end it from outside:

```bash
termide --kill my-project
```

TermIDE inside gets SIGTERM, then SIGKILL if it is still there three seconds
later; if the host process does not go either, `--kill` kills it too. Unsaved
changes in that instance are lost, and its shells and jobs end with it. A
client attached at the time is told the instance ended.

## Only one client at a time

A second `--attach` to the same instance is refused while another client is
attached, rather than mirroring the screen to both. Detach the first client (or
close its terminal) and the next attach succeeds immediately.

When the first client is out of reach — left attached on a locked desktop, or
behind an SSH connection that hung — take the instance over instead:

```bash
termide --attach my-project --force   # or -f
```

The other client is detached with a note that the instance was taken over, and
the instance repaints for the new terminal. Nothing inside it is disturbed. A
client that has stopped reading entirely is cut off without the note.

An instance started by an older TermIDE does not understand `--force`: detach
its client the ordinary way, or end it with `--kill`.

## Where the instance state lives

Sockets live in `$XDG_RUNTIME_DIR/termide/` on Linux and BSD, and in
`~/Library/Application Support/termide/run/` on macOS, which has no
`XDG_RUNTIME_DIR`. The directory is owner-only (`0700`), so no other account on
the machine can attach to your instances.

Nothing there needs cleaning up by hand: a socket outlives its host only until
the next `--list-instances` or `--detached`, which prunes it.
