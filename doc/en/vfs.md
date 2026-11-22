# Remote Filesystems (VFS)

Termide includes a pure-Rust VFS layer that lets the [file
manager](file-manager.md) browse remote servers as if they were local
directories. Files can be opened in the editor, copied between local
and remote panels, renamed, and so on — the same UI, with progress
shown in the [Operations panel](operations.md).

No native libraries are required: SFTP runs on `russh` + `russh-sftp`,
FTPS on `rustls`, SMB on `smb2`, NFS on `nfs3_client`. Builds work
statically on Alpine / musl out of the box.

## Supported protocols

| Scheme  | Protocol                                          |
|---------|---------------------------------------------------|
| `sftp://` | SSH File Transfer Protocol over SSH             |
| `ftp://`  | Plain FTP                                       |
| `ftps://` | FTP over TLS (rustls)                           |
| `smb://`  | SMB 2 and 3: Windows, Samba and NAS shares      |
| `nfs://`  | NFS version 3                                   |

See [SMB and NFS](#smb-and-nfs) for what is particular to those two.

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
smb://bob@nas.local/media/films
nfs://nas.local/srv/backup
```

Notes:
- Non-ASCII characters in `path` are accepted; the parser
  percent-decodes them back to UTF-8 before talking to the server.
- `%`, `#` and `?` in a path, and `@` or `:` in a user name, are
  written percent-encoded: `%25`, `%23`, `%3F`, `%40`, `%3A` — e.g.
  `sftp://me%40corp.example@host/srv/issue%231`. Termide writes server
  URLs this way itself.
- Omitting the user or port makes SFTP take them from your SSH
  configuration (see [SSH configuration](#ssh-configuration-sftp)).
- Otherwise, omitting the port uses the protocol default (22 / 21 / 990
  / 445); NFS asks the server's portmapper.
- A password in the URL is not used. When a server asks for one, Termide
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
   `~/.config/termide/bookmarks.toml` on Linux, `~/Library/Application Support/termide/bookmarks.toml` on macOS, `%APPDATA%\termide\bookmarks.toml` on Windows (or the project-local
   `.termide/bookmarks.toml` if you keep team-shared shortcuts in the
   repo).

Both paths land you in a normal file manager view rooted at the
remote directory; everything else (tree expand, copy, rename,
properties on Space) is exactly the same as for local panels.

## Authentication (SFTP)

SFTP logs in the way `ssh` does, and the
[SSH configuration](#ssh-configuration-sftp) of the host chooses how:

1. **SSH agent** — the one `IdentityAgent` names (a socket path, or
   `$VAR` for the socket in that variable), else the one
   `SSH_AUTH_SOCK` points at. `IdentityAgent none` turns it off.
2. **Keys** — the host's `IdentityFile` keys, then the default keys
   (`id_ed25519`, `id_rsa`, `id_ecdsa`, `id_dsa`).
   `IdentitiesOnly yes` skips the agent and the default keys.
3. **Password** — when the methods above are refused, Termide takes the
   password from the [password vault](passwords.md) or asks for it.

`PreferredAuthentications` picks among them: without `publickey` the
agent and keys are not tried; without `password` (or
`keyboard-interactive`) no password is asked for, and a refused login
is reported as an error.

An encrypted key that the agent does not hold asks for its passphrase
once nothing else got in, before the password unless
`PreferredAuthentications` puts `password` first. The passphrase is
taken from the vault or asked for, and kept in the vault under the key's
path, so one entry serves every server that key opens.

FTP and FTPS log in with the user from the URL; a refused login asks for
the password the same way. Without a user in the URL the login is
anonymous.

## SSH configuration (SFTP)

The host in an `sftp://` URL may be a `Host` alias: keep the bookmark
URL plain (`sftp://my-build-host/path`) and let the SSH configuration
supply the real host name, port, user and key, as for the `ssh` CLI.

Termide reads `~/.ssh/config` and then `/etc/ssh/ssh_config` the way
`ssh` does: the first value found for an option wins, `Host` patterns
take `*`, `?` and `!` negations, and `Include` is followed (wildcards
allowed, relative paths taken from `~/.ssh` or `/etc/ssh`). These
options are used:

| Option | Effect |
|--------|--------|
| `HostName` | The server actually connected to; `%h` stands for the host in the URL. |
| `Port` | Used when the URL has no port. |
| `User` | Used when the URL has no user; without either, your local user name. |
| `IdentityFile`, `IdentitiesOnly` | The keys tried, and whether the agent and default keys are. |
| `IdentityAgent` | The SSH agent used; `none` for none. |
| `PreferredAuthentications` | Which of keys (`publickey`) and the password are tried, and which first. |
| `UserKnownHostsFile`, `GlobalKnownHostsFile` | Where server keys are looked up. |
| `HostKeyAlias` | The name the server key is looked up and recorded under. |
| `HashKnownHosts` | `yes` writes remembered keys with the host name hashed. |
| `StrictHostKeyChecking` | `yes` refuses an unknown server key instead of asking. |
| `ProxyJump` | Jump hosts to connect through; see below. |

A port or user written in the URL wins over `Port` and `User`, as on
the `ssh` command line. `Match` blocks are skipped as a whole, since
not all of their conditions can be judged. `ProxyCommand` is ignored:
Termide does not run commands from the SSH configuration, so a host
reached that way is connected to directly; use `ProxyJump` instead.
Other options are ignored too.

`%` tokens are expanded where `ssh_config(5)` allows them:

| Token | Stands for |
|-------|------------|
| `%h` | The host connected to (after `HostName`). |
| `%n` | The host as written in the URL or in `ProxyJump`. |
| `%p` | The port connected to. |
| `%r` | The remote user. |
| `%u` | Your local user name. |
| `%d` | Your home directory. |
| `%k` | `HostKeyAlias`, else the host as written. |
| `%%` | A literal `%`. |

`HostName` takes `%h` and `%%`; `ProxyJump` takes `%h`, `%n`, `%p`, `%r`
and `%%`; `IdentityFile` and `UserKnownHostsFile` take them all. Other
tokens (`%C`, `%i`, `%j`, `%L`, `%l`) are left as written.

### Jump hosts

With `ProxyJump` (`[user@]host[:port]`, or `ssh://[user@]host[:port]`,
several separated by commas and visited in order) Termide logs in to
each jump host in turn, asks it to open a connection to the next one,
and runs the next SSH session inside that connection, as `ssh -J` does.
`ProxyJump none` turns it off for a host.

- Each jump host is resolved with its own `Host` entry: `HostName`,
  `Port`, `User`, keys and `known_hosts` options. A user or port in the
  `ProxyJump` value wins over that entry. The first jump host of the
  list may itself have a `ProxyJump`; the others are reached through
  the ones before them. More than 8 jump hosts on the way to one server
  are refused, which also stops a loop.
- Each jump host's key is checked like any server key (see below)
  before anything is sent to it. The question or refusal names the jump
  host, and the answer is recorded in that host's `known_hosts`.
- Jump hosts log in with the SSH agent and keys only, without the
  password. A refused login to a jump host is reported as an error
  naming it; the password Termide asks for, or takes from the vault, is
  sent only to the server in the URL. An encrypted key is not asked
  about for a jump host: load it into the SSH agent.

## Server keys (SFTP)

Before anything is sent to an SFTP server — a password included —
Termide checks the server's key the way OpenSSH does, against the
files named by `UserKnownHostsFile` and `GlobalKnownHostsFile`: by
default `~/.ssh/known_hosts`, `~/.ssh/known_hosts2`,
`/etc/ssh/ssh_known_hosts` and `/etc/ssh/ssh_known_hosts2`. The server
is looked up under its `HostKeyAlias` if set, else under its
`HostName`, written `[name]:port` off port 22. Hashed entries
(`|1|…`), wildcards and `!` negations are understood.

- **Recorded key** — the connection goes ahead without a word.
- **Unknown host** — the connection stops and a dialog shows the host,
  key type and SHA256 fingerprint, as `ssh` prints them. Compare the
  fingerprint with one you trust, then choose:
  - **Accept and remember** — the key is appended in OpenSSH format
    to the first `UserKnownHostsFile` (`~/.ssh/known_hosts` by
    default; its directory is created with mode 0700 and the file with
    0600 if missing), with the host name hashed under
    `HashKnownHosts yes`, and the connection is made again. Next time,
    here and in `ssh`, it goes ahead silently. With
    `UserKnownHostsFile none` there is nowhere to remember it, and
    only **Accept once** works.
  - **Accept once** — the key is trusted until Termide exits and
    nothing is written.
  - **Cancel** *(preselected)* — nothing is connected. `Esc` does the
    same.

  With `StrictHostKeyChecking yes` there is no question: the
  connection is refused, and the message shows the fingerprint. Every
  other value (`ask`, `accept-new`, `no`, `off`) asks as above;
  Termide never accepts an unknown key without asking.
- **Changed key** — when the recorded key of that type differs, or the
  key is marked `@revoked`, the connection is refused and the dialog
  names the `known_hosts` file and line. There is no override: if the
  change is expected, remove the old key with the `ssh-keygen -R`
  command the dialog gives, and connect again.

## SMB and NFS

`smb://host/share/path` opens a share over SMB 2 or 3, and
`smb://host/` lists the server's shares as directories (the hidden
`$` shares and printers left out). Put the user in the address:
`smb://bob@nas.local/media`, or with a domain
`smb://CORP;bob@server/media`. When the server wants a password,
Termide asks for it and can keep it in the
[password vault](passwords.md). Without a user it signs in as a guest,
which a server may refuse; then give a user.

`nfs://host/export/path` opens an NFSv3 export, and `nfs://host/`
shows the server's exports as directories: an export `/srv/media`
appears as `srv`, with `media` in it. NFS has no password. The server
trusts the user and group ids the client sends, so Termide sends
yours, as the system's NFS client does, and the server maps them by
its own rules (`root_squash`, `all_squash`). A port in the address
(`nfs://nas:2049/srv`) is the NFS service's; when the server has no
portmapper, its mount service is looked for on that port too, as
servers in a container often answer both there.

Two kinds of export cannot be opened, and the panel says which and why:

- **An export that takes requests only from a privileged port**
  (below 1024). That is the Linux default, `secure`, and Linux gives
  those ports to root only; macOS gives them to anyone since 10.14.
  Add `insecure` to the export's options on the server, or mount the
  export with the system and open the mount point.
- **An export that needs Kerberos** (`sec=krb5`, `krb5i` or `krb5p`).
  Mount it with the system and open the mount point.

A server that speaks only NFSv4 is not reached either; most servers
serve version 3 alongside.

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
- A file that SFTP replaces, in either direction, is swapped for the
  new one only once it has arrived whole, and keeps its permission
  bits; a cancelled or broken transfer leaves it as it was. On a server
  that offers `posix-rename@openssh.com` (OpenSSH does) an upload swaps
  the files in one step, so the name always holds one of them; elsewhere
  the old file steps aside for the moment of the swap. A symlink
  on the server is written through rather than replaced. Downloads
  from SMB, NFS and FTP replace a local file the same way; an FTP
  upload writes the server's file in place, so a cancelled one leaves
  what got there.
- A directory download stops at an entry name the server lists with a
  path in it (`../x`, `a/b`), rather than writing outside the folder.
- A new file on SFTP, SMB or NFS is created by the server only while
  the name is free, so a name another client takes at the same moment
  is refused too. FTP cannot create that way: there the name is looked
  up first.

A move within one connection is renamed on the server, not downloaded
and uploaded again; on SMB within one share and on NFS within one
export, as the protocols allow. Anywhere else the move copies and
then deletes.

## Projects

A project layout that includes a remote file manager panel persists the
URL just like a local path. On the next start Termide reconnects in the
background and shows the panel with a loading placeholder until the
listing arrives — the rest of the UI is responsive immediately.

### Dropped connections

If the remote session is lost (idle timeout, network drop), the next
operation fails and the panel shows a recovery dialog with three choices:
**Reconnect** (open a fresh session to the same path), **Open home
(local)** (drop the connection and switch the panel to your local home
directory), or **Close panel**. Pressing `Esc` dismisses the dialog and
leaves the panel on its last listing. The panel never loops on the error.
A dialog that waited behind another one answers the panel that raised it,
even when another panel has the focus by then.

A connection that is still being opened after `connection_timeout_secs`
seconds under `[vfs]` (default `60`; **VFS → Connection timeout (secs)**
in the settings) is given up and the panel says so. Opening an archive is
local work and is not timed out; `Esc` cancels it.

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
when its target stays inside the extracted tree — resolved the way the
system resolves it, through the archive's other links — and only after
all files are written. Nothing is written through a symbolic link that
is already in the destination: a link where a file goes is replaced by
the file, and a link where a directory goes stops the extraction with an
error. An entry that unpacks to more than its declared size fails
instead of filling the disk.

The URL of a location inside an archive is
`archive://<archive file>!<path inside>`, e.g.
`archive:///home/nvn/src.tar.gz!/docs`; it works in `Go to path` and in
saved projects like any other VFS URL.

### Packing

`P` (or `Alt+F5`) packs the selected files and directories (or the entry under the
cursor) into a new archive. The prompt suggests `<name>.zip` next to
the selection; the name's suffix picks the format — `.zip`, `.tar`,
`.tar.gz`, `.tar.bz2`, `.tar.xz` or `.tar.zst` — and the **Format** row
under the name swaps it for another writable one. A relative path is
taken from the selection's directory, and a directory gets an archive
named after the selection inside it.

The **Compression** row under it (reached with `↓`, changed with
`←`/`→`, `Space` or a click, as is **Format**) picks **None**, **Fast**,
**Normal** (the default) or **Best**. zip, `.tar.gz`, `.tar.bz2` and `.tar.xz` map them
to levels 0, 1, 6 and 9; bzip2 and xz have no uncompressed mode, so
**None** gives their weakest level. `.tar.zst` only tells **None** from
the rest, and a plain `.tar` is never compressed.

Packing runs in the [Operations panel](operations.md) with progress,
pause and cancel. Symbolic links are stored as links, not followed;
permissions and modification times are kept. The archive is written
to a temporary file and appears under its name only when complete, so
a cancelled or failed run leaves nothing behind, and an existing file
is never overwritten. Only local files can be packed.

### Unpacking

`Alt+F9` unpacks the archives in the selection, or the archive under the
cursor; other selected entries are left out. The prompt suggests a
directory named after the archive in the nearest other file manager's
directory and offers the other panels' directories in its
[directory tree](file-manager.md#destination-path). A
single archive is unpacked into the directory you give, which is created
if needed; several archives go into a directory each, named after the
archive, under it. A relative path is taken from the archive's directory.

Unpacking runs in the [Operations panel](operations.md) with progress,
pause and cancel. When a target directory already holds files, Termide
asks first, since files with the same names are replaced. Only local
archives can be unpacked. For an encrypted zip archive Termide asks for
the password before starting and asks again after a wrong one.

## Limitations / known gaps

- File search and replace work only in local folders: in a remote panel
  or inside an archive they are refused with a notice.
- NFS version 4 and Kerberos are not spoken, and an export kept to
  privileged ports needs root on Linux; see [SMB and NFS](#smb-and-nfs).
- No resume-from-byte-offset for interrupted transfers: cancelled
  uploads start over from the beginning if re-issued.
- The passphrase of an encrypted SSH key is not asked for in remote
  panels; load the key into your SSH agent.
- Host certificates (`@cert-authority`) are not accepted, and user
  certificates (`CertificateFile`) are not offered.
- `Match` blocks in the SSH configuration are skipped, `ProxyCommand` is
  not run, and the `%C`, `%i`, `%j`, `%L` and `%l` tokens are not
  expanded.
- Jump hosts cannot log in with a password; load their key into your
  SSH agent or name it with `IdentityFile`.
