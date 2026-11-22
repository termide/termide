#!/bin/sh
# Termide Installer
# https://github.com/termide/termide
#
# Usage: curl -fsSL https://raw.githubusercontent.com/termide/termide/main/install.sh | sh
#
# Environment, as install.ps1 takes it:
#   TERMIDE_VERSION      install this version instead of the latest
#   TERMIDE_INSTALL_DIR  where the binary goes (default /usr/local/bin;
#                        INSTALL_DIR is still read)

set -e

REPO="termide/termide"
# Termux has no /usr/local and no sudo: its prefix holds the binaries.
if [ -n "$TERMUX_VERSION" ] && [ -n "$PREFIX" ]; then
    DEFAULT_DIR="$PREFIX/bin"
else
    DEFAULT_DIR="/usr/local/bin"
fi
INSTALL_DIR="${TERMIDE_INSTALL_DIR:-${INSTALL_DIR:-$DEFAULT_DIR}}"
BOLD="\033[1m"
GREEN="\033[32m"
YELLOW="\033[33m"
RED="\033[31m"
RESET="\033[0m"

info() {
    printf "${GREEN}>${RESET} %s\n" "$1"
}

warn() {
    printf "${YELLOW}!${RESET} %s\n" "$1"
}

error() {
    printf "${RED}x${RESET} %s\n" "$1" >&2
    exit 1
}

# Detect OS
detect_os() {
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
    case "$OS" in
        linux) OS="linux" ;;
        darwin) OS="darwin" ;;
        *) error "Unsupported OS: $OS" ;;
    esac
}

# Detect architecture
detect_arch() {
    ARCH=$(uname -m)
    case "$ARCH" in
        x86_64|amd64) ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) error "Unsupported architecture: $ARCH" ;;
    esac
}

# Detect Linux distribution
detect_distro() {
    DISTRO="unknown"
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        DISTRO="$ID"
    elif [ -f /etc/debian_version ]; then
        DISTRO="debian"
    elif [ -f /etc/redhat-release ]; then
        DISTRO="rhel"
    fi
}

