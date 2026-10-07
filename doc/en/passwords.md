# Password Vault

TermIDE keeps the passwords of remote connections in an encrypted vault
protected by a master password, so bookmarks and saved layouts never have
to contain them.

The vault serves:

- **Remote file systems** — SFTP, FTP, FTPS (see [VFS](vfs.md)).
- **Databases** — PostgreSQL and MySQL (see [Database Viewer](database.md)).
- **Git over HTTPS** — user name and password or token for push, pull and
  fetch.
- **Git over SSH** — the passphrase of your SSH key.

## How it works

1. A connection is refused: SFTP after the SSH agent and keys, FTP after
   the anonymous login, a database without a password, git without
   credentials.
2. If the vault holds a password for that server and user, TermIDE uses it.
   When the vault is locked, it asks for the master password first.
3. Otherwise TermIDE asks for the password. Tick **Save in password vault**
   to keep it.
4. A password you asked to keep is saved only after the server accepted it,
   so a typo never lands in the vault.
5. The first save creates the vault. You choose the master password and
   type it twice.

If a stored password is refused, for example because it was changed on the
server, TermIDE asks again. The checkbox is ticked already, so the new
password replaces the old one.

Cancelling the master password prompt does not cancel the connection.
TermIDE asks for the login password itself instead.

Passwords are filed by server: scheme, host and port (default ports are
left out) plus the user name. A database password belongs to the server
login, not to one database, so switching databases in a panel keeps
finding it. The passphrase of the SSH key used by git is kept as one entry.

## Bookmarks with a password in the URL

When you add or edit a bookmark whose URL contains a password
(`postgres://user:secret@host/db`), TermIDE offers to move the password into
the vault. Answer **Yes**:

- the password is stored in the vault;
- once it is stored, the bookmark is rewritten without it.

Project bookmarks (`.termide/bookmarks.toml`) can then be committed safely.

## Locking

The unlocked vault locks itself after 15 minutes without use. Change the
delay in **Settings → VFS → Lock password vault after**, or in `config.toml`:

```toml
[vault]
lock_after_mins = 15   # 0 keeps it unlocked until termide exits
```

**Lock password vault** in the command palette locks it at once.

A [detached instance](detached-instances.md) holds the unlocked vault in the
server process; clients never receive it.

## The vault file

The vault is `~/.config/termide/vault.toml` (owner-only permissions). There
is one vault per user, never one per project.

- **Secrets are encrypted.** They are sealed to an X25519 key pair, using
  ephemeral X25519, HKDF-SHA256 and XChaCha20-Poly1305. The private key is
  encrypted with a key derived from the master password by Argon2id
  (64 MiB, 3 passes).
- **Saving needs no unlock.** Only the public key is used for it, so
  TermIDE asks for the master password only when a stored password is
  actually needed.
- **Metadata is readable.** Kind, server and user name are stored in
  clear, so TermIDE can tell whether a password exists without unlocking.
  They are bound to the encrypted secret: an edited entry fails to decrypt
  instead of sending the password to another host.
- **Concurrent writes are safe.** Several TermIDE processes may share the
  vault. Writes are atomic and serialised by a lock file.

**A forgotten master password cannot be recovered.** Delete `vault.toml` to
start over; the stored passwords are lost with it.

## What it protects against

The vault protects passwords from anyone who reads your config files,
backups, synced dotfiles or a repository a bookmark file was committed to.

It does not protect against a program running as your user while the vault
is unlocked: such a program can read TermIDE's memory or log your
keystrokes.

## Limitations

- An FTP URL without a user name logs in anonymously, and its prompt asks
  for a password only. Put the user in the URL (`ftp://user@host`).
- There is no list or editor of the stored entries yet. To remove one,
  delete the vault file.
- Encrypted archive passwords are not stored.
