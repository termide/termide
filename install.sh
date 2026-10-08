#!/bin/sh
# TermIDE Installer
# https://github.com/termide/termide
#
# Usage: curl -fsSL https://raw.githubusercontent.com/termide/termide/main/install.sh | sh

set -e

REPO="termide/termide"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
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

# Get latest version from GitHub
get_latest_version() {
    VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed -E 's/.*"v?([^"]+)".*/\1/')
    if [ -z "$VERSION" ]; then
        error "Failed to get latest version"
    fi
}

# Check if command exists
has_cmd() {
    command -v "$1" >/dev/null 2>&1
}

# Install using deb package
install_deb() {
    info "Installing via deb package..."
    URL="https://github.com/$REPO/releases/download/$VERSION/termide_${VERSION}-1_amd64.deb"
    TMP_FILE="/tmp/termide_${VERSION}.deb"
    curl -fsSL "$URL" -o "$TMP_FILE"
    sudo dpkg -i "$TMP_FILE"
    rm -f "$TMP_FILE"
}

# Install using rpm package
install_rpm() {
    info "Installing via rpm package..."
    URL="https://github.com/$REPO/releases/download/$VERSION/termide-${VERSION}-1.x86_64.rpm"
    sudo dnf install -y "$URL" || sudo rpm -i "$URL"
}

# Install using binary tarball
install_binary() {
    info "Installing binary to $INSTALL_DIR..."

    if [ "$OS" = "linux" ]; then
        TARGET="${ARCH}-unknown-linux-gnu"
    else
        TARGET="${ARCH}-apple-darwin"
    fi

    URL="https://github.com/$REPO/releases/download/$VERSION/termide-${VERSION}-${TARGET}.tar.gz"
    TMP_DIR=$(mktemp -d)

    curl -fsSL "$URL" | tar xz -C "$TMP_DIR"
    sudo install -m 755 "$TMP_DIR/termide" "$INSTALL_DIR/"
    rm -rf "$TMP_DIR"
}

# Install using Homebrew
install_homebrew() {
    info "Installing via Homebrew..."
    brew install termide/termide/termide
}

# Install using Cargo
install_cargo() {
    # Build from the Git repository, not crates.io: TermIDE is a Cargo
    # workspace and is not published to crates.io (the name there points at
    # an obsolete early release). `--locked` builds against the committed
    # Cargo.lock for a reproducible result.
    info "Installing via Cargo (compiling from source)..."
    cargo install --git https://github.com/termide/termide --locked termide
}

# Install using Nix
install_nix() {
    info "Installing via Nix..."
    nix profile install github:termide/termide --refresh
}

# Show menu and get choice
show_menu() {
    printf "\n${BOLD}TermIDE Installer${RESET}\n"
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

    info "Fetching latest version..."
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
    info "TermIDE $VERSION installed successfully!"
    info "Run 'termide' to start."
}

main
