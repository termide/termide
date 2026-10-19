# Screenshots

Reproducible screenshots and GIFs of termide for the README and the website,
recorded with [VHS](https://github.com/charmbracelet/vhs) inside a container
against a synthetic project.

```sh
tools/screenshots/run.sh              # build the image, render every tape
tools/screenshots/run.sh agent hex    # render only these tapes
SKIP_BUILD=1 tools/screenshots/run.sh overview   # reuse the last image
```

Output lands in `tools/screenshots/out/` (ignored by git), and
`out/site/` holds the same shots under the names the website uses:
`screenshots/*.png|gif`, `themes/<theme-id>.png` for every built-in theme
and `og.png` (1200x630). No image is committed here: `sync-shots.sh` in
the website repository (termide.github.io) copies `out/site/` into its
`assets/`, and the README and docs link them from there. The first build
compiles termide from this checkout and takes several minutes; later builds reuse BuildKit's cargo cache. A full
run takes about 20 minutes, most of it the theme gallery. Requires Docker
with BuildKit.

`run-ghostty.sh` is an optional second route for what VHS cannot draw: the
image viewer needs the Kitty graphics protocol, which xterm.js lacks. It
opens Ghostty (with its user configuration ignored) running the same
isolated container; a pty driver inside (`ghostty/pty-driver.py`) plays a
key script and signals each shot, and the script captures only that window
with `screencapture -l`. It needs the Screen Recording permission for the
terminal app that runs it, and says so when it is missing.

## What is isolated

Nothing from the machine running the recording can reach an image:

- **Only the sources go in.** The build context is filtered by
  `Dockerfile.dockerignore` to the crates, the manifests and this folder's
  scripts. No home directory, configuration, shell history or git identity
  is copied.
- **Only the output comes out.** At run time the container sees the scripts
  (read-only) and `out/`, nothing else. `/tmp` is a private tmpfs.
- **No network.** Recordings run with `--network none`; the agent talks to a
  mock model on loopback (`mock-llm/server.py`) that replays a fixed script.
- **Invented identity.** User `demo`, host `demo`, prompt
  `demo@demo:/tmp/demo/inventory$`, git author `Demo User <demo@example.com>`,
  `TZ=UTC`, `LANG=en_US.UTF-8`, a fresh termide configuration
  (`env/config.toml`).
- **Pinned clock.** libfaketime sets the date to Sunday 2026-09-27 10:42 UTC
  for termide, git and the shell, so the clock, the calendar, git's relative
  dates and file times read the same on every run. (This is why the image
  builds termide against glibc: a static musl binary ignores `LD_PRELOAD`.)
- **Invented project.** `fixtures/make-demo.sh` builds `/tmp/demo/inventory`
  from scratch on every tape: a small Python service with a git history on
  fixed dates, a branch with its own worktree, uncommitted changes, a SQLite
  database, Markdown with Mermaid, and a binary file.

Some readings still come from the machine that runs Docker, because the
container shares its kernel: the **RAM total** in the menu bar (the Docker
VM's memory) and live CPU/network figures. The monitor tape runs termide in
a PID namespace of its own (`env/demo-ns.sh`) next to two demo services, so
its process and network lists show `inventory-api` and `stock-worker`, not
the recorder; the disk modal is never opened, since it names the VM's disk.

## Writing a tape

Each `tapes/*.tape` is one shot; files starting with `_` are shared steps:
`_settings.tape` (geometry, font), `_start.tape` (hidden setup, termide in
the project), `_widen.tape` (move the panel just opened into a wide
column) and `_next-scene.tape` (quit and restart termide on a fresh
project out of frame). `tour.tape` chains the single-shot tapes into the
README's hero GIF, `tour.gif` on the website. VHS cannot send `Alt` with a digit or a special key, nor function
keys, so `env/config.toml` binds the layout actions the tapes need to free
`Alt+<letter>` chords; `Alt+m` also trips VHS's parser, so the menu is
opened with `Alt+M`. Viewers are opened from the file manager (`v` views,
`Enter` opens), since `termide <file>` always starts the text editor.
The debug tape needs `F5` and `Ctrl+F8` too: it starts debugging from the command palette, sets the breakpoint with `Alt+y` and unstacks the debug panel with `Ctrl+b`, both bound in `env/config.toml`, and debugs a script and a debug command that `env/debug-scene.sh` adds to the project (debugpy comes from Debian's `python3-debugpy`). Mouse clicks are out of VHS's reach.
The resource modals are reached with `Alt+M` then `Left` past the disk
indicator and through the calendar, whose weekday the pinned date fixes.
`# hint:` comments in a tape are key badges laid over its GIF after the
recording, by `keys.mjs` on this machine (Node, ffmpeg, Chrome): `# hint:
Ctrl+F8 "breakpoint" for=1.4s color=#ffd166 bg=#3a1020` — keys and quoted
text, then optionally how long it stays, its colour and background. They
name what a viewer would press, not the stand-in a tape sends: `debug.tape`
shows `Ctrl+F8` where it sends `Alt+y`. The GIF with the badges replaces the
plain one, and an MP4 of it lands beside it.
The theme gallery tape is generated from `crates/theme/themes` by
`gen-themes.sh`.
