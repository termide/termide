# Remote Filesystems (VFS)

TermIDE includes a pure-Rust VFS layer that lets the [file
manager](file-manager.md) browse remote servers as if they were local
directories. Files can be opened in the editor, copied between local
and remote panels, renamed, and so on — the same UI, with progress
shown in the [Operations panel](operations.md).

No native libraries are required: SFTP runs on `russh` + `russh-sftp`,
FTPS on `rustls`. Builds work statically on Alpine / musl out of the
box.

## Supported protocols

| Scheme  | Protocol                                          |
|---------|---------------------------------------------------|
| `sftp://` | SSH File Transfer Protocol over SSH             |
| `ftp://`  | Plain FTP                                       |
| `ftps://` | FTP over TLS (rustls)                           |

`smb://` and `nfs://` are recognised by the URL parser but the
providers behind them are not currently shipped — use them via the
operating system's native mount instead.

## URL syntax

A VFS URL has the form:

```
scheme://[user[:password]@]host[:port][/path]
```

Examples:

```
sftp://nvn@example.com/home/nvn/projects
sftp://example.com:2222/srv/builds
ftp://files.example.com/pub
ftps://secure.example.com/uploads
```

Notes:
- Non-ASCII characters in `path` are accepted; the parser
  percent-decodes them back to UTF-8 before talking to the server.
- Omitting the user makes the SFTP provider fall back to your SSH
  config (see Authentication).
- Omitting the port uses the protocol default (22 / 21 / 990).
- A password in the URL is not used. When a server asks for one, TermIDE
  prompts for it and can keep it in the
  [password vault](passwords.md); for SFTP, key-based authentication
  remains the better choice.

## Opening a remote location

Two ways to get a remote panel open:

1. **Go to path** — open the file manager's `Go to path` input and
   paste / type a VFS URL.
2. **Bookmarks** — store frequently-used remotes once and reach them
   through the `Bookmarks` menu. Bookmark entries accept the same URL
   syntax as above; they're stored in
   `~/.config/termide/bookmarks.toml` (or the project-local
   `.termide/bookmarks.toml` if you keep team-shared shortcuts in the
   repo).

Both paths land you in a normal file manager view rooted at the
remote directory; everything else (tree expand, copy, rename,
properties on Space) is exactly the same as for local panels.

## Authentication (SFTP)

SFTP supports four authentication modes:

- **`Auto`** *(default)* — tries SSH agent first, then keys listed in
  `~/.ssh/config` for the host (including `IdentityFile`, `User`,
  `Port` and `Hostname` aliases), then default keys
  (`id_ed25519`, `id_rsa`, `id_ecdsa`, `id_dsa`), then password.
- **SSH agent** — uses an SSH agent if `SSH_AUTH_SOCK` points at one.
- **SSH key** — explicit private-key file, optionally with a
  passphrase.
- **Password** — when the methods above are refused, TermIDE takes the
  password from the [password vault](passwords.md) or asks for it.

FTP and FTPS log in with the user from the URL; a refused login asks for
the password the same way. Without a user in the URL the login is
anonymous.

Because `Auto` reads `~/.ssh/config`, you can keep the bookmark URL
plain (`sftp://my-build-host/path`) and let SSH config supply the
real hostname, user and key file. Same configuration as the `ssh`
CLI uses.

## How remote panels feel

Remote operations are all asynchronous:

- The first directory listing shows a brief spinner; entries appear
  when the server replies.
- Expanding a subdirectory inserts a tiny `…` placeholder; once the
  listing arrives it's swapped for the real children.
- Tree state, cursor and selection are preserved across reloads.

Wide-view directory sizes are **not** computed for remote panels —
the cost of walking a remote tree just for a column would dwarf the
benefit. Remote size always shows blank.

## Transferring files

Pick files in one panel, hit `C` / `F5` (copy) or `M` / `F6` (move),
and choose a destination — either a local path or another remote
panel. The transfer registers as an operation in the
[Operations panel](operations.md), with:

- A real progress bar (bytes + files), updated from the worker on
  each chunk.