# Get the version to install: TERMIDE_VERSION, or the latest from GitHub
get_latest_version() {
    if [ -n "$TERMIDE_VERSION" ]; then
        # Tags carry no `v`: TERMIDE_VERSION=v0.51.0 means 0.51.0.
        VERSION="${TERMIDE_VERSION#v}"
        return
    fi
    VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v?([^"]+)".*/\1/')
    if [ -z "$VERSION" ]; then
        error "Failed to get latest version"
    fi
}

# Check if command exists
has_cmd() {
    command -v "$1" >/dev/null 2>&1
}

# The SHA-256 of file $1, in lowercase hex
sha256_of() {
    if has_cmd sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has_cmd shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        error "Neither sha256sum nor shasum is installed: cannot verify the download"
    fi
}

# Check file $1 against the checksum published at $2, or stop
verify_sha256() {
    SUM_FILE="$1.sha256"
    curl -fsSL "$2" -o "$SUM_FILE" || error "No checksum published at $2: not installing an unverified download"
    EXPECTED=$(cut -c1-64 "$SUM_FILE" | tr 'A-F' 'a-f')
    case "$EXPECTED" in
        *[!0-9a-f]*|"") error "The checksum at $2 is not a SHA-256" ;;
    esac
    [ ${#EXPECTED} -eq 64 ] || error "The checksum at $2 is not a SHA-256"
    ACTUAL=$(sha256_of "$1")
    [ "$ACTUAL" = "$EXPECTED" ] || error "Checksum mismatch: expected $EXPECTED, got $ACTUAL"
    info "Checksum verified."
}

# Install using deb package
install_deb() {
    info "Installing via deb package..."
    URL="https://github.com/$REPO/releases/download/$VERSION/termide_${VERSION}-1_amd64.deb"
    TMP_DIR=$(mktemp -d)
    TMP_FILE="$TMP_DIR/termide_${VERSION}.deb"
    curl -fsSL "$URL" -o "$TMP_FILE" || error "Download failed: $URL"
    verify_sha256 "$TMP_FILE" "$URL.sha256"
    # apt, unlike dpkg -i, installs what the package depends on
    sudo apt-get install -y "$TMP_FILE"
    rm -rf "$TMP_DIR"
}

# Install using rpm package
install_rpm() {
    info "Installing via rpm package..."
    URL="https://github.com/$REPO/releases/download/$VERSION/termide-${VERSION}-1.x86_64.rpm"
    TMP_DIR=$(mktemp -d)
    TMP_FILE="$TMP_DIR/termide-${VERSION}.rpm"
    curl -fsSL "$URL" -o "$TMP_FILE" || error "Download failed: $URL"
    verify_sha256 "$TMP_FILE" "$URL.sha256"
    sudo dnf install -y "$TMP_FILE" || sudo rpm -i "$TMP_FILE"
    rm -rf "$TMP_DIR"
}

# Whether glibc version $1 (such as 2.35) is at least $2.$3
glibc_at_least() {
    MAJOR=$(echo "$1" | cut -d. -f1)
    MINOR=$(echo "$1" | cut -d. -f2)
    case "$MAJOR$MINOR" in
        ""|*[!0-9]*) return 1 ;;
    esac
    [ "$MAJOR" -gt "$2" ] || { [ "$MAJOR" -eq "$2" ] && [ "$MINOR" -ge "$3" ]; }
}

# Install using binary tarball
install_binary() {
    info "Installing binary to $INSTALL_DIR..."

    if [ "$OS" = "linux" ]; then
        # The gnu build needs the system's glibc loader. Without it (Alpine,
        # NixOS, a 32-bit userland on a 64-bit kernel such as Raspberry Pi OS
        # with arm_64bit=1) the binary fails with "No such file or directory",
        # so fall back to the fully static musl build.
        case "$ARCH" in
            x86_64) LOADER="/lib64/ld-linux-x86-64.so.2" ;;
            aarch64) LOADER="/lib/ld-linux-aarch64.so.1" ;;
        esac
        # The gnu build is linked against glibc 2.34; an older one (RHEL 8,
        # Debian 11, Ubuntu 20.04) cannot load it either.
        GLIBC=$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc //p')
        if [ ! -e "$LOADER" ]; then
            info "No glibc loader ($LOADER) found, using the static musl build"
            TARGET="${ARCH}-unknown-linux-musl"
        elif ! glibc_at_least "$GLIBC" 2 34; then
            info "glibc ${GLIBC:-of unknown version} is older than 2.34, using the static musl build"
            TARGET="${ARCH}-unknown-linux-musl"
        else
            TARGET="${ARCH}-unknown-linux-gnu"
        fi
    else
        TARGET="${ARCH}-apple-darwin"
    fi

    URL="https://github.com/$REPO/releases/download/$VERSION/termide-${VERSION}-${TARGET}.tar.gz"
    TMP_DIR=$(mktemp -d)

    ARCHIVE="$TMP_DIR/termide.tar.gz"
    curl -fsSL "$URL" -o "$ARCHIVE" || error "Download failed: $URL"
    verify_sha256 "$ARCHIVE" "$URL.sha256"
    tar xzf "$ARCHIVE" -C "$TMP_DIR"
    # sudo only where it is needed: root in a container, Termux or a
    # directory of the user's own have none, or no need of it.
    if [ -d "$INSTALL_DIR" ] && [ -w "$INSTALL_DIR" ] || [ "$(id -u)" = 0 ]; then
        mkdir -p "$INSTALL_DIR"
        install -m 755 "$TMP_DIR/termide" "$INSTALL_DIR/"
    else
        sudo mkdir -p "$INSTALL_DIR"
        sudo install -m 755 "$TMP_DIR/termide" "$INSTALL_DIR/"
    fi
    rm -rf "$TMP_DIR"
    info "Shell completions: run 'termide --install-completions'."
}

# Install using Homebrew
install_homebrew() {
    info "Installing via Homebrew..."
    brew install termide/termide/termide
}

