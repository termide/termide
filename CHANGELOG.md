# Changelog

All notable changes to TermIDE will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- **Held-key auto-repeat on Kitty-protocol terminals.** When the terminal supports the Kitty keyboard protocol, termide enables `REPORT_EVENT_TYPES`, so a held key streams `Repeat` events. The event handler previously discarded `Repeat` along with `Release`. `Repeat` is now treated like `Press`, restoring auto-repeat.

## [0.24.0] - 2026-06-07

### Added
- **Database viewer (read-only).** Browse **SQLite**, **PostgreSQL** and **MySQL/MariaDB** tables from a bookmark whose path is a database URL (`sqlite:///…`, `postgres://…`, `mysql://…`). The panel connects in the background and shows rows in a 2D grid with a cell cursor, sliding-window pagination, single-column server-side sort (ascending → descending → unsorted), type-aware per-column filtering (combined with `AND`), a full-row detail dialog (copy as TSV / JSON / INSERT), and cell/row copy. When the URL omits a database (PostgreSQL/MySQL) a selector lets you pick one (the first is auto-selected); the panel is restored across sessions. Queries run on a background runtime so the UI never blocks; everything is strictly read-only (`SELECT`/catalog queries only). Action keys are configurable under `[database.keybindings]` and shown in Help; the mouse can open the selectors, sort by clicking a column header, and move the cursor by clicking a cell. See [Database Viewer](doc/en/database.md).
- **Android / Termux support.** Releases now include a statically-linked `aarch64-unknown-linux-musl` binary that runs in Termux; see the installation docs.

### Fixed
- The Operations panel no longer offers **Pause** for script/command operations (it was never actually implementable); **Cancel** still stops them by killing the process. Pause/Resume remain for file transfers.

## [0.23.11] - 2026-06-05

### Added
- **Multiple Git Status panels.** Opening Git Status (`Alt+G` / Tools menu) now always creates a new panel instead of focusing the existing one, so several repositories can be watched side by side — each panel tracks its own repo via the dropdown.

### Security
- The SSH askpass helper now hands the passphrase to ssh via a raw stdout write rather than a `print!` macro. Behaviour is unchanged (ssh reads it over a pipe it owns — never a terminal or log); this just keeps static analysis from mistaking the askpass contract for cleartext logging.

## [0.23.10] - 2026-06-05

### Added
- **Git Status/Log now find nested repositories.** Opening termide in a directory that isn't a repo itself but contains git projects (or sits inside a whole-home/whole-disk repo) now lists those nested projects, discovered asynchronously up to two levels deep — alongside the usual submodule discovery.

### Changed
- The repository dropdown is sorted by name (case-insensitive) instead of full path, so the list reads alphabetically.

### Fixed
- The Git Status/Log panels no longer keep showing a repository's branch, files, or commits after it disappears (e.g. its `.git` was deleted) — the panel state is cleared, and it reloads when the selected repository changes.
- Navigating no longer briefly flashes "no repositories" while the repo list is re-scanned.

## [0.23.9] - 2026-06-03

### Fixed
- **SSH/credential prompts no longer corrupt the interface.** Opening a repo auto-fetches, and push/pull/fetch ran with the terminal attached, so when ssh needed an SSH key passphrase it drew the prompt straight over the TUI. Network git operations now run non-interactively (detached stdin, `GIT_TERMINAL_PROMPT=0`, ssh `BatchMode`), so a key already in ssh-agent just works and a missing one fails cleanly. A failed auto-fetch reports on the status line instead of popping a modal.

### Added
- **Enter an SSH key passphrase in a modal for git push/pull/fetch.** When an operation needs a passphrase that isn't in ssh-agent, termide now asks for it in a masked modal and retries (via a private, short-lived askpass handoff) instead of failing. The passphrase is cached in memory for the session so repeated operations don't re-prompt; it is never written to the config or persisted.

## [0.23.8] - 2026-06-03

### Fixed
- **Crash when opening a dropdown on a short or narrow terminal.** Theme/menu/language/git-selector popups sized themselves from their item count without clamping to the terminal, so a list taller or wider than the screen rendered past the buffer edge and panicked (e.g. opening Options → Themes on a 24-row terminal, or a git branch/stash selector on a small window). Popups are now clamped to the visible area and scroll instead.

## [0.23.7] - 2026-06-02