- Pause / Resume that actually stops the byte stream (paused
  uploads / downloads sit idle on the worker side; the SFTP actor
  stays free for other panels' metadata requests).
- Cancel that stops cleanly between chunks. If the cancelled
  operation left a partial file on the server, the panel asks
  whether to delete it — see the cancel-cleanup section in
  [Operations panel](operations.md).

Same-host SFTP and FTP renames stay on the server: a move within one
connection issues a remote-side rename, not download-then-upload.

## Projects

A project layout that includes a remote file manager panel persists the
URL just like a local path. On the next start TermIDE reconnects in the
background and shows the panel with a loading placeholder until the
listing arrives — the rest of the UI is responsive immediately.

### Dropped connections

If the remote session is lost (idle timeout, network drop), the next
operation fails and the panel shows a recovery dialog with three choices:
**Reconnect** (open a fresh session to the same path), **Open home
(local)** (drop the connection and switch the panel to your local home
directory), or **Close panel**. Pressing `Esc` dismisses the dialog and
leaves the panel on its last listing. The panel never loops on the error.

## Archives

Archive files open in the file manager like directories: `Enter` on
one lists its contents, and `..` (or `Backspace`) at its root returns
to the directory holding it, with the cursor back on the archive.
`F3`, `F4` and `O` still treat the archive as a file.

| Format | Names |
|--------|-------|
| ZIP    | `.zip`, `.jar`, `.war`, `.apk`, `.whl` |
| tar    | `.tar`, `.tar.gz` / `.tgz`, `.tar.bz2` / `.tbz2`, `.tar.xz` / `.txz`, `.tar.zst` / `.tzst` |
| ISO 9660 | `.iso` |

The format is detected from the file's content; the name only decides
which files `Enter` opens this way. Opening reads the archive's table
of contents once — for a large compressed tar that takes a pass over
the whole file, and `Esc` cancels it. If the archive changes on disk
while it is open, the next access reads it again.

An archive in a remote panel or inside another archive opens the same
way: it is first copied whole into a temporary directory (the status
line shows `Opening …` meanwhile), and the copy is deleted when the
archive is left. Changes to the remote file while it is open are not
noticed.

An encrypted ZIP (ZipCrypto or AES) asks for its password when it is
opened, and again after a wrong one. The password is checked by
decrypting the smallest encrypted entry and is kept only while the
archive stays open.

ZIP names are read as UTF-8 whenever they are valid UTF-8 (macOS writes
them so without saying). Older archives from Windows store names in the
system's code page; one whose names look like Cyrillic in CP866 is read
as CP866, anything else as CP437, the encoding the ZIP format names.

An ISO image shows its Rock Ridge tree (POSIX names, permissions and
symbolic links) when it has one, otherwise its Joliet tree (long
Unicode names), otherwise the bare ISO 9660 names such as `README.TXT`.
UDF is not read: an image that keeps its files only in UDF — Windows
installation images, DVD-Video, Blu-ray — shows just the small ISO 9660
part next to it, usually a lone `README.TXT`.

Archives are **read-only**. Opening a file from one works as for a
remote file (a temporary copy), and copying with `C` / `F5` to a
local panel extracts the selected files and directories, with the
usual progress, pause and cancel in the
[Operations panel](operations.md). Creating, deleting, moving and
renaming inside an archive are refused with a notice.

Extraction never writes outside the destination: entries named with
`..` or an absolute path are not shown, a symbolic link is created only
when its target stays inside the extracted tree (and only after all
files are written), and an entry that unpacks to more than its declared
size fails instead of filling the disk.

The URL of a location inside an archive is
`archive://<archive file>!<path inside>`, e.g.
`archive:///home/nvn/src.tar.gz!/docs`; it works in `Go to path` and in
saved projects like any other VFS URL.

### Packing

`P` packs the selected files and directories (or the entry under the
cursor) into a new archive. The prompt suggests `<name>.zip` next to
the selection and lists the other formats in its dropdown; the name's
suffix picks the format — `.zip`, `.tar`, `.tar.gz`, `.tar.bz2`,
`.tar.xz` or `.tar.zst`. A relative path is taken from the selection's
directory, and a directory gets an archive named after the selection
inside it.

Packing runs in the [Operations panel](operations.md) with progress,
pause and cancel. Symbolic links are stored as links, not followed;
permissions and modification times are kept. The archive is written
to a temporary file and appears under its name only when complete, so
a cancelled or failed run leaves nothing behind, and an existing file
is never overwritten. Only local files can be packed.

## Limitations / known gaps

- No `smb://` / `nfs://` provider yet — only URL parsing.
- No resume-from-byte-offset for interrupted transfers: cancelled
  uploads start over from the beginning if re-issued.
- The passphrase of an encrypted SSH key is not asked for in remote
  panels; load the key into your SSH agent.
