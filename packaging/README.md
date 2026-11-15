# Termide Packaging

This directory contains packaging configurations for various Linux and macOS package managers.

## Debian/Ubuntu (.deb)

Configuration is in the main `Cargo.toml` under `[package.metadata.deb]`.

**Build package:**
```bash
cargo install cargo-deb
cargo deb
```

Output: `target/debian/termide_<version>_<arch>.deb`

## Fedora/RHEL (.rpm)

Configuration is in the main `Cargo.toml` under `[package.metadata.generate-rpm]`.

**Build package:**
```bash
cargo install cargo-generate-rpm
cargo build --release
cargo generate-rpm
```

Output: `target/generate-rpm/termide-<version>-1.<arch>.rpm`

## Arch Linux (AUR)

Two packages are available on [AUR](https://aur.archlinux.org/):
- `termide` - Builds from source
- `termide-bin` - Uses pre-built binaries from GitHub releases

**Installation (using yay or paru):**
```bash
yay -S termide      # or termide-bin
```

**Local testing:**
```bash
cd aur
makepkg -si                     # source package
makepkg -p PKGBUILD-bin -si     # binary package
```

> **Note:** AUR packages are automatically updated via GitHub Actions when a new release is tagged.

## Homebrew (macOS/Linux)

Formula is maintained in a separate tap repository: [termide/homebrew-termide](https://github.com/termide/homebrew-termide)

**Installation:**
```bash
brew tap termide/termide
brew install termide
```

> **Note:** Homebrew formula is automatically updated via GitHub Actions when a new release is tagged.

## GitHub Actions

Automated packaging is configured in `.github/workflows/release.yml`.

On each tag push, the workflow automatically:
1. Runs quality checks (fmt, clippy, tests, cargo-deny)
2. Builds binaries: Linux x64/arm64 (glibc and musl), macOS x64/arm64, Windows x64
3. Creates .deb and .rpm packages (x86_64)
4. Uploads all artifacts to GitHub Releases
5. Updates AUR packages (termide and termide-bin)
6. Updates the Homebrew tap formula
7. Updates the website

**Required secrets for automation:**
- `AUR_SSH_KEY` - Private SSH key for pushing to AUR
- `HOMEBREW_TAP_SSH_KEY` - Deploy key with write access to the homebrew-termide repo
- `SITE_DEPLOY_KEY` - Deploy key with write access to the website repository

## NixOS

The project includes a `flake.nix` in the root directory for Nix users.

**Build with Nix:**
```bash
nix build
```

**Run without installing:**
```bash
nix run github:termide/termide
```

## Notes

- `tools/release/version.sh <version>` bumps the version everywhere it is repeated, the in-repo PKGBUILDs and `flake.nix` among them; CI runs `tools/release/version.sh --check`
- The release workflow fills in the checksums of the AUR packages and the Homebrew formula
- For official repository inclusion (Debian, Fedora, Homebrew Core), additional review processes apply
