# Installation Guide

This guide covers different methods to install TermIDE on your system.

## Download Pre-built Binary (Recommended)

The easiest way to get started is to download a pre-built binary for your platform.

### Step 1: Download

Visit the [GitHub Releases](https://github.com/termide/termide/releases) page and download the latest version for your platform:

**Linux x86_64** (also works in WSL/WSL2):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.5.0-x86_64-unknown-linux-gnu.tar.gz
```

**Linux ARM64** (Raspberry Pi, ARM servers):
```bash
wget https://github.com/termide/termide/releases/latest/download/termide-0.5.0-aarch64-unknown-linux-gnu.tar.gz
```

**macOS Intel (x86_64)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.5.0-x86_64-apple-darwin.tar.gz
```

**macOS Apple Silicon (M1/M2/M3)**:
```bash
curl -LO https://github.com/termide/termide/releases/latest/download/termide-0.5.0-aarch64-apple-darwin.tar.gz
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
```

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

### Windows (WSL)

TermIDE works in Windows Subsystem for Linux (WSL and WSL2):

1. Install WSL2 if you haven't already
2. Download the Linux x86_64 binary inside WSL:
   ```bash
   wget https://github.com/termide/termide/releases/latest/download/termide-0.5.0-x86_64-unknown-linux-gnu.tar.gz
   tar xzf termide-0.5.0-x86_64-unknown-linux-gnu.tar.gz
   ./termide
   ```

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