# Install using Cargo
install_cargo() {
    # Build from the Git repository, not crates.io: Termide is a Cargo
    # workspace and is not published to crates.io (the name there points at
    # an obsolete early release). `--locked` builds against the committed
    # Cargo.lock for a reproducible result. CFLAGS carries the flag the
    # repository's .cargo/config.toml sets for the syntax grammars, which
    # cargo does not read for an install from git: without it GCC builds some
    # grammars' scanners into code that writes into freed memory.
    info "Installing via Cargo (compiling from source)..."
    CFLAGS="${CFLAGS:+$CFLAGS }-fno-strict-aliasing" \
        cargo install --git https://github.com/termide/termide --locked termide
}

# Install using Nix
install_nix() {
    info "Installing via Nix..."
    nix profile install github:termide/termide --refresh
}

# Show menu and get choice
show_menu() {
    printf "\n${BOLD}Termide Installer${RESET}\n"
    printf "Version: ${GREEN}%s${RESET}\n" "$VERSION"
    printf "System:  %s/%s" "$OS" "$ARCH"
    [ "$OS" = "linux" ] && printf " (%s)" "$DISTRO"
    printf "\n\n"

    printf "Available installation methods:\n\n"

    N=1
    METHODS=""

    # Check if Nix is the recommended method (NixOS with nix available)
    NIX_RECOMMENDED=""
    if [ "$DISTRO" = "nixos" ] && has_cmd nix; then
        NIX_RECOMMENDED="yes"
    fi

    # Binary is always available
    if [ -n "$NIX_RECOMMENDED" ]; then
        printf "  ${GREEN}%d${RESET}) Binary\n" "$N"
    else
        printf "  ${GREEN}%d${RESET}) Binary (recommended)\n" "$N"
    fi
    METHODS="${METHODS}binary "
    N=$((N + 1))

    # Package managers
    if [ "$OS" = "linux" ]; then
        if has_cmd dpkg && [ "$ARCH" = "x86_64" ]; then
            printf "  ${GREEN}%d${RESET}) Deb package (Debian/Ubuntu)\n" "$N"
            METHODS="${METHODS}deb "
            N=$((N + 1))
        fi
        if has_cmd dnf && [ "$ARCH" = "x86_64" ]; then
            printf "  ${GREEN}%d${RESET}) RPM package (Fedora/RHEL)\n" "$N"
            METHODS="${METHODS}rpm "
            N=$((N + 1))
        fi
    fi

    if has_cmd brew; then
        printf "  ${GREEN}%d${RESET}) Homebrew\n" "$N"
        METHODS="${METHODS}homebrew "
        N=$((N + 1))
    fi

    if has_cmd nix; then
        if [ -n "$NIX_RECOMMENDED" ]; then
            printf "  ${GREEN}%d${RESET}) Nix (recommended for NixOS)\n" "$N"
        else
            printf "  ${GREEN}%d${RESET}) Nix\n" "$N"
        fi
        METHODS="${METHODS}nix "
        N=$((N + 1))
    fi

    if has_cmd cargo; then
        printf "  ${GREEN}%d${RESET}) Cargo (compile from source)\n" "$N"
        METHODS="${METHODS}cargo "
        N=$((N + 1))
    fi

    printf "\n  ${YELLOW}0${RESET}) Cancel\n"
    printf "\n"
}

# Main installation flow
main() {
    info "Detecting system..."
    detect_os
    detect_arch
    [ "$OS" = "linux" ] && detect_distro

    info "Fetching the version to install..."
    get_latest_version

    show_menu

    printf "Select installation method [1]: "
    read -r CHOICE </dev/tty
    CHOICE="${CHOICE:-1}"

    if [ "$CHOICE" = "0" ]; then
        info "Installation cancelled."
        exit 0
    fi

    # Get method by index
    METHOD=$(echo "$METHODS" | cut -d' ' -f"$CHOICE")

    if [ -z "$METHOD" ]; then
        error "Invalid choice: $CHOICE"
    fi

    printf "\n"

    case "$METHOD" in
        binary) install_binary ;;
        deb) install_deb ;;
        rpm) install_rpm ;;
        homebrew) install_homebrew ;;
        nix) install_nix ;;
        cargo) install_cargo ;;
        *) error "Unknown method: $METHOD" ;;
    esac

    printf "\n"
    info "Termide $VERSION installed successfully!"
    info "Run 'termide' to start."
}

main
