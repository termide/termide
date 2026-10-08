# Installation Guide

This guide covers different methods to install TermIDE on your system.

## Download Pre-built Binary (Recommended)

The easiest way to get started is to download a pre-built binary for your platform.

### Step 1: Download

Visit the [GitHub Releases](https://github.com/termide/termide/releases) page and download the latest version for your platform:

**Linux x86_64** (also works in WSL/WSL2):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-gnu.tar.gz
```

**Linux x86_64 — static musl** (Alpine, distroless containers, any
glibc-free system):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
```

**Linux ARM64** (Raspberry Pi, ARM servers):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-unknown-linux-gnu.tar.gz
```

**Linux ARM64 — static musl** (Android/Termux, Alpine ARM, any glibc-free ARM64):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
```

**macOS Intel (x86_64)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-apple-darwin.tar.gz
```

**macOS Apple Silicon (M1/M2/M3)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-apple-darwin.tar.gz
```

### Step 2: Extract

```bash
tar xzf termide-*.tar.gz
```

### Step 3: Run

```bash
./termide
```

### Step 4: Install System-wide (Optional)

To install TermIDE system-wide, move the binary to a directory in your PATH:

```bash
# Linux
sudo mv termide /usr/local/bin/

# macOS
sudo mv termide /usr/local/bin/
```

Now you can run `termide` from anywhere in your terminal.

## Portable Static Binary (Alpine / containers)

Every release also ships a fully static **musl** build that links no shared
libraries and runs on any Linux distribution, including Alpine and minimal
containers. The whole project is pure-Rust (rustls + russh + russh-sftp — no
OpenSSL, no libssh2), so it is the same code, just compiled against musl.

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
tar xzf termide-0.40.0-x86_64-unknown-linux-musl.tar.gz
./termide

# Verify it is fully static — no shared libraries
ldd ./termide   # → "not a dynamic executable"
```

To build it yourself (e.g. for a different musl variant), the flake exposes the
recipe as a derivation:

```bash
nix build github:termide/termide#termide-static
./result/bin/termide
```

The binary can be copied into a container or a stripped Alpine image and runs
without `musl-dev` or `glibc` installed. (The ARM64 musl build is also what
[Android / Termux](#android-termux) uses.)

## Install via Package Manager

### Debian/Ubuntu (.deb)

```bash
wget https://github.com/termide/termide/releases/latest/download/termide_0.40.0-1_amd64.deb
sudo dpkg -i termide_0.40.0-1_amd64.deb
```

### Fedora/RHEL/CentOS (.rpm)

```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-1.x86_64.rpm
sudo rpm -i termide-0.40.0-1.x86_64.rpm
```

### Arch Linux (AUR)

```bash
# Build from source
yay -S termide

# Or install pre-built binary
yay -S termide-bin
```

### Homebrew (macOS/Linux)

```bash
brew tap termide/termide
brew install termide
```

### NixOS/Nix (Flakes)

```bash
# Run without installing
nix run github:termide/termide

# Install to user profile
nix profile install github:termide/termide
```

## Build from Source

### Prerequisites

- **Rust 1.70+** (stable toolchain)
- **Git**

### Using Cargo

```bash
# Clone the repository
git clone https://github.com/termide/termide.git
cd termide

# Build in release mode
cargo build --release

# The binary will be at target/release/termide
./target/release/termide

# Optional: Install to ~/.cargo/bin
cargo install --path .

# Or install directly from the repository, without cloning first:
cargo install --git https://github.com/termide/termide --locked
```

> **Note:** TermIDE is **not** published to crates.io. `cargo install termide`
> would fetch an obsolete, unrelated early release — always build from source
> (the clone above) or use `cargo install --git …`.

### Using Nix (with Flakes)

```bash
# Clone the repository
git clone https://github.com/termide/termide.git
cd termide

# Enter the development shell
nix develop

# Build with cargo
cargo build --release

# Or build with Nix directly
nix build
```

## Shell Completions

`termide --completions <shell>` prints a completion script for bash, zsh or
fish. Once loaded, it completes the `--` options, the values `--log-level` and
`--completions` take, paths after `--config`, and — the useful part on a server
with several detached instances — the instance ids after `--attach`, read from
`termide --list-instances` at the moment Tab is pressed.

The quick way is to let termide place the file:

```bash
termide --install-completions          # for the shell in $SHELL
termide --install-completions fish     # or name one
```

bash and fish scan a per-user directory on their own —
`~/.local/share/bash-completion/completions/` (with the `bash-completion`
package installed) and `~/.config/fish/completions/` — so for them that is the
whole installation. zsh has no such directory: the script goes to
`~/.zfunc/_termide` and termide prints the one line to add to `~/.zshrc`
before `compinit`, or to `~/.zshenv`. If nothing in those files runs
`compinit`, it says so too: that is what switches completion on in zsh, and
without it no command completes. It never edits an rc file itself.

```zsh
fpath=(~/.zfunc $fpath)
```

The script can also be printed and placed by hand, which is what a dotfiles
repository or a shell without `bash-completion` needs:

```bash
# bash: in ~/.bashrc
eval "$(termide --completions bash)"

# zsh: into a directory on $fpath, then compinit
termide --completions zsh > ~/.zfunc/_termide

# fish
termide --completions fish > ~/.config/fish/completions/termide.fish
```

The same three files live in the repository's `completions/` directory and in
the release tarballs. The `.deb`, `.rpm` and AUR packages install them
system-wide, so with a package nothing needs adding to a shell rc file.

## Platform-Specific Notes

### Linux

No additional dependencies required for the pre-built binary.

If building from source, you may need development packages:
```bash
# Debian/Ubuntu
sudo apt-get install build-essential

# Fedora/RHEL
sudo dnf install gcc
```

### macOS

On first run, macOS may block the application because it's not signed. To allow it:
1. Right-click on `termide` and select "Open"
2. Click "Open" in the security dialog

Alternatively, remove the quarantine attribute:
```bash
xattr -d com.apple.quarantine termide
```

### Windows (Native)

TermIDE runs natively on Windows 10+ using ConPTY. Requires Windows Terminal for best experience.

1. Download the `.zip` archive from [GitHub Releases](https://github.com/termide/termide/releases):
   - `termide-VERSION-x86_64-pc-windows-msvc.zip`
2. Extract the archive
3. Run `termide.exe` in Windows Terminal

**Configuration paths:**
- Config: `%APPDATA%\termide\config.toml`
- Projects: `%APPDATA%\termide\projects\`
- Logs: `%LOCALAPPDATA%\termide\cache\termide.log`

### Windows (WSL)

TermIDE also works in Windows Subsystem for Linux (WSL and WSL2):

1. Install WSL2 if you haven't already
2. Download the Linux x86_64 binary inside WSL:
   ```bash
   wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-x86_64-unknown-linux-gnu.tar.gz
   tar xzf termide-0.40.0-x86_64-unknown-linux-gnu.tar.gz
   ./termide
   ```

### Android (Termux)

TermIDE runs in [Termux](https://termux.dev). Use the **static ARM64 musl**
build — the glibc `aarch64-unknown-linux-gnu` build won't run on Android's
Bionic libc:

```bash
pkg install git openssh   # tools termide shells out to (plus any LSP servers)
wget https://github.com/termide/termide/releases/latest/download/termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
tar xzf termide-0.40.0-aarch64-unknown-linux-musl.tar.gz
./termide
```

The system clipboard is unavailable on Android (no X11/Wayland) and the resource
monitor may show partial data (restricted `/proc`); the editor, file manager,
git, and integrated terminal work normally.

## Verify Installation

After installation, verify it's working:

```bash
termide --version
```

## Next Steps

- Read the [User Interface Guide](ui.md) to understand the application layout
- Learn about [File Manager](file-manager.md) keyboard shortcuts
- Explore [Terminal](terminal.md) and [Editor](editor.md) features
- Customize your experience with [Themes](themes.md)
