# TermIDE Packaging

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

Two PKGBUILD variants are provided in `aur/`:
- `PKGBUILD` - Builds from source
- `PKGBUILD-bin` - Uses pre-built binaries from GitHub releases

**For source package (termide):**
```bash
cd aur
makepkg -si
```

**For binary package (termide-bin):**
```bash
cd aur
makepkg -p PKGBUILD-bin -si
```

**Publishing to AUR:**
1. Update sha256sums after creating release
2. Generate .SRCINFO: `makepkg --printsrcinfo > .SRCINFO`
3. Clone AUR repo: `git clone ssh://aur@aur.archlinux.org/termide.git`
4. Copy PKGBUILD and .SRCINFO to AUR repo
5. Commit and push to AUR

## Homebrew (macOS/Linux)

Formula is provided in `homebrew/termide.rb`.

**For tap repository (recommended):**
1. Create GitHub repo: `homebrew-termide`
2. Add formula to `Formula/termide.rb`
3. Users install with: `brew install termide/termide/termide`

**Update SHA256 checksums:**
After creating a GitHub release, download each tarball and compute SHA256:
```bash
shasum -a 256 termide-0.1.3-*.tar.gz
```

Update the corresponding `sha256` values in `termide.rb`.

**Local testing:**
```bash
brew install --build-from-source packaging/homebrew/termide.rb
```

## GitHub Actions

Automated packaging is configured in `.github/workflows/release.yml`.

On each tag push, the workflow:
1. Builds binaries for all platforms
2. Creates .deb and .rpm packages
3. Uploads all artifacts to GitHub Releases
4. Optionally updates Homebrew tap formula

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

- All packages require updating version numbers and checksums after each release
- GitHub Actions workflow automatically handles binary builds and package creation
- For official repository inclusion (Debian, Fedora, Homebrew Core), additional review processes apply