### Added
- **Open files from the command line / use termide as `$EDITOR`.** `termide path/to/file ...` now opens the given files (creating them if absent), so termide works as the editor for `git commit`, `crontab -e`, `visudo`, and similar tools that previously failed with "unexpected argument". When launched with file arguments termide starts in a lightweight editor mode: it shows only those files, does not restore or save the project session (so editing a commit message can't clobber it), and closing the last panel quits — returning control to the launching tool like nano or vim.

## [0.23.6] - 2026-06-02

### Fixed
- **"Import class" no longer deletes code or loses unsaved edits.** A command-based quick-fix's edit is computed by the language server against the document it tracks — the editor buffer — but it was being applied to the file on disk and the editor reloaded from there. With unsaved changes in the buffer (e.g. a just-typed `Order::where()`), disk and buffer diverged: the reload discarded the unsaved text and the edit landed on the wrong lines. Server-driven edits to an open file now go through its buffer, preserving unsaved work and landing where the server intended; the server is also synced before and after so repeated runs don't duplicate the import.

## [0.23.5] - 2026-06-02

### Fixed
- **"Import class" no longer mangles the file or duplicates the import.** The command-based quick-fix added in 0.23.4 could garble text around the insertion point — servers such as phpactor send the import as two edits at the same position, which the edit-application code mis-ordered — and running it a second time added a duplicate `use` line because the server was never told the file had changed. Edits are now applied by absolute document offset (every other line is preserved verbatim), and the language server is resynced after each applied edit.

## [0.23.4] - 2026-06-02

### Fixed
- **Command-based quick-fixes now apply.** Actions a language server performs through a command rather than an inline edit — such as phpactor's "Import class" — showed up in the `Alt+Enter` popup but did nothing when chosen. They now run via `workspace/executeCommand`, and the edit the server pushes back is applied (adding the `use` statement and reloading any affected editors). This completes the code-action support started in 0.23.2–0.23.3.
- The code-action shortcut (`Alt+Enter`) is now listed in the help panel alongside the other LSP shortcuts, and the documented manual-completion shortcut was corrected to `Ctrl+J` / `Ctrl+Space`.

## [0.23.3] - 2026-06-01

### Fixed
- **Code actions from servers that fill in the edit lazily now work.** Quick-fixes that a language server returns without an inline edit (resolving it only when chosen) previously never appeared in the `Alt+Enter` popup; they are now listed and resolved on accept, so more servers' actions (e.g. phpactor "Import class") apply.

## [0.23.2] - 2026-06-01

### Added
- **LSP code actions** — press `Alt+Enter` on a line to request quick-fixes (e.g. "Import class" for PHP) and pick one from a popup; the chosen edit is applied across files with open editors reloaded. (`Ctrl+.` stays bound to toggle-comment, so the default is `Alt+Enter`.)

### Changed
- Auto-completion now triggers on the characters the language server actually advertises (so PHP's `->` opens the popup), falling back to the built-in set only when the server reports none.

### Fixed
- **Completion no longer duplicates the typed prefix.** Accepting a suggestion now applies the server's edit verbatim instead of a word-boundary heuristic, so `$va`→`$var` (not `$$var`) and `Ord`→`Order` (not `OrdOrder`). Imports that accompany a completion (`use` statements) are applied too.

## [0.23.1] - 2026-05-31

### Added
- The version string (`termide --version` and the help panel header) now includes the git commit it was built from, e.g. `0.23.1 (50b81b1)`, so builds that share a version number can be told apart.

### Fixed
- **PHP syntax highlighting** — `.php` files are now highlighted instead of shown as plain text, including mixed HTML/PHP templates (the HTML, the `<?php … ?>` code, and the tags are all coloured). The PHP grammar had been silently disabled in the shipped binary: different crates pulled incompatible tree-sitter-php versions, whose clashing `tree_sitter_php` symbols collided at link time onto an ABI the runtime rejected. All crates are now pinned to the same grammar version.
- **JSX syntax highlighting** — `.jsx` files are now highlighted; the language was listed as supported but never actually loaded.
- **Multi-line constructs** — block comments and strings that span several lines now stay coloured to their end across every language, instead of losing highlighting after the first line.

### Changed
- The editor now highlights the whole buffer in one context-aware pass (files up to 1 MB) rather than line by line, which is what makes template languages and multi-line tokens colour correctly. Larger files fall back to the previous per-line highlighting.
- A syntax grammar that fails to load is now logged instead of being dropped silently, so missing highlighting is diagnosable.

### Security
- Bumped `russh` to 0.61.1 (CVE-2026-46702): the SFTP client now bounds post-decompression SSH packet size, closing a remote resource-exhaustion vector a malicious server could trigger when compression is negotiated.
- Refreshed the dependency lock off the yanked `aes` 0.9.0 onto 0.9.1 (no API change).

## [0.23.0] - 2026-05-23

### Added
- **Remote filesystems** — pure-Rust SFTP / FTP / FTPS via russh + russh-sftp + rustls. Browse, open, copy, rename and move remote files in the same FileManager as local paths. Bookmarks accept `sftp://user@host:port/path`, `ftp://`, `ftps://` URLs; `Auto` auth chains SSH agent → `~/.ssh/config` (`IdentityFile`, `User`, `Port`, `Hostname` aliases) → default keys → password. Non-ASCII paths round-trip through URL encoding intact. See [`doc/en/vfs.md`](doc/en/vfs.md).
- **Static musl Linux binary** — `termide-0.23.0-x86_64-unknown-linux-musl.tar.gz` published with every release. Runs on Alpine, distroless containers and any glibc-free Linux. The Nix flake also exposes the same recipe as `#termide-static`.
- **Operations panel** —
  - Per-card popup menu (Pause / Resume / Cancel) on the bracketed type icon `[↑]`/`[↓]`/`[⧉]` (mouse) plus `Space` / `Esc` / `Delete` keyboard equivalents.
  - Cancel-cleanup modal for interrupted remote uploads — "Delete partial upload 'filename'?" defaults to delete and is batch-safe (only the in-flight file).
  - `⏸` indicator next to the icon while an operation is paused.
  - Help panel now lists Esc alongside Delete/Backspace.
  - See [`doc/en/operations.md`](doc/en/operations.md).
- **`--diagnostics` CLI** — pre-flight check (config parse, XDG dirs, git availability) that runs before terminal init and exits 0/1 for scripts.
- **Journal panel: level pills** — clickable `[TRACE] [DEBUG] [INFO] [WARN] [ERROR]` in the header row, also togglable via `Alt+1..5`.
- **Unsaved buffer recovery** — orphaned buffers from a crashed session were already restored as editor panels; now each one is announced in the Journal so users see why the extra tabs appeared.
- **FileManager — cursor-level actions** — `Ctrl+N`, `D`/`F7` and `Ctrl+V` now land at the cursor's tree level (the same subdir the cursor is inside), not always at the panel root. Documented in `doc/{en,ru,zh}/file-manager.md`.
- **Editor** — `F8` deletes the current line (or every line touched by the active selection) outside vim mode; read-only buffers ignore it.
- **Config** — layered global + per-project override loading; `<project>/.termide/config.toml` overlays `~/.config/termide/config.toml`. Both files save only diffs against their baseline.
- **Architecture docs** — `doc/en/architecture.md` gained an "Async Pipelines" table and a dedicated VFS section.

### Changed
- **Massive startup / interaction perf work.** Opening a session whose FileManagers root at a tracked home directory used to freeze the UI for seconds. Each of the following moved off the main thread, all polling through the existing `tick()` pattern:
  - Initial directory read.
  - Subtree expand (local: spawn worker, placeholder `…` row; matches the existing remote pattern).
  - Git status / git log panel refresh (5–6 `git` calls per refresh).
  - Git submodule discovery (`RepoManager` — only the top-level repo is found sync; submodules fold in via `poll`).
  - File-search git status walk.
  - Session restore — every panel in a group is built on its own worker thread, then joined in saved order.
  - Watcher repository registration — `WalkBuilder` traversal moved to a worker; `inotify_add_watch` calls are chunked across ticks (`INSTALL_CHUNK = 256`) so a 5000-directory repo doesn't spike a single tick.
- **SFTP transfers are pause-correct.** The actor now serves only atomic open / read-chunk / write-chunk / close commands; the chunk loop lives on a sync worker and polls pause/cancel between dispatches. A paused upload actually stops the byte stream, and other panels' `list_dir` requests still go through unblocked.
- **Same-host SFTP / FTP renames stay on the server** — `mv` within one connection no longer downloads and re-uploads.
- **Wide-view directory size** — default per-walk budget lowered from 1000 ms → 100 ms; `Space` on a directory also publishes its exact size into the shared cache so the column picks it up on the next redraw.
- **Config / session load failures route to the Journal** instead of stderr-before-raw-mode where the message scrolled away.
- **Editor** — cursor and current-line highlight hidden when the panel is unfocused; the visual state now matches the FileManager.
- **Modal block** — an empty title no longer punches a 1-cell gap through the top border.
- **Async tree-expand for remote FileManager panels** with `…` placeholder rows; duplicate-on-repeat-expand fixed in the same pass.
- **Inline blame annotation** in wrapped editor lines anchors to the last wrap-row instead of the cursor's row.

### Fixed
- Several SFTP cancel-edge cases: hanging actor on cancel, file handles left open, stale receivers wedging the russh-sftp request-id space — pause / cancel / reconnect are clean now (the cached `ConnectOptions` enables seamless auto-reconnect after a cancelled transfer).
- Remote panel rename routed through local `fs::*` (created a local dir + leftover file); now goes through the VFS path.
- Newly-created file/dir cursor placement: matches by full path, so an entry nested inside an expanded subdir is found correctly.
- Repeated expand on a remote directory used to duplicate children.
- Paste from clipboard always landed at the panel root regardless of cursor position.
- Editor → Commands: editing a project command's hotkey now invalidates the in-memory hotkey table so the rebind takes effect immediately.
- Panel terminal: live output no longer drags the user's scrollback position toward the tail.

### Removed
- **`--features vendored-openssl` build flag** — the workspace is pure-Rust (rustls + russh + russh-sftp), no OpenSSL footprint. The flag was a no-op already; removed from `release.yml`.

### CI / DX
- New `.github/workflows/ci.yml` runs fmt / check / clippy / test on every PR and push to main (release.yml stays release-only).
- `deny.toml` + `cargo-deny check` step covers advisories, licenses, bans and sources.
- Pre-commit hook documented in `CONTRIBUTING.md`.

[0.24.0]: https://github.com/termide/termide/releases/tag/0.24.0
[0.23.11]: https://github.com/termide/termide/releases/tag/0.23.11
[0.23.10]: https://github.com/termide/termide/releases/tag/0.23.10
[0.23.9]: https://github.com/termide/termide/releases/tag/0.23.9
[0.23.8]: https://github.com/termide/termide/releases/tag/0.23.8
[0.23.7]: https://github.com/termide/termide/releases/tag/0.23.7
[0.23.6]: https://github.com/termide/termide/releases/tag/0.23.6
[0.23.5]: https://github.com/termide/termide/releases/tag/0.23.5
[0.23.4]: https://github.com/termide/termide/releases/tag/0.23.4
[0.23.3]: https://github.com/termide/termide/releases/tag/0.23.3
[0.23.2]: https://github.com/termide/termide/releases/tag/0.23.2
[0.23.1]: https://github.com/termide/termide/releases/tag/0.23.1
[0.23.0]: https://github.com/termide/termide/releases/tag/0.23.0

## [0.22.1] - 2026-05-10

### Added
- **Config**: Layered global + per-project override loading. `<project>/.termide/config.toml` overlays the user's global `~/.config/termide/config.toml`, which in turn overlays built-in defaults. Both files now save **only** fields that differ from their baseline (defaults for global, defaults+global for the project file), so config files stay small and a future release-changed default no longer gets silently masked by the user's saved-as-default value.
- **Settings**: New footer button "Create / Remove project override" — single visual control to start writing a per-project diff or delete it (with confirmation). State derived from the existence of the project file; no separate flag.
- **Editor**: `F8` deletes the current line (or every line touched by the active selection) outside vim mode. Read-only buffers ignore the binding; clipboard is intentionally untouched.

### Changed
- **VFS**: Provider dispatch deduplicated via a closure-based `dispatch_remote` helper (no behavioural change; ~70 LOC removed from `crates/vfs/src/lib.rs`).
- **Cleanup**: Three unused public helpers removed (`is_supported`, `is_language_supported`, `count_files_under`); `panel-terminal` test code re-formatted to silence two `clippy --all-targets` lints.

### Fixed
- **Editor**: Inline blame annotation is now anchored to the last wrap-row of the cursor's logical line in word-wrap mode. Earlier it tried to follow the cursor's exact wrap-row, which on long lines is the row most full of code and pushed the annotation off the right edge — effectively invisible.
- **Panel terminal**: While a background program writes new output, the user's scrollback position no longer drifts toward the live tail. `scroll_up` now bumps `scroll_offset` (capped at scrollback length) when a line goes into the scrollback, so the visible window stays on the same content. Tail-following users (offset == 0) keep seeing the latest output as before.
- **Editor → Commands**: Editing a project command's hotkey through the modal now invalidates the in-memory hotkey table, so the rebind takes effect immediately and survives a restart.

### Security
- **openssl** bumped to `0.10.79` (transitive via `native-tls → suppaftp → termide-vfs`), closing GHSA-xv59-967r-8726 / CVE-2026-44662 (heap overflow in AES key-wrap-with-padding) and GHSA-xp3w-r5p5-63rr / CVE-2026-42327 (UB in `X509Ref::ocsp_responders` on non-UTF-8 OCSP URLs). Termide does not exercise either code path directly.

[0.22.1]: https://github.com/termide/termide/releases/tag/0.22.1

## [0.22.0] - 2026-04-30

### Added
- **Layout**: Unified accordion + split layout model with three-mode header drag and live vertical-divider resize (#18)
- **Layout**: Split-mode borders and `Alt+Shift+=` / `Alt+Shift+-` step resize for the focused panel

### Changed
- **Hotkeys**: Single end-to-end canonicalization at the matcher boundary — Cyrillic→Latin, shifted-glyph undo, Caps-Lock strip on letters, and VTE-only legacy quirks (`Ctrl+7→Ctrl+/`, `Ctrl+4→Ctrl+\`); inline conflict warnings in the keybinding picker; startup warnings for chords that require Kitty proto
- **Hotkeys defaults**: Tighter punctuation defaults — `panel_grow_vertical`/`panel_shrink_vertical` → `Alt+Shift+=`/`Alt+Shift+-`; `open_sessions` → `Alt+\`; `*.switch_directory` → `Ctrl+\`; `editor.toggle_comment` → `["Ctrl+/", "Ctrl+."]`; `editor.replace_all` → `["Ctrl+Alt+R", "Alt+R"]`; `editor.trigger_completion` → `["Ctrl+J", "Ctrl+Space"]`
- **Layout**: Accordion and split modes share one layout model; resize, drag, and keyboard-resize behaviour now identical in both
- **VFS**: `VfsManager` provider dispatch deduplicated via a closure-based `dispatch_remote` helper; semantics preserved (read-guard scope, cache invalidation)
- **Cleanup**: Dead helpers pruned across `i18n`/`highlight`/`panel-git-status`; two `clippy --all-targets` lints (test-only) silenced; visibility tightened on tree helpers in `panel-git-status`

### Fixed
- **Commands**: Editing a command via the modal now invalidates the cached hotkey table, so the rebind takes effect immediately and survives a restart
- **Commands**: Project-local `commands.toml` is honoured after a session switch — `commands_registry` and `hotkey_table` are reset on `project_root` change in `switch_to_session` / `create_new_session` / `move_session_to`
- **Commands**: Refined command-config editing flow and local-definition handling
- **Mouse**: Wheel events route to the panel under the cursor instead of the active panel
- **Operations panel**: Status icons normalised to match the rest of the UI
- **File ops**: Symlink destination handling unified across copy paths
- **Panel terminal**: Input passthrough compatibility with more shells / TUIs

### Documentation
- Hotkeys reference (`doc/{en,ru,zh}/keybindings.md`) refreshed; new panel-resize step documented; drag-overlay copy updated
- Shared project skills generalised so non-Anthropic agents can run them

[0.22.0]: https://github.com/termide/termide/releases/tag/0.22.0

## [0.21.0] - 2026-04-27

### Added
- **Menu**: Battery percentage and AC-charging indicator next to the clock
- **Modal**: Mouse-wheel scrolling in list modals
- **Editor**: Shift+click to extend text selection (with Alt+click fallback)
- **Editor**: Click the Tab indicator in the status bar to override `tab_size` per editor
- **File Manager**: Wide-view directory size computation with a per-frame time budget
- **File Manager**: Directory symlinks included in wide-view size walks
- **File Manager**: Directory-size cache shared across all FM panels and preserved across navigation
- **File Manager**: Symlink target shown in the properties modal; nested-entry status bar arrow fixed

### Changed
- **Performance (input)**: Drain Release/Repeat key events without generating idle ticks; fewer hotkey-table rebuilds and lower idle poll latency
- **Performance (system-monitor)**: Cache mount/process/network lookups; battery readings cached instead of rereading `/sys` every frame; removed duplicated `disk_space` module
- **Performance (file-manager)**: Async directory reload for watcher-triggered updates
- **Performance (editor)**: Wrap points precomputed once per physical line; per-render diagnostics-by-line map built once
- **Performance (highlight)**: Syntax fallback returns `Cow<str>` segments to skip `String` allocations
- **Performance (core)**: `Panel::prepare_render` borrows `Arc<Config>` instead of cloning
- **Performance (app)**: LSP diagnostics fan-out folded into a single panel loop
- **Performance (misc)**: Cache foreground-command lookup, menu layout and hunk regex; consolidate per-frame panel iterations
- **Observability**: Swallowed filesystem and Git errors are now logged instead of silently discarded
- **i18n**: Trivial string accessors generated via macro; `normalize_lang` made internal
- **Workspace**: `scripts` renamed to `commands` across the codebase; ~1.7K LOC of dead code pruned, several public APIs tightened to `pub(crate)`

### Fixed
- **Hotkeys**: Letter bindings honour Caps Lock state via the Kitty keyboard protocol
- **Terminal**: Encode Ctrl/Shift/Alt modifiers for arrow, Home and End keys (#17)
- **Terminal**: Keep the PTY screen alive on very small terminal sizes
- **Modal**: Coalesced mouse-wheel events routed into the active modal
- **Modal**: Tab-size modal no longer shows a stray prompt row
- **Settings**: Removed the non-functional `[X]` close button from the modal title
- **Editor**: Smart-indent triggers gated behind a recognized syntax
- **File Manager**: Disconnected async-reload channels handled gracefully (no more panic)
- **System Monitor**: Add Windows fallback for `get_disk_space_info_cached` so the indicators code path compiles on Windows targets

### Security
- **deps**: Bump `openssl` 0.10.76 → 0.10.78 to address GHSA-pqf5-4pqq-29f5 (CVE-2026-41676), GHSA-8c75-8mhr-p7r9 (CVE-2026-41678), GHSA-ghm9-cr32-g9qj (CVE-2026-41681), GHSA-hppc-g8h3-xhp3 and GHSA-xmgf-hq76-4vx2 — buffer overflow / out-of-bounds advisories in `Deriver::derive`, `aes::unwrap_key`, `MdCtxRef::digest_final` and PSK/cookie/PEM callbacks. Reached transitively via `suppaftp` → `native-tls`.
- **deps**: Bump `rand` 0.8.5 → 0.8.6 to address GHSA-cq8v-f236-94qc (Stacked Borrows soundness issue triggered by custom loggers calling `thread_rng()` under trace-level logging).

## [0.20.1] - 2026-04-21

### Added
- **Panels**: Context menu on `[≡]` button with Close / Split / Merge / Move actions (closes #16)
- **Panels**: Hotkey for panel action context menu (`Alt+K` / `Shift+F10`)
- **Panels**: Drag-and-drop panels by their top border

### Changed
- **Architecture**: Deduplicated drag overlay and menu action dispatch
- **Architecture**: Deduplicated layout-op error handling and navigation bookkeeping

### Fixed
- **Bookmarks**: Suppress xdg-open output to prevent TUI corruption

## [0.20.0] - 2026-04-19

### Added
- **LSP**: Rename symbol (default `F4`) — full WorkspaceEdit flow with prompt modal, guards for unsaved/no-identifier cases, i18n feedback on applied edits across 15 locales
- **Settings**: Redesigned settings modal with sidebar layout, functional groupings with separators, expandable Keybindings group, checkbox `[✓]/[✗]` style, native language names, Tab/Shift+Tab zone cycling
- **InfoModal**: Scrollable content for long script reports — scrollbar on right border, Up/Down/PageUp/PageDown/Home/End + mouse wheel
- **Panels**: `Ctrl+C` copy support across all panels; VTE terminal keyboard handling fix
- **Git**: Stash dropdown menu from git status button (replaces separate stash panel)
- **Git**: `F2` rename stash message in dropdown menu
- **Git**: Stage all / Unstage all / Revert all / Log buttons in git status panel
- **Help**: Help generator reads configurable keybindings instead of hardcoded values
- **i18n**: Translated 46 missing settings/permission keys, added English runtime fallback, `lsp_rename_*` keys across 15 locales

### Changed
- **Architecture**: Decomposed `modal_handler.rs` (1694 → 659 LOC, -61%) into 6 focused submodules (git/bookmark/search/path/progress/script)
- **Architecture**: Decomposed `mouse_handler.rs` (1531 → 530 LOC, -65%) into 3 submodules (indicators/submenu/layout)
- **Architecture**: Added `AppState` facade getters (`is_menu_open`, `is_resource_modal_open`, `has_pending_action`, `active_modal`) and migrated 13 call sites
- **Architecture**: Consolidated 5 scattered batch-operation fields into `BatchOperationState`; activated `app-core` traits (`StateManager`, `ModalManager`, `PanelProvider`, `LayoutController`)
- **Architecture**: Save-and-close flow deduplicated via `queue_remote_editor_upload()` / `force_save_active_editor()` helpers
- **Config**: Migrated all panel hotkeys from hardcoded matchers to config-driven `HotkeyTable`
- **Hotkeys**: `rename_symbol` default set to `F4` to avoid conflict with `save = [F2, Ctrl+S]`
- **Cleanup**: Removed empty `app-event` crate; replaced `eprintln!` with `log` macros; dropped debug logging remnants

### Fixed
- **Security**: Updated `rand` 0.9.2 → 0.9.4 (advisory RUSTSEC)
- **App**: Closed 3 async-upload TODOs — remote editor saves now reliably queue uploads
- **Settings**: UTF-8-safe string truncation in labels (no more panics on multibyte characters)
- **Scripts**: `direnv` JSON integration, `setsid` for process groups, journal prepare_render fix
- **Git**: Directory color uses majority status instead of worst-wins
- **Git**: Stash drop message, `Delete` key handling, and button click offset
- **Git**: Stash dropdown polish and cleanup
- **UI**: Toggle behavior for disk indicator, stash dropdown, terminal submenu, bookmark/script groups
- **UI**: Centred `rwx` headers over permission checkboxes in file properties modal
- **Image**: Prevent double `Escape` firing when closing image viewer

### Removed
- **Crates**: Empty `app-event` crate

[0.21.0]: https://github.com/termide/termide/releases/tag/0.21.0
[0.20.1]: https://github.com/termide/termide/releases/tag/0.20.1
[0.20.0]: https://github.com/termide/termide/releases/tag/0.20.0

## [0.19.0] - 2026-04-14

### Added
- **Scripts**: Form-based script creation (ScriptCreateModal) with name, group, type (Terminal/Background/Report), and project scope
- **Scripts**: F2 rename and Delete with confirmation for scripts and bookmarks in menu
- **Scripts**: Type icons in menu and operation cards (terminal, background, report)
- **Hotkeys**: Config-based `HotkeyTable` system replacing hardcoded key matching — all hotkeys now configurable via TOML
- **Hotkeys**: Cyrillic keyboard normalization for all keys (ЙЦУКЕН → QWERTY mapping including punctuation)
- **Panels**: Escape requests panel close with confirmation dialog
- **Errors**: All errors now shown as InfoModal dialogs instead of transient status bar messages
- **File Manager**: Interactive permissions editor in file info modal
- **Menu**: Disk usage indicator, calendar grid in menu navigation
- **Menu**: Project-local `.termide/` support for bookmarks and scripts
- **Sessions**: Delete session via Delete key in sessions modal

### Changed
- **Hotkeys**: `open_external` unified across FM and Git Log panels — default changed to `O / Alt+Enter` for VTE terminal compatibility
- **Hotkeys**: Removed dead "Universal" hotkeys section from help panel and config (-435 LOC)
- **Operations**: Redesigned operation cards with border titles, icons, and elapsed timer
- **Operations**: Script name shown in card border title instead of generic "Script"
- **Layout**: Width distribution uses largest-remainder algorithm (fixes rounding errors)
- **Refactor**: Removed legacy HotkeyKind/Hotkey/HotkeyProcessor semantic layer
- **Refactor**: Editor blame converted from hotkey toggle to config setting
- **Logging**: Silent `let _ = fs::*` operations replaced with proper error logging

### Fixed
- **SSH**: Prevent blank screen on SSH — skip `supports_keyboard_enhancement()` detection, add panic handler for terminal state recovery, guard against 0×0 terminal size
- **SSH/Terminal**: Add bounds checks to prevent buffer index panics during terminal resize
- **SFTP**: Fix overwrite conflict for local-to-remote copy — use upload request instead of remote-to-remote, resolve destination as file or directory via VFS stat
- **Scripts**: Sanitize filenames in script create/rename (replace invalid chars with `-`)
- **Scripts**: Fix submenu executing wrong script with mixed project/global sources
- **Scripts**: Properly kill scripts on cancel, support parallel report scripts
- **Keyboard**: Restore lost keys from 0.18.2 (PageUp/PageDown/Enter in Outline, Diagnostics, Git panels)
- **Keyboard**: Fix editor F2/F4 binding conflicts with global hotkeys
- **File Operations**: Preserve permissions on copy, use `fs::rename` for same-filesystem move
- **i18n**: Remove duplicate keys, fix unlocalized help panel strings

## [0.18.2] - 2026-04-02

### Added
- **Layout**: Adaptive default layout — 1/2/3 panel groups based on terminal width (<100, 100-139, 140-199, 200+)
- **Directory Picker**: Tree-view navigation with expand/collapse (Right/Left), cursor matching file manager style
- **Hotkey**: `Alt+N` for creating new session (configurable)
- **Theme**: Norton Commander theme auto-selected on Linux VT (16-color terminals)
- **File Manager**: Single-file copy shows full path with filename for inline rename

### Fixed
- **Help Panel**: Opens in wide group instead of narrow sidebar
- **Command Palette**: Category and keybinding columns right-aligned consistently
- **Git**: Directory replacing deleted symlink now shows correct status (Added, not Deleted)
- **Directory Picker**: Long paths truncated left with ellipsis

### Changed
- **Layout**: Auto-width groups now receive remaining space (not average of fixed groups)
- **Documentation**: Updated theme count to 38, added missing themes to tables, Ctrl+Shift+P hotkey

## [0.18.1] - 2026-03-27

### Fixed
- **File Manager**: Canonicalize `current_path` at all write points (new, navigate, GoHomeDir) to fix symlink/bind-mount watcher mismatch — root cause of repeated auto-refresh failures
- **Git**: Skip diff coloring for gitignored files (added `git check-ignore` check)
- **Git**: Count local commits as ahead when remote refs are missing (empty remote after clone)
- **Watcher**: Canonicalize repo root from git events; detect MERGE_HEAD, FETCH_HEAD, REBASE_HEAD, CHERRY_PICK_HEAD
- **Terminal**: Strip spurious newlines from soft-wrapped lines on copy (track per-line `wrapped` flag)
- **Modal**: Use high-contrast fg/bg inversion for buttons in all remaining modals (info, info_action, editable_select, tree_search)
- **Terminal**: Add debug_assert for buffer/wrapped-flag length sync

## [0.18.0] - 2026-03-26

### Added
- **Git Stash Panel**: Dedicated panel for stash management (Pop, Apply, Drop, Diff via context menu); accessible from Tools menu and git-status Stash button; session persistence
- **Git Blame**: Inline blame annotation in editor (Alt+B) showing author, age, commit hash, and summary; enabled by default for git repos; async loading
- **Command Palette**: Quick access to all commands via Ctrl+Shift+P
- **LSP Find References**: Open references panel with Shift+F12 showing all symbol usages across project
- **LSP Rename Symbol**: Rename symbol under cursor with F2 via LSP WorkspaceEdit
- **Themes**: 4 new themes — blue-sky (light), pinky-pie (light), green-backs (light), billiard (dark)
- **Git Diff**: Adaptive diff colors and arrow key navigation

### Changed
- **Git Stash**: Extracted from inline ViewMode in GitStatusPanel into standalone `panel-git-stash` crate with SelectModal context menu
- **Panels**: Git Log, Git Stash, and Git Diff panels are now singletons (reuse existing instead of creating duplicates)
- **Editor**: Opening an already-open file focuses existing tab instead of creating duplicate
- **Modal Buttons**: Use high-contrast fg/bg inversion for reliable readability across all themes
- **Terminal**: F-key escape sequences replaced with lookup table
- **SFTP**: Magic number polling intervals replaced with named constant

### Fixed
- **InputModal**: Left/Right arrows now navigate between OK/Cancel buttons (were intercepted by text input handler)
- **Git**: Lines outside git repo no longer incorrectly highlighted
- **Git Blame**: Annotation rendered on correct line (was off by one)
- **Git Blame**: Age formatter shows months range (avoids "0 years ago")
- **Hotkeys**: Resolved terminal/tmux conflicts, added missing bindings
- **Config**: `switch_directory` hotkey now configurable in file_manager.keybindings

### Refactored
- **PanelExt**: Reduced deprecated downcasts and fixed code duplication
- **Documentation**: Updated hotkey references, added Git Blame docs, Find References and Rename Symbol docs in all 3 languages

## [0.17.4] - 2026-03-24

### Added
- **Git Diff**: File filter dropdown — click the `[filter ▼]` widget to show only Added, Modified, or Deleted files

### Fixed
- **Git**: Subdirectories inside untracked directories now highlighted green (previously only top-level untracked dir was highlighted)
- **File Manager / Git Status**: Actions on deleted files (move, copy, rename) now ignored to prevent errors
- **Editor**: All lines shown as Added (green) for untracked files, matching git status behavior

### Performance
- **Core**: `Panel::prepare_render()` now accepts `Arc<Config>` instead of `&Config`, eliminating per-frame allocations
- **Editor**: Pre-allocated syntax style vectors in line rendering

## [0.17.3] - 2026-03-23

### Added
- **Session Switcher**: Filter input field in the session switching modal — typing filters sessions by path (case-insensitive substring match); backspace removes last character; navigation (↑↓/Enter/Esc) works on filtered results
- **Git Log**: Repo and branch selector dropdowns — click `[repo ▼]` or `[branch ▼]` widgets to open inline dropdown lists for switching repositories and branches directly from the git log panel

### Fixed
- **Git Log**: Clicking an open selector now toggles it closed (instead of reopening); clicking one selector while another is open now switches immediately in one click
- **Git Log**: Branch dropdown aligned with its selector position; branch selector placed immediately after repo selector
- **File Manager**: Bare Cyrillic hotkeys now translated to Latin (e.g., pressing `щ` on ЙЦУКЕН layout acts as `o`)
- **File Manager**: `o` key added as fallback for open-with-external-viewer (in addition to `Ctrl+Enter`)
- **Git Status**: Modifiers cleared when rendering dropdown items to prevent strikethrough style bleeding from previous render
- **Git**: Repository paths canonicalized to deduplicate symlinked repos (prevents same repo appearing twice in repo selector)
- **Editor**: Render cache invalidated after file reload from disk — ensures updated content is displayed immediately

### Changed
- **Assets**: Theme screenshots refreshed, 15 new theme previews added
- **Refactor**: Magic submenu index numbers replaced with named constants (`SESSIONS_SUBMENU_SWITCH`, `TOOLS_SUBMENU_GIT_LOG`, etc.) across all menu action handlers

[0.17.4]: https://github.com/termide/termide/releases/tag/0.17.4
[0.17.3]: https://github.com/termide/termide/releases/tag/0.17.3

## [0.17.2] - 2026-03-22

### Added
- **Menu**: Keyboard navigation over resource indicators — Right/Left arrows now extend beyond the 5 menu items to the 4 right-side indicators (network ↓/↑, CPU, RAM, clock); Enter opens the corresponding modal and closes the menu; indicators highlight with the standard selected style when focused
- **System Monitor**: Network activity modal — clicking the ↓/↑ network indicator opens a modal showing top processes by connections; displays listening TCP ports and established connection count per process, sorted by connections descending; works cross-platform without elevated privileges (Linux: `/proc/net/tcp`+`/proc/<pid>/fd`; macOS: `lsof`; Windows: `netstat`+`tasklist`)

### Changed
- **Layout**: New panels inserted immediately after the currently active panel (instead of appending to the end of the group)
- **Git Status**: Panel now auto-registers with the filesystem watcher independently — no longer requires a file manager panel to be open in the same repository for auto-refresh to work
- **File Manager**: Read-only `R` attribute rendered in `disabled` color (less visual noise)
- **Theme**: Terminal `disabled` color changed from `[200,200,200]` to `[120,120,120]` to be visually distinct from `fg [204,204,204]`

### Fixed
- **App**: Log UTF-16 decode errors in WSL shell detection instead of silently dropping them

## [0.17.1] - 2026-03-21

### Fixed
- **Windows**: Session files now save correctly — drive letter prefix (`C:\`) is stripped via `Path::components()` to prevent sessions from being written to the project root
- **Windows**: Triangle symbols `▶` (U+25B6) replaced with WGL4-compatible `►` (U+25BA) in all UI panels — fixes square/tofu rendering in default Windows console fonts

### Changed
- **Refactor**: `panel-file-manager/src/lib.rs` split into focused modules — `SelectionState`→`selection.rs`, `NavigationState`→`navigation.rs`, git tracking→`git_status.rs`, file-open logic→`operations.rs`; file reduced from 2694→2143 LOC
- **Refactor**: LSP server mutex guards hardened against thread-panic lock poisoning (`unwrap()` → `unwrap_or_else(|e| e.into_inner())`)
- **Refactor**: Eliminated 29 redundant string allocations (`to_string_lossy().to_string()` → `into_owned()`) across multiple crates

## [0.17.0] - 2026-03-20

### Added
- **UI / Editor / Terminal**: Hex color preview popup on `Ctrl+click` — clicking on a `#rgb` or `#rrggbb` value shows a color swatch while the button is held; works in both the editor and terminal panels
- **Theme**: 9 new built-in themes: `ayu-dark`, `catppuccin-macchiato`, `everforest`, `github-dark`, `gruvbox`, `kanagawa`, `material-ocean`, `rosepine`, `tokyonight`

### Fixed
- **Theme**: Menu bar items and editor cursor line now use `accented_fg` on `accented_bg` surfaces — fixes low-contrast rendering in `windows-98`, `dos-navigator`, and similar themes
- **Theme**: Disabled color contrast improved in classic retro themes (`norton-commander`, `volkov-commander`, `far-manager`, `windows-95`); `github-light` error color corrected to GitHub red
- **UI**: Indic script cursor desync fixed via unicode-width fork — cursor no longer drifts when editing text containing Devanagari, Bengali, or other complex scripts
- **File Manager**: Right arrow key now only expands directories in tree view; no longer also triggers directory entry
- **App**: Fixed double-nesting when copying a directory with OverwriteAll conflict mode selected

### Changed
- **Refactor**: Removed leaky pub exports, dead methods, and simplified regex statics across multiple crates (no behavior change)

## [0.16.4] - 2026-03-19

### Fixed
- **Git Status**: Pull button was silently truncated in narrow panels — buttons now wrap to the next line instead of being cut off
- **Git Status**: Button order changed to Pull → Push → Diff → Commit so the most critical sync actions appear first and are always visible
- **Git**: Files inside untracked directories now correctly show as Added in file manager git status
- **App**: `git fetch` errors are now displayed in the status bar instead of being silently swallowed
- **CI**: AUR PKGBUILD is now synced from the repository before version substitution

### Changed
- **Git Status / App**: Git panels (git-status, git-log) now sync their repository list when the user navigates to a new directory, so the panel reflects the current repo without a manual refresh
- **Refactor**: `ThemeColors` now derives `Copy` — eliminates heap allocations on every render frame
- **Refactor**: Parallel `unstaged_*` / `staged_*` tree fields in `GitStatusPanel` consolidated into a single `FileTree` struct, removing 8 redundant fields
- **Refactor**: Duplicate header rendering code in git-status panel extracted into reusable helpers (×4 → ×2 call sites)

## [0.16.3] - 2026-03-17

### Fixed
- **App**: Disk space in status bar is now cached per tick instead of calling `statvfs` on every render — eliminates potential blocking on NFS mounts under high input rate
- **App**: Resource modal auto-refresh interval unified — CPU, RAM and Disk modals all use `resource_monitor_interval` config (was hardcoded 3 s for process modals); default interval changed from 1 s to 2 s

### Changed
- **App**: Disk modal now auto-refreshes on tick like CPU/RAM modals
- **Refactor**: Preserve original error types in VFS/file-ops paths; remove dead code; optimize rendering hot path

## [0.16.2] - 2026-03-16

### Added
- **Terminal**: Shell picker submenu under Windows > Terminal — lists all available shells, marks default with ●, saves selection to config (#12)
- **Docs**: Document shell picker feature (en, ru, zh)

### Fixed
- **Terminal**: Deduplicate shells in picker by canonical path + basename — eliminates duplicates on NixOS and merged-usr distros while keeping sh/bash as separate entries
- **Windows**: Coalesce pasted text into bracketed paste events (#13)
- **Git**: Unpushed commits indicator was showing total project commits instead of ahead count
- **App**: Clear cached shells when closing menu by clicking outside it
- **UI**: Move Terminal to top of Windows submenu for faster access

## [0.16.1] - 2026-03-16

### Added
- **UI**: Calendar modal on clock click — monthly grid with day navigation (arrows, PgUp/PgDn for months, Home to jump to today)
- **UI**: Colorized panel titles and improved status bar styling
- **Git Log**: Open commit in browser with `o` / `Shift+Enter`
- **VFS**: FTP/FTPS and SMB connection support via address bar input

### Changed
- **Refactor**: Deduplicate grapheme utilities into shared `termide-ui` crate
- **Refactor**: Clean up menu bar rendering, remove `TERMIDE_LANG` env variable
- **Performance**: Fix render allocations and SMB error handling
- **Docs**: Sync documentation with recent feature changes

## [0.16.0] - 2026-03-16

### Added
- **Windows**: Native Windows 11/10 support via ConPTY (no WSL required)
- **Windows**: Native Win32 APIs for disk space and process detection
- **Windows**: Git Bash as default shell on Windows
- **Windows**: Windows MSVC build target in CI release workflow
- **Theme**: New `terminal` theme — classic black background inheriting terminal colors
- **Network**: Real-time network throughput monitor (download/upload) in header bar
- **Security**: Symlink validation in scripts loader — prevents directory traversal
- **Security**: 30-second timeout for background git operations (push/pull/fetch)
- **i18n**: `git_operation_timed_out` translation in all 15 locales

### Fixed
- **SSH**: Ghost divider line and drag coalescing for smooth SSH resize
- **File Manager**: Open source files in editor even when executable bit is set
- **Docs**: Sync theme count (24→25), language count (15+→21), remove duplicate doc link
- **Docs**: Fix deb package description language count (19→21)
- **Code**: Cleanup formatting and dead code from PRs #9, #10, #11
- **Build**: Gate executable permission variables behind `#[cfg(unix)]`

### Changed
- **Performance**: Replace PowerShell subprocess calls with native Win32 APIs on Windows
- **Docs**: Add native Windows installation section to all 3 language docs
- **Docs**: Update platform list to include native Windows support

## [0.15.4] - 2026-03-15

### Added
- **Git Status**: Trigger background `git fetch` on Ctrl+R refresh
- **App**: Bell notification for git push/pull completion and terminal exit
- **Clipboard**: OSC 52 clipboard fallback for SSH/headless sessions

### Fixed
- **Terminal**: Prevent panic on UTF-8 search (Cyrillic, emoji) — advance by match length instead of 1 byte
- **File Manager**: Use full path when entering nested directory in tree view

### Changed
- **Performance**: Skip mouse handler processing and simplify OSC 52 logic
- **Performance**: Skip idle timer reset and forced redraw on mouse hover
- **Performance**: Replace `.repeat()` padding with static `PAD` slice in file manager rendering
- **Performance**: Cache menu layout in `MenuLayout` struct to avoid repeated allocations
- **Refactor**: Improve VFS error logging in batch handler
- **Refactor**: Remove dead `find_visible_by_path` method

## [0.15.3] - 2026-03-13

### Added
- **Git Status**: Single-click directory icon to expand/collapse tree nodes

### Fixed
- **Modal**: Correct quit confirm title and button layout
- **Modal**: Adjust checkbox indent and spacing in InputModal
- **i18n**: Add `modal_confirm_title` key — ConfirmModal no longer shows "Yes" as title
- **i18n**: Localize hardcoded "Confirm" string in file manager paste confirmation
- **Docs**: Fix syntax highlighting language count in README (19→21)
- **Docs**: Update CONTRIBUTING.md with current i18n architecture (15 languages, TOML-based)

### Changed
- **Refactor**: Replace SelectModal with ChoiceModal for editor close dialogs

## [0.15.2] - 2026-03-13

### Added
- **File Manager**: Tree view with expandable/collapsible directories (`→`/`←` or `l`/`h` in vim mode)
- **File Manager**: In-tree incremental search (`/`) — filter files as you type with auto-expanding matches
- **File Manager**: Cascading selection — selecting a directory with `Insert` selects all files within it
- **File Manager**: Nested git status — directories show aggregated status of their children
- **Search Modal**: Unified search modal with file glob and content search modes from file manager (`Ctrl+F` / `Ctrl+Shift+F`)
- **Search Modal**: Tree-based result display with expandable file groups for content search

### Fixed
- **Search Modal**: Tab/Shift+Tab now trigger initial search in non-Text modes (matching F3/Shift+F3 behavior)
- **Git**: Handle empty original content for new/untracked files in diff cache

### Changed
- **Refactor**: Deduplicated nested submenu navigation in menu actions
- **Refactor**: Unified git-status file selection and tree building helpers
- **Refactor**: Extracted VFS URL parsing and source name helpers in batch handler
- **Performance**: Cached line boundary calculations in git-status panel loops

## [0.15.1] - 2026-03-11

### Added
- **Terminal**: Text search with `Ctrl+F` — search across scrollback buffer and visible screen with match highlighting, navigation, and auto-scroll
- **Core**: `Searchable` trait for unified search across Editor, Journal, and Terminal panels
- **Search Modal**: Button focus area with keyboard navigation (Down/Up to switch, Left/Right between buttons)

### Fixed
- **Terminal**: Default search keybinding changed from `Ctrl+Shift+F` to `Ctrl+F` — host terminals (GNOME Terminal, etc.) intercept `Ctrl+Shift+F`
- **Journal**: Auto-scroll suppressed during active search to keep viewport on current match

## [0.15.0] - 2026-03-10

### Added
- **File Manager**: Symlink creation from copy modal and follow-symlink navigation
- **UI**: Emoji icons for panel types in title bars
- **File Manager**: Font modifier styling — ITALIC for symlinks, BOLD for executables
- **File Manager**: Grouped sorting — directories → executables → regular files

### Changed
- **Editor**: Git gutter uses background-colored line numbers with auto-contrast instead of foreground coloring
- **Refactor**: Reduced coupling, tightened visibility (`pub(crate)`), extracted panel factory
- **Performance**: Cached SFTP file name allocations, reduced per-frame config cloning
- **Docs**: Updated README and docs — CLI options, Ctrl+/ comment toggle, package managers

### Fixed
- **Git**: Inline diff detection for consecutive modified lines (Del…Del Ins…Ins blocks now paired correctly)
- **Git**: Symlink path resolution — `canonicalize()` before `strip_prefix()` for diff loading
- **Git Status**: Directory label format in tree view
- **UI**: Tilde expansion for paths in copy/move/symlink/save-as modals
- **File Ops**: Prevent symlink deletion from destroying target data
- **File Manager**: Prevent ".." from being selectable
- **UI**: Double spacing in collapsed panel headers

## [0.14.3] - 2026-03-08

### Fixed
- **Resource Modals**: Normalize CPU % by core count (0–100% of total capacity instead of per-core)
- **Resource Modals**: Filter out threads (VfsLoader, GC-marker) and kernel threads from process lists
- **Resource Modals**: Add spacing between count and CPU columns to prevent overlap at high values
- **Resource Modals**: Always show process count (was hidden when count=1)
- **Resource Modals**: Remove colon separator in Segments modal rows
- **Resource Modals**: Auto-refresh CPU/RAM modal content every 3 seconds
- **SSH**: Validate port range (1–65535) instead of digits-only check

### Changed
- **Disk Modal**: Redesigned with free%, free, total columns and colored free values
- **Disk Modal**: Added column headers with i18n support (15 languages)
- **Performance**: Zero-allocation case-insensitive sort in file manager (was allocating on every comparison)
- **Performance**: Terminal keybindings cloned only when config changes (was cloned every frame)
- **Code Quality**: Synced ConflictResolution variants across core/modal/file-ops
- **Code Quality**: Added `#[must_use]` to 11 pure getters in BookmarksConfig
- **Security**: FTP password zeroed in memory on provider drop

## [0.14.2] - 2026-03-06

### Added
- **Editor**: Toggle comment with Ctrl+/ (line/block comment support)
- **Keyboard**: Ctrl+/ added to help panel shortcuts

### Fixed
- **Keyboard**: Normalize Ctrl+/ for legacy terminals (Ctrl+_ mapping)
- **App**: Require double-click on title bar to open directory picker (prevents accidental opens)

## [0.14.1] - 2026-03-04

### Added
- **File Manager**: F3 opens files in read-only mode (view mode)
- **File Manager**: Directory picker on title click for quick navigation
- **File Manager**: Keyboard shortcuts for rename (R/F2), view (V/F3), edit (E/F4), new file (F/Ctrl+N)
- **Git Status**: Keyboard shortcuts for revert, view, edit
- **Layout**: Improved accordion panel insertion and removal behavior
- **Git Status**: Auto-fetch on init to show correct pull button

### Changed
- **I18n**: `init()` and `init_with_language()` return `Result` instead of panicking
- **Codebase**: Added `must_use` attributes, fixed unsafe unwrap

### Fixed
- **Terminal**: Resize alternate screen buffer on window resize
- **Git**: Canonicalize paths before strip_prefix for symlink repos
- **File Manager**: Deleted files appearing in wrong directories
- **Keyboard**: Fixed incorrect and missing keyboard shortcuts in documentation

## [0.14.0] - 2026-02-25

### Added
- **UI**: Replace home directory prefix with `~` in terminal title, FileManager panel titles, sessions modal, and directory switcher
- **UI**: Update terminal window title when switching or creating sessions
- **Editor**: Ctrl+Up/Down for paragraph/symbol navigation

### Changed
- **UI**: Deduplicate dropdown hit-test logic, fix swallowed errors in menu rendering
- **Panel Operations**: Auto-close operations panel when it becomes empty

## [0.13.0] - 2026-02-24

### Added
- **Editor**: Auto-closing brackets and quotes with context-aware logic (don't close apostrophes mid-word)
- **Editor**: Ctrl+Left/Right word navigation and Ctrl+Shift+Left/Right word selection
- **Editor**: Split-bracket indent on Enter between matching pairs (`{|}` → three lines with proper indentation)
- **Editor**: `auto_indent` and `auto_close_brackets` configuration options

### Changed
- **VFS**: Deduplicate SFTP operations via `spawn_op`/`spawn_op2` helpers (-224 LOC)
- **I18n**: Replace unsafe pointer cast in `t()` with safe `Box::leak` approach
- **I18n**: Use fallback instead of guard-checked `expect` in pluralization
- **Editor**: Extract word boundary logic from vim motions into shared `word_boundary` module

## [0.12.6] - 2026-02-23

### Added
- **CLI**: `--log-level`, `--no-lsp`, `--config` command-line arguments
- **CLI**: Set terminal window/tab title to "Termide: <cwd>" on startup
- **Logger**: TRACE log level and trace user input events

### Changed
- **Git Status**: Remove flat view mode, always use tree view
- **ProgressModal**: Deduplicate 8 constructors via shared base + struct update syntax (-166 LOC)
- **API**: Return `&[T]` instead of `&Vec<T>` in `PanelGroup::panels()` and `TerminalScreen::get_line_by_absolute()`
- **Constants**: Centralize `MEGABYTE` in `termide-config`, remove duplicates from `panel-editor` and `modal`
- **Codebase**: Remove unused `_buffer` parameter from `build_diagnostic_maps`
- **Codebase**: Remove redundant clones, deduplicate constructors, fix unwrap

### Fixed
- **Editor**: Invalidate wrap cache on selection delete
- **Journal**: Account for trailing empty line in auto-scroll check
- **Journal**: Bind auto-scroll to cursor position instead of key codes
- **Terminal**: Update cursor position on arrow key movement

## [0.12.5] - 2026-02-18

### Added
- **Git Status**: Auto-switch between tree and flat view based on panel width (>= 35 columns)

### Changed
- **Codebase**: Extract `TREE_VIEW_MIN_WIDTH` and `MAX_RECURSION_DEPTH` shared constants
- **Codebase**: Consolidate `SpeedTracker` into `file-ops` crate, re-export from `state`
- **Terminal**: Deduplicate constructor code via `set_env`/`spawn_reader`/`build` helpers
- **Git Status**: Optimize tree prefix computation from O(n²) to O(n) reverse scan
- **Clipboard**: Graceful fallback on headless systems instead of panic

### Fixed
- **Editor**: Correct cursor positioning with wide characters at wrap boundary and in no-wrap mode
- **Session**: Prevent duplicate unsaved buffer files on auto-save

## [0.12.4] - 2026-02-17

### Added
- **Outline**: YAML and XML regex fallback for outline panel when tree-sitter is unavailable

### Changed
- **Editor**: Extract viewport/scrolling logic into dedicated `editor_viewport.rs` module (-572 LOC from core.rs)
- **Editor**: Extract generic `poll_receiver` helper to deduplicate LSP polling methods
- **Codebase**: Replace magic numbers with named constants (`PROGRESS_THROTTLE_FILES`, `EXECUTABLE_MASK`)
- **Codebase**: Preserve error context in VFS lock/channel/FTP operations instead of discarding
- **Codebase**: Replace `is_none() + unwrap()` pattern with `is_none_or` and double `.all()` iteration with single-pass check

### Fixed
- **Git**: Show local commit count as ahead when no remote/upstream is configured
- **Git Status**: Re-discover git repository on refresh after external `git init`
- **Session**: Restore non-empty orphaned unsaved buffers instead of deleting them
- **Editor**: Invalidate word-wrap cache on undo/redo, replace, and vim edits

## [0.12.3] - 2026-02-09

### Added
- **Status Bar**: Display LF/CRLF line ending indicator before encoding (UTF-8) in editor status bar

### Changed
- **Codebase**: Deduplicate `LineEnding` enum — single definition in buffer crate root, reused internally

### Fixed
- **Buffer**: Normalize CRLF line endings to LF on file load, preserving original type for save
- **Editor**: Normalize line endings on paste for VTE-based terminals
- **Terminal**: Add Alt+Enter as fallback for newline input on VTE terminals
- **Codebase**: Reduce duplication and improve safety across VFS and rendering modules

## [0.12.2] - 2026-02-08

### Added
- **i18n**: Complete Chinese (zh) localization — translated all remaining English UI strings
- **i18n**: Translated git action labels (stage, unstage, commit, etc.) across all locales
- **Documentation**: Added full Chinese documentation set (doc/zh/) and README.zh.md

### Changed
- **Documentation**: Updated English and Russian docs with new keybindings, menu items, panel types, LSP/Vim sections, and modernized config example

### Fixed
- **Outline Panel**: CJK headings no longer truncated to first character — use display width instead of character index for positioning

## [0.12.1] - 2026-02-08

### Added
- **File Manager**: Toggle hidden files visibility with `.` key

### Changed
- **InfoModal**: Colored segments, multiline support and truncation
- **Codebase**: Rename Welcome remnants to Help across codebase

### Fixed
- **Layout**: Refresh stale panels on next/prev switch in group — fixes Outline going empty after panel switching
- **File Manager**: Treat directory symlinks as directories
- **App**: Enable search in vim normal mode and JournalPanel
- **Outline**: Repopulate outline after expanding collapsed panel

## [0.12.0] - 2026-02-07

### Added
- **Outline Panel**: Structural code navigation using tree-sitter queries with regex fallback for markdown/HTML; tree-drawing prefixes, cursor sync, and keyboard/mouse navigation
- **Global Hotkeys**: Keyboard shortcuts for Outline, Diagnostics, and Git Log panels
- **File Manager Edit Button**: Replaced Git Status button with Edit in file info modal

### Changed
- **Code Quality**: Eliminated unsafe unwraps across app and git crates, deduplicated batch operations
- **Modal Handlers**: Deduplicated modal handlers, added path traversal guard and RwLock recovery
- **Panel Index Cleanup**: Removed obsolete panel_index field, fixed related clippy warnings

### Fixed
- **Outline Sync**: Outline panel now syncs on panel navigation and debounces live edits
- **Outline Resync on Close**: Outline panel rebinds to another editor (or clears) when tracked editor is closed

## [0.11.3] - 2026-02-06

### Added
- **4 New Themes**: Matrix (digital rain), Terminator (Skynet HUD), Pip-Boy (Fallout CRT), Manuscript (medieval parchment)
- **Adaptive Default Layout**: Width-dependent session layout — wide terminals (>= 160 cols) get 3-group layout, normal terminals get 2-group layout with sidebar accordion
- **GitStatus in Default Sidebar**: New sessions automatically include GitStatus panel in the sidebar accordion when a git repository is detected
- **Diagnostic Row Caching**: Editor caches diagnostic row counts for faster viewport scrolling with LSP diagnostics

### Changed
- **State Module Split**: Refactored monolithic `crates/state/src/lib.rs` into submodules: `batch`, `layout`, `operations`, `pending_action`, `ui`
- **Dead Code Removal**: Removed unused functions across 7 crates (`clipboard::has_text`, `git::check_git_available`, `logger::get_entries`, `editor::cleanup_temp_file`, `theme::all_themes`, `theme::builtin_theme_names`, `system_monitor::cpu_usage_float`)
- **Removed text-search Crate**: Deleted unused `termide-text-search` crate from workspace
- **Documentation Update**: Updated README, architecture docs, and developer guides (EN+RU) to reflect current workspace structure, 24 themes, 15 languages, all panel types

### Fixed
- **Security**: Updated `time` crate 0.3.45 → 0.3.47 to fix stack exhaustion DoS vulnerability (GHSA)
- **Editor Diagnostics**: Reverted diagnostic row deduplication in viewport scroll methods that caused incorrect cursor positioning
- **Watcher Registration**: Fixed watchers not being registered on session switch; optimized `.git/objects` watch exclusion
- **Terminal Link Highlight**: Fixed Ctrl+click link highlight offset for non-ASCII content in terminal panel
- **File Manager Git Root**: Preserved `git_root` on internal navigation, fixed post-commit refresh losing git context
- **File Manager Redraw**: Fixed missing redraw after async git status update completed

## [0.11.2] - 2026-02-04

### Fixed
- **UTF-8 Panic**: Fixed byte-index panic when resizing panels with Cyrillic/Unicode text — now truncates by display width, not byte offset
- **Editor Word-Wrap Mouse**: Fixed cursor mispositioning on mouse click in word-wrap mode when git deletion markers are present
- **Editor Word-Wrap Scroll**: Fixed viewport failing to scroll past lines with deletion markers or diagnostics in word-wrap mode
- **Editor Visual Cursor**: Fixed cursor unable to reach end of last visual row during page navigation in word-wrap mode
- **Editor Inline Diff Click**: Fixed mouse click offset in no-wrap mode when inline diff deleted text shifts visual positions
- **Git Status Stale Panel**: Collapsed git-status panel now marks itself stale and refreshes on expand (MarkStale/RefreshIfStale)
- **Git Status Title**: Collapsed git-status panel title now updates branch/file counts on git/filesystem events
- **Git Diff Toggle**: Mouse click on file header in git-diff panel now correctly toggles collapse
- **Operations Panel Focus**: Opening the operations panel no longer steals focus from the active editor
- **CodeQL Security**: Broke taint chain for cleartext-logging alerts in VFS types

## [0.11.1] - 2026-02-01

### Added
- **109 New Tests**: Added tests across 8 crates covering critical gaps (layout, config, buffer, clipboard, core, git, i18n, session)

### Changed
- **Stale-on-Collapse Optimization**: Collapsed panels skip tick/watcher/git/LSP background work; refresh once when expanded again
- **Expanded-Aware Iterators**: Added `iter_all_panels_with_expanded_state_mut()` and `iter_expanded_panels_mut()` to LayoutManager
- **LSP Polling Scoped to Expanded Editors**: Loading status updates only run for visible editor panels

### Fixed
- **CI/Security**: Vendored OpenSSL for cross-platform builds, fixed macOS statvfs types, redacted credentials from logs
- **SFTP Stale Bug**: Remote panels are never marked stale by local fs/git events (prevents broken reconnections)
- **VFS Spinner Hang**: FileManager tick always drains VFS and git-status receivers even when collapsed, preventing stuck spinners
- **VFS Error Draining**: Fixed early return in FileManager tick that prevented VFS error results from being consumed

## [0.11.0] - 2026-02-01

### Added
- **VFS with SFTP Support**: Remote file operations via sftp:// protocol — browse, open, edit, upload, download remote files
- **Unified File Operations System**: OperationManager with pause/cancel support for copy, move, delete, upload, download
- **Operations Panel**: Dedicated panel showing active file operations with progress cards and speed tracking
- **Conflict Resolution**: End-to-end conflict handling — overwrite, skip, rename, overwrite-all, skip-all
- **Byte Progress Bar**: Single-file upload/download progress with byte-level granularity
- **Background Operations Indicator**: Status bar indicator for running file operations
- **Ctrl+C Copy in Terminal**: Copy selected text with Ctrl+C when selection is active
- **Auto-scroll During Selection**: Continuous auto-scroll when dragging mouse beyond editor/terminal viewport
- **Word Wrap Caching**: Cached word wrap functions for faster cursor movement in large files
- **Text Selection in Modals**: Full text selection support in all input fields
- **Word Wrap in Journal**: JournalPanel now supports word-wrapped log entries
- **Version Header in Help**: Help panel displays current version
- **Mouse Scroll Coalescing**: Batched scroll events reduce render cycles during fast scrolling

### Changed
- **i18n Cleanup**: Removed 98 dead translation keys, wired 30 new keys for previously hardcoded UI strings
- **OperationManager Migration**: Migrated local copy, delete, batch upload/download, and directory copy to unified system
- **Logging Migration**: Switched to standard `log` crate macros throughout the codebase
- **Module Decomposition**: Split monolithic `app/mod.rs` into focused submodules (watcher, session, background_ops, etc.)
- **SuggestionInput Component**: Extracted reusable dropdown input widget from bookmark modal
- **Zero-alloc Rendering**: Reduced allocations in rendering hot paths with static buffers
- **Adaptive Tick Rate**: Event loop slows from 24 FPS to 5 FPS after 500ms idle, reducing CPU to near-zero
- **Conditional System Monitor Redraw**: Only redraws status bar when CPU% or memory MB actually changes
- **Watcher Registration**: Split from per-tick polling; registration runs only on panel add/navigate

### Fixed
- **Operations Panel Tracking**: Fixed batch and remote operation tracking in operations panel
- **Conflict Reuse**: Fixed conflict resolution reuse and batch progress tracking
- **Duplicate Downloads**: Prevented duplicate downloads when copying from remote
- **Editor Upload Flow**: Fixed close-after-save and file manager refresh for remote saves
- **Session Panel Widths**: Adapt panel widths to current terminal size on session restore
- **Terminal Duplicate Prompts**: Prevented duplicate prompts during sync_output transitions
- **Editable Select Padding**: Fixed extra empty lines and padding in dropdown
- **VFS Path Sync**: Fixed local directory navigation path synchronization
- **Git Session Restore**: Preserved repository list when restoring session with submodule
- **Unicode Search Highlighting**: Corrected Unicode handling in editor search
- **Spinner CPU Usage**: Throttled spinner redraws to reduce idle CPU
- **Bookmark Navigation**: Fixed dropdown keyboard navigation and mouse click handling

## [0.10.1] - 2026-01-23

### Changed
- **LSP Performance**: Reduced lock contention and reorganized response handling
- **Editor Refactoring**: Extracted mouse handling to separate module
- **Modal Cleanup**: Removed unused panel_index field from SelectOption
- **Theme Colors**: Updated theme color usage in modals and dropdowns

### Fixed
- **Terminal Sync Artifacts**: Defer cache invalidation during sync_output batches to prevent partial frame rendering
- **Terminal Render Clearing**: Clear render area before drawing to prevent visual artifacts
- **Modal Width Stability**: Fixed editable select width during dropdown toggle
- **Terminal Cache**: Added cache invalidation for all buffer-modifying operations

## [0.10.0] - 2026-01-23

### Added
- **Vim Mode**: Full vim keybinding support for Editor panel with Cyrillic keyboard layout support
- **Directory Switcher**: Quick directory navigation modal with Ctrl+P, recent directories, and fuzzy search
- **Bookmark Add Modal**: Smart group input with dropdown suggestions for organizing bookmarks
- **Modal Paste Support**: Paste text directly into modal input fields with control character filtering

### Fixed
- **Terminal Visual Artifacts**: Fixed race condition in sync_output cache invalidation
- **Terminal Batch Rendering**: Force cache invalidation on ED (clear screen) commands
- **Buffer Invariant**: Added ensure_buffer_size() to prevent IL/DL edge cases

### Documentation
- Added documentation for vim mode, directory switcher, and bookmarks features

## [0.9.1] - 2026-01-22

### Added
- **Linux TTY Support**: Native terminal colors for Linux console/framebuffer (TERM=linux)
- **Remote Branch Display**: Git Status panel shows remote branches not available locally
- **DECSTBM Scroll Regions**: Proper CSI scroll region support for terminal applications

### Changed
- **Terminal Refactoring**: Extracted utility modules (clipboard, disk_space, link_detection, shell_utils)
- **Editor Refactoring**: Decomposed core.rs into focused modules (LSP, movement, search, text)
- **Git Refactoring**: Split monolithic lib.rs into command, commits, files, operations modules
- **Performance**: Cached regex patterns for link detection, reduced panel code duplication
- **DRY Improvements**: Extracted helpers for file/terminal opening across panels

### Fixed
- **Synchronized Output**: Fixed cache invalidation when exiting sync_output mode (CSI ? 2026 l)
- **CI Release Notes**: Fixed extraction of release notes from CHANGELOG.md

### Performance
- **Terminal**: Reduced SSH input latency and htop rendering overhead

## [0.9.0] - 2026-01-20

### Added
- **LSP Integration**: Full Language Server Protocol support with completion, diagnostics, hover, and go-to-definition
- **Git Diff Panel**: GitHub-style diff viewer with commit diff viewing from Git Log panel
- **Save As Modal**: New dialog with executable checkbox for setting file permissions (Ctrl+Shift+S)
- **Report Scripts**: Scripts with `.report.` suffix run in background and show output in modal dialog
- **Runtime Language Switching**: Change UI language on the fly with 15 language support
- **Dynamic Help Panel**: Replaced static WelcomePanel with context-aware help
- **Git Status Enhancements**: Init button when no repository, left-ellipsis path truncation, enhanced file properties
- **Terminal Features**: File path detection, multi-line link highlighting, bracketed paste support
- **Editor Features**: Disk space info in status bar, improved PageUp/PageDown behavior
- **UI Improvements**: Smart title truncation with Unicode ellipsis, unified animated spinners

### Changed
- **Menu Renamed**: "Tools" → "Windows", "Actions" → "Scripts" for better UX
- **Keybindings**: Ctrl+Shift+S now opens Save As (was Force Save)
- **Submenu Renamed**: "Manage actions" → "Manage scripts"
- **Git Diff Style**: Unified file headers with termide design system
- **Refactoring**: Extensive code cleanup with Command Pattern, state extraction, and helper consolidation

### Fixed
- **i18n**: Format keys moved to correct section, warnings redirected to journal
- **Git**: Submodule detection by parsing .gitmodules
- **Copy/Move Modals**: Removed duplicate text

### Documentation
- **LSP Docs**: Added comprehensive LSP documentation with configuration examples
- **Scripts Docs**: Updated for new menu names and .report. suffix feature
- **Editor Docs**: Added Ctrl+Shift+S Save As keybinding

## [0.8.9] - 2026-01-15

### Added
- **Terminal URL detection**: Ctrl+hover highlights URLs (cyan + underline), copies to clipboard, Ctrl+click opens in browser
- **ImagePanel reuse**: Browsing images reuses existing panel instead of creating new ones
- **Git async operations**: Push/Pull run in background with spinner indicator and result modal

### Fixed
- **Modal UX**: InfoModal closes only on Enter, Escape, or button click (not any key press)
- **Modal display**: Git operation results show actual output without "Error:"/"Status:" labels
- **Unicode click detection**: Fixed button click detection for Unicode text using grapheme width
- **Git TUI corruption**: Stdout/stderr piped to prevent git output from corrupting terminal display

### Changed
- **Git spinner location**: Moved from status bar to GitStatusPanel for better visibility during panel switches
- **UI cleanup**: Consolidated duplicate code and removed dead code
- **Menu structure**: Restructured menu and fixed log viewer truncation

## [0.8.8] - 2026-01-13

### Security
- **lru vulnerability**: Fixed GHSA-rhfx-m35p-ff5j (IterMut Stacked Borrows violation) by updating to lru 0.16.3

### Added
- **Configurable keybindings**: Full keybinding customization via config.toml for all panels
- **Shared abstractions**: New ClickTracker, Viewport, SelectionStyle utilities for DRY code

### Changed
- **Dependencies**: Updated ratatui 0.29→0.30, ratatui-image 5→10
- **Performance**: 50-500x speedup in hot paths (grapheme iteration, git diff markers, LRU eviction, gitignore checks)
- **Refactoring**: Integrated ClickTracker into GitStatusPanel and FileManager (~40 LOC saved)

## [0.8.7] - 2026-01-12

### Added
- **Install script**: Nix installation method with `nix profile install` for NixOS users
- **Install script**: Nix shown as "(recommended for NixOS)" on NixOS systems

### Fixed
- **Install script**: Works correctly when piped via `curl | sh` (reads from /dev/tty)
- **Install script**: Fixed download URLs (removed incorrect `v` prefix from version)
- **Install script**: Fixed ANSI escape codes appearing literally in success message
- **Install script**: Added `--refresh` flag to ensure latest version is installed via Nix

### Changed
- **Build**: Enabled LTO and strip for release builds (-14% binary size: 29→25 MiB)
- **Packaging**: Removed obsolete help/themes install steps (files are embedded in binary)
- **Nix**: Track Cargo.lock for reproducible flake builds

## [0.8.6] - 2026-01-11

### Added
- **i18n**: 25 missing translation keys for Git panel and preferences in 7 languages (de, es, fr, hi, pt, th, zh)

### Fixed
- **Theme dropdown**: Mouse clicks now work correctly for all items (not just first 12)
- **Terminal paste**: Removed explicit flush preventing React/Ink TUI app overflow errors (e.g., Claude Code)

### Changed
- **README**: Added "Why TermIDE?" comparison table and quick navigation links
- **README**: Fixed theme screenshot paths

## [0.8.5] - 2026-01-07

### Added
- **Universal install script**: New `install.sh` for easy cross-platform installation

### Fixed
- **Config hot-reload**: Editor now applies settings (theme, word_wrap, tab_size) when saving config.toml
- **UTF-8 paths**: Git Status panel correctly handles non-ASCII file paths in truncation

### Changed
- **Git repo discovery**: Repositories now discovered from panel paths instead of project root
- **Performance**: O(n²) → O(n log n) optimization in nested paths removal algorithm
- **Performance**: Reduced allocations in search replace using `take()` instead of `clone()`
- **Code quality**: Extracted helper methods in GitStatusPanel (git operations, rendering, navigation, mouse handling)
- **Code quality**: Removed unused `SearchDirection` and `Action::Group` from buffer module

## [0.8.4] - 2026-01-06

### Added
- **CI/CD automation**: Automatic AUR and Homebrew updates on release via GitHub Actions
- **AUR packages published**: `termide` (source) and `termide-bin` (binary) now available on AUR

### Fixed
- **Terminal text selection**: Selection now uses absolute buffer coordinates
- **Selection persistence**: Selection follows text when scrolling through scrollback
- **Ctrl+Shift+C**: Keyboard shortcut for copying terminal selection
- **Selection cleanup**: Selection cleared on keyboard input
- **Trailing whitespace**: Preserved when copying selected text

## [0.8.3] - 2026-01-01

### Fixed
- CI: Use Ubuntu 22.04 for Linux builds to ensure glibc compatibility with Debian 12

## [0.8.2] - 2026-01-01

### Added
- **Inline diff visualization**: Character-level diff highlighting for modified lines in editor (deleted text in red, inserted in green)
- **Git gutter markers**: Visual indicators in editor line numbers (`+` for added, `~` for modified lines)
- **Revert button**: Quick revert action for individual files in Git Status panel

### Fixed
- Git commands now properly detect repository when editing files in subdirectories
- Inline diff line mapping correctly tracks modified lines when other lines are added/deleted above
- Debian package (.deb) now includes binary in `/usr/bin/`
- README.md download links corrected for .deb and .rpm packages

## [0.8.1] - 2025-12-28

### Added
- **Alt+G hotkey**: Quick access to open/focus Git Status panel
- **Git Status navigation**: Spatial navigation with Up/Down between sections, Left/Right within rows
- **Git Status sticky headers**: Staged/Unstaged headers stay visible when scrolled
- **CommitModal improvements**: PageUp/PageDown support, mouse scroll in textarea
- **Terminal Shift+Enter**: Insert newline for multi-line input

### Fixed
- Git Status panel: PageDown now scrolls fully to show all files
- Terminal: Shift+Enter sends newline instead of CSI u escape sequence
- Editor: Mouse scroll no longer jumps back to cursor position
- CommitModal: Textarea border now uses correct accent color

### Documentation
- Added Shift+Enter shortcut to terminal documentation (EN/RU)
- Added Git panel menu item and Alt+G shortcut to UI docs (EN/RU)
- Updated help files for all 9 languages with Git panel shortcuts

## [0.8.0] - 2025-12-27

### Added
- **Git Status Panel**: Full git status panel with staged/unstaged files, commit/push/pull actions
- **Git Log Panel**: View commit history with navigation
- **Sessions Menu**: New/switch/change-root session actions via Ctrl+S menu
- **CommitModal**: Multi-line textarea for entering commit messages with Commit/Cancel buttons
- **TextArea component**: Multi-line text input with 2D cursor, selection, undo/redo, clipboard support
- **Input field improvements**: Text selection, undo/redo (Ctrl+Z/Y), clipboard support in modal input fields

### Fixed
- Modal input fields: UTF-8 handling, clipboard operations, text selection
- Collapsed panel titles: now show truncated title instead of empty string
- Git Status panel title: properly displays repo name, branch, and status indicators

### Changed
- Git Status panel: spinner icon during refresh, status numbers moved to panel title
- InlineSelector component: improved styling and interaction

## [0.7.0] - 2025-12-18

### Added
- **ImagePanel**: Native image preview using terminal graphics protocols (Kitty, Sixel, iTerm2)
- **Panel resize**: Drag-and-drop panel border resizing with mouse
- **F3 key**: View file without executing (executables open in editor)
- **Shift+Enter**: Force open file with system application (xdg-open)
- **Execute on Enter**: Run executable files directly in terminal
- Smart file type detection: raster images → ImagePanel, vector/video/binary → xdg-open

### Fixed
- Sessions list: cursor now positions on current session
- Editor: off-by-one error in smart word wrap
- SaveAs dialog: now uses file manager's working directory

### Changed
- Split `min_panel_width` config into separate parameters for finer control
- Binary file detection extracted to shared utility (`core::util::is_binary_file`)

### Documentation
- Updated file manager docs with new keyboard shortcuts (F3, Shift+Enter)
- Updated help screens for all 9 languages

## [0.6.1] - 2025-12-16

### Added
- Live theme preview: theme changes on cursor navigation in theme selection menu
- Theme restored on cancel (Esc), saved on confirm (Enter)

### Fixed
- Menu toggle on repeated click (Preferences dropdown now closes on second click)
- Nested submenu toggle (Themes menu closes on repeated click)
- i18n: pluralization for time_*_ago functions (fixed "{plural}" appearing in session list)

### Performance
- Git status: optimized from 6 to 2 git process spawns per call (66% reduction)
- RenamePattern::apply(): reduced allocations using Vec<&str> instead of Vec<String>

### Refactoring
- Extracted CursorNavigation trait for modal cursor navigation (DRY, -70 LOC)
- Extracted git_command helpers to reduce duplication
- DRY improvements in system-monitor and status_bar
- Removed dead code across multiple crates (~300 LOC)

## [0.6.0] - 2025-12-16

### Added
- File search modal (Ctrl+Shift+P) with async glob pattern matching
- Content search modal (Ctrl+Shift+F) with regex support for searching text in files
- Theme selection UI with nested dropdown menu in Preferences
  - Color preview for each theme showing bg/fg and accent colors
  - Immediate theme application and config persistence
- Theme refactoring: `all_theme_names()` and `get_by_name()` API for theme system

### Fixed
- Syntax highlighting fallback to theme foreground color for light themes
- Session modal: current session now highlighted with accent color and selectable
- Session modal and file search UX improvements

### Documentation
- Added file search (Ctrl+Shift+P) and content search (Ctrl+Shift+F) shortcuts
- Updated theme documentation with new menu-based selection method

## [0.5.4] - 2025-12-15

### Added
- Session switching modal (Alt+M → Sessions) to switch between open termide sessions
- Static loading indicator for file watcher initialization (replaced animated Braille spinner)

### Fixed
- Word wrap cursor positioning unified calculation
- Mouse selection offset with word wrap enabled
- Disk space indicator formatting (removed space before unit: "411/467GB")

### Performance
- File watcher now uses `ignore` crate to skip .gitignored directories

### Documentation
- Updated project structure in README and architecture docs to reflect workspace crates
- Updated i18n section to show all 9 supported languages
- Added Ctrl+D (duplicate line) to editor help files

## [0.5.3] - 2025-12-15

### Fixed
- Git status color flickering on directory reload (preserve cache during refresh)
- `.git` directory date not updating on commit when viewing repo root
- Single mouse click copying character to clipboard in terminal (now only drag selection copies)
- File manager git status update timing and CPU format display in menu bar

### Changed
- Unified git and filesystem watchers into single `UnifiedWatcher` with separate debounce intervals (300ms files, 1000ms git)

### Performance
- Optimized rendering and reduced memory allocations

## [0.5.2] - 2025-12-14

### Fixed
- Terminal scroll not working during user input wait (scroll events now bypass modal check)
- i18n placeholders showing '{}' instead of values (all 9 language files updated with named placeholders)

### Changed
- Removed duplicate `/i18n/` folder from project root (was unused copy)
- Moved `/help/` files to `crates/panel-misc/help/` for better code organization

## [0.5.1] - 2025-12-13

### Changed
- Terminal panel performance optimizations
  - Use `has_pending_output()` in event loop for efficient redraw triggering
  - Pre-allocate spans Vec for reduced allocations
  - Use O(1) VecDeque methods for row 0 operations
- Remove crates.io publishing (distribution via GitHub Releases, deb/rpm only)

### Fixed
- Git status display in file info modal now shows actual change count
- File manager cursor resets to position 0 when entering subdirectory
- Version consistency across modal, panel-editor, ui-render crates

## [0.5.0] - 2025-12-13

### Added
- Editor Tab key insertion and block indent/unindent (Tab/Shift+Tab)
- External file change detection with reload/close dialog
- Log viewer panel with Editor-based rendering (cursor, selection support)
- Mouse scroll based on cursor position, not panel focus
- Recursive file watching for git repositories

### Changed
- **Major architecture refactoring**: extracted 31 workspace crates from monolithic src/
  - Modular crate structure: app, app-core, app-event, app-modal, app-panel, etc.
  - Improved build times with incremental compilation
  - Better code organization and separation of concerns
- Type-safe panel communication architecture
  - `PanelCommand` enum replacing unsafe `dyn Any` downcasting
  - `CommandResult` enum for type-safe command responses
  - `handle_command()` method on Panel trait
- Modal dialog code consolidation
  - Shared frame rendering with [X] close button
  - Shared input field rendering with cursor
  - Reduced code duplication in search/replace modals
- Config restructured to nested TOML sections
- Async git diff computation to prevent UI freeze
- Gitignore-aware filesystem updates

### Fixed
- File manager cursor position preserved after file deletion
- Copy/paste operations no longer cause unwanted scroll
- Local timezone display for file modification times
- File sizes rounded to whole units and right-aligned
- Git diff deduplication to prevent UI freeze
- FS watcher feedback loops and deleted files display

### Performance
- Async git diff and gitignore-aware FS updates
- Cache `find_repo_root()` and throttle spinner
- Conditional redraw to reduce idle CPU usage
- Memory optimization: removed redundant clones

### Tests
- PanelCommand and CommandResult unit tests (7 tests in core)
- Editor handle_command integration tests (9 tests)
- FileManager handle_command integration tests (8 tests)
- Modal base module tests (4 tests)
- Large file handling tests (7 tests)
  - 10K+ line file loading and navigation
  - Scroll performance benchmark (50K lines in <100ms)

### Code Quality
- Zero TODO/FIXME comments in production code
- All 5 unsafe blocks documented with SAFETY comments
- Replaced 21+ critical `unwrap()` calls with `expect()` + context messages
- Fixed error swallowing in Editor `handle_key()` - errors now shown to user
- Removed dead code: unused `RequestDirSize` event and `PanelProvider` trait methods
- Optimized `get_selected_text()` - no longer copies entire buffer for large selections

## [0.4.0] - 2025-12-07

### Added
- Double-click word selection in editor
  - Select word between nearest delimiters with double-click
  - Proper handling of alphanumeric word boundaries
- Smart word wrapping with word boundary detection
  - Breaks lines at word boundaries when possible
  - Falls back to hard break for words wider than viewport
- Visual line navigation for word-wrapped text
  - Cursor Up/Down moves through visual lines, not buffer lines
  - Preserves preferred column across visual line movements
- Proper Unicode rendering for CJK and combining characters
  - Chinese/Japanese/Korean characters display with correct 2-column width
  - Hindi and other scripts with combining characters render correctly
  - Uses grapheme clusters for proper text segmentation
- Localization for 7 new languages
  - German (de), Spanish (es), French (fr), Hindi (hi)
  - Japanese (ja), Korean (ko), Portuguese (pt), Thai (th), Chinese (zh)
- Panel reordering hotkeys
  - Alt+[ and Alt+] to reorder panels within current group
  - Alt+PageUp/PageDown context-aware (reorder or switch)
- Kitty keyboard protocol support for proper Alt+Cyrillic handling

### Changed
- Major editor architecture decomposition
  - Extracted cursor movement to dedicated modules (physical, visual)
  - Separated rendering into focused modules (line, wrap, cursor, highlights)
  - Created RenderContext for shared rendering state
  - Keyboard handling with Command Pattern
- Translations migrated from Rust code to TOML files
  - Easier to add/update translations
  - Cleaner separation of concerns
- Terminal VT100 parser extracted to dedicated module
- Extensive code cleanup and DRY refactoring
  - Extracted TextInputHandler for modal inputs
  - Created path_utils module for path resolution
  - Added panel downcast helpers (PanelExt trait)

### Fixed
- Cursor Up/Down navigation with word wrap
- Git status tracking in subdirectories
- Editor navigation and viewport issues with word wrap
- App initialization with correct terminal size

### Performance
- Critical hot path optimizations (100-270x faster)
  - Vec pre-allocation when size is known
  - Eliminated unnecessary string allocations
  - Optimized terminal character shift with copy_within

## [0.3.0] - 2025-12-04

### Added
- Release management Claude Code skill for automated release workflow
  - Pre-release quality checks (fmt, clippy, test, build)
  - Multi-source change analysis (uncommitted, commits, file states)
  - Interactive version selection with validation
  - Automated version updates across 12+ files
  - Auto-generated CHANGELOG entries from git history
  - Post-update quality verification
  - Conventional commit generation and git tag creation
- XDG Base Directory Specification support
  - Config: `~/.config/termide/` (or `$XDG_CONFIG_HOME/termide/`)
  - Data: `~/.local/share/termide/` (or `$XDG_DATA_HOME/termide/`)
  - Cache: `~/.cache/termide/` (or `$XDG_CACHE_HOME/termide/`)
  - Proper cross-platform paths (Linux, macOS, Windows)
- Automatic session persistence with configurable retention
  - Sessions save automatically on focus loss (debounced)
  - Auto-cleanup of sessions older than configured retention period (default: 30 days)
  - Per-project session storage
  - Unsaved buffer persistence across sessions
- Comprehensive project documentation
  - CHANGELOG.md with full version history (0.1.0 to 0.2.0)
  - CONTRIBUTING.md with development guidelines
  - Updated issue templates
  - Revised security policy
  - Contributor Covenant Code of Conduct

### Changed
- **BREAKING**: FileManager is now a regular closable panel
  - Removed special fixed left panel handling
  - FileManager can be closed, resized, and moved between groups
  - Default initialization with 2 FileManager panels (50/50 layout)
  - Simplified architecture (-350 lines of code)
  - All panels are now first-class citizens with identical capabilities
  - Existing sessions will load with default layout

## [0.2.0] - 2025-12-04

### Added
- Comprehensive logging system with configurable levels (debug, info, warn, error)
- Real-time git diff visualization in editor line numbers
  - Display uncommitted changes with color-coded line numbers
  - Show deletion markers with count on horizontal lines
  - In-memory diff computation with debounced updates (300ms)
  - Localized deletion marker text (English/Russian)
- Configurable word wrap option in editor (enabled by default)
- Per-project session storage with automatic unsaved buffer persistence
- Automatic cleanup of old sessions (configurable retention period, default 30 days)
- New configuration options:
  - `word_wrap` - Enable/disable word wrap in editor
  - `min_log_level` - Minimum logging level (debug, info, warn, error)
  - `session_retention_days` - Session cleanup retention period
  - `show_git_diff` - Toggle git diff visualization
  - `fm_extended_view_width` - Minimum width for file manager extended view

### Changed
- Rewrite clipboard system using arboard library
  - Support both CLIPBOARD and PRIMARY selections on Linux
  - More reliable cross-platform clipboard operations
- Improve cursor rendering across all panels
  - Use inverse colors instead of theme selection colors
  - Better visual contrast and cursor visibility
  - Handle reverse attribute correctly in terminal panels
- Improve config auto-completion with hash-based detection
  - Automatically detect ALL missing config keys
  - No manual maintenance of required_keys array
  - Correctly handle optional fields
  - Normalize config file format

### Fixed
- Word wrap rendering bug causing single-line display
- Cursor going off-screen with git diff deletion markers
- Viewport calculations now account for virtual lines (deletion markers)
- Empty line rendering in word wrap mode
- Modified flag after undo/redo operations
- File manager navigation now remembers directory when going up
- Old API tests marked as ignored to fix CI

## [0.1.5] - 2025-12-02

### Fixed
- Package build issues for .deb and .rpm on ARM64 architecture
  - Removed aarch64 from package build matrices to avoid cross-compilation issues
  - Binary tarballs still support all platforms including ARM64

### Added
- Local package build testing script (`scripts/test-packages.sh`)

## [0.1.4] - 2025-12-02

### Added
- Package manager distribution support:
  - Debian/Ubuntu packages (.deb)
  - Fedora/RHEL/CentOS packages (.rpm)
  - Arch Linux AUR packages (source and binary variants)
  - Homebrew formula for macOS/Linux
- Enhanced Nix Flake support:
  - Add `packages.default` output with `rustPlatform.buildRustPackage`
  - Add `apps.default` for `nix run` support
  - Add `overlays.default` for nixpkgs integration
  - Install help files and themes in postInstall phase
- Automatic config file completion for new configuration keys
- Comprehensive installation documentation for all package managers

### Changed
- Reorganize README installation section with collapsible details blocks
- Use emoji icons for better visual navigation in documentation
- Update GitHub Actions workflow to build .deb and .rpm packages automatically

### Fixed
- GitHub Actions workflow dependencies (crates.io publication now requires all builds to succeed)
- Binary paths in cargo-deb and cargo-generate-rpm for cross-compilation
- Fail-fast strategy in build matrices to see all failures

## [0.1.3] - 2025-12-02

### Added
- **Accordion panel system** - Major architectural improvement
  - Smart panel stacking based on terminal width
  - Vertical accordion layout within horizontal groups
  - One expanded panel per group, others collapse to title bar
  - Configurable minimum panel width threshold (80 characters)
- New navigation hotkeys:
  - `Alt+Up/Down` - Navigate panels within group
  - `Alt+PgUp/PgDn` - Move panel to previous/next group
  - `Alt+Home/End` - Move panel to first/last group
  - `Alt+Plus/Minus` - Increase/decrease active group width
  - `Alt+Backspace` - Toggle panel stacking (merge/unstack)
- Developer documentation (`doc/en/architecture.md`, `doc/en/developer-guide.md`)

### Changed
- Complete panel layout system redesign
  - New `LayoutManager` for centralized panel group management
  - New `PanelGroup` for vertical panel stacking
  - Separate panel rendering logic
- Panel width management improvements
  - Fix redistribute after group deletion (8 locations)
  - Add zero-sum balance correction for resize operations
  - Fix auto-stacking calculation to use average width
  - Add proportional width redistribution across all groups

### Removed
- **BREAKING**: Removed `LayoutMode` (SimplePanel/MultiPanel) in favor of dynamic groups
- **BREAKING**: Changed panel navigation model from flat to hierarchical (groups + panels)

## [0.1.2] - 2025-11-29

### Added
- Duplicate line/selection feature (`Ctrl+D`)
- Replace operation feedback ("Replaced N occurrence(s)" message)
- File size validation before opening in editor (100 MB limit)
- Configurable tab size support (reads from `config.toml`)
- Crates.io publishing in release workflow

### Changed
- Migrate to semantic versioning tags without 'v' prefix (e.g., `0.1.2` instead of `v0.1.2`)
- Update license in Cargo.toml from dual (MIT OR Apache-2.0) to MIT only
- Improve error handling across the application
  - Replace panic with graceful error handling in theme parsing
  - Falls back to hardcoded default theme on parse errors
  - Better mutex error handling in terminal (22 `.lock().unwrap()` replaced with `.expect()`)
- Add clear error messages for oversized files

### Fixed
- Application crashes from invalid theme files
- Editor tab size now respects user configuration

## [0.1.1] - 2025-11-25

### Added
- Interactive search modal (`Ctrl+F`) with live preview and match counter
- Interactive replace modal (`Ctrl+H`) with dual input fields
- Tab/Shift+Tab navigation in search mode
- State preservation for search/replace queries
- `[X]` close button on editor panels
- Arrow key navigation between fields in replace modal

### Fixed
- Replace operation skipping matches on same line
- Inconsistent cursor positioning (now always at end of match with selection)
- Prev/Next navigation buttons resetting search state
- Escape key behavior (closes search first, then panel)
- Enter key in replace modal (now replaces instead of deleting)

### Changed
- Update match positions after replacement operations
- Standardize cursor positioning across all search/replace operations

## [0.1.0] - 2025-11-25

### Added
- Initial TermIDE release with complete feature set
- Terminal-based IDE with syntax highlighting for 15+ programming languages
  - Rust, Python, JavaScript, TypeScript, Go, C/C++, Java, Ruby, PHP
  - Haskell, Nix, HTML, CSS, JSON, TOML, YAML, Bash, Markdown
- Smart file manager with intuitive TUI interface
  - File type icons with attributes column
  - Symlink and executable file detection
  - Advanced keyboard and mouse selection controls
  - Recursive git status for directories
- Integrated virtual terminal with full PTY support
  - Ctrl+Shift+V paste with bracketed paste mode
  - 24 FPS rendering
  - Scrollback buffer and ANSI color support
- Multi-panel layout system
- Git integration
  - Background git status monitoring with file watching
  - Automatic updates on repository changes
  - Color-coded status indicators
  - Dimmed styling for gitignored files
  - Support for repository subdirectories
- 12 built-in themes
  - Dark: Default, Midnight, Dracula, OneDark, Monokai, Nord, Solarized Dark
  - Light: Atom One Light, Ayu Light, GitHub Light, Material Lighter, Solarized Light
  - Custom theme support from config directory
- System resource monitoring
  - Real-time CPU and RAM usage
  - Color-coded alerts
- Multi-language support (English, Russian)
  - Full Cyrillic keyboard layout support
  - Case-preserving hotkey translation
- Mouse support for all panels and UI elements
- Clipboard system for cut/copy/paste operations
- Batch file operations (copy, move, delete)
- Quit confirmation for unsaved changes and running processes
- Robust error handling and file size limits
- Automatic directory refresh on filesystem changes
- Cross-platform support (Linux, macOS, Windows via WSL)
- Multi-architecture builds (x86_64, ARM64)

### Technical
- Built with Rust using ratatui TUI framework
- Crossterm for cross-platform terminal manipulation
- Portable-pty for PTY implementation
- Tree-sitter for syntax highlighting
- Ropey for text buffer management
- Sysinfo for system resource monitoring
- Pre-commit hooks for code quality
- Comprehensive test suite
