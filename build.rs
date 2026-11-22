//! Build script: stamp the binary with the git commit it was built from so
//! that two builds carrying the same crate version (e.g. several `0.23.1`
//! builds during a release) can be told apart at runtime.
//!
//! Emits `TERMIDE_VERSION` as `"<pkg> (<short-hash>[-dirty])"`, or just
//! `"<pkg>"` when git is unavailable (release tarballs, offline builds).
//!
//! It lives in the binary crate, the leaf of the workspace graph, because it
//! reruns on every change of git's index: in a library crate each rerun
//! rebuilt every crate depending on it, the whole workspace on each commit.
//! The binary hands the string to `termide_core::set_version` at startup.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Re-stamp when the checked-out commit changes. Paths are relative to the
    // repository root, this crate's manifest dir. `index` moves on
    // commit/stage, `HEAD` on branch switch. In a linked worktree `.git` is a
    // file, so neither exists and the stamp follows source changes only.
    for path in [".git/HEAD", ".git/index"] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());

    let short_hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|out| !out.stdout.is_empty())
        .unwrap_or(false);

    let version = match short_hash {
        Some(hash) if dirty => format!("{pkg} ({hash}-dirty)"),
        Some(hash) => format!("{pkg} ({hash})"),
        None => pkg.clone(),
    };

    println!("cargo:rustc-env=TERMIDE_VERSION={version}");

    // For Windows, embed version and product metadata into the executable so
    // that Explorer shows them in the file properties and SignPath can enforce
    // metadata restrictions on signed binaries. The target's OS, not the
    // host's: a build script is compiled for the host, so `cfg(windows)` here
    // would miss a cross-build and fire for a Windows host building for
    // another target.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let parts: Vec<u64> = pkg.split('.').map(|s| s.parse().unwrap_or(0)).collect();
        let (major, minor, patch) = (
            *parts.first().unwrap_or(&0),
            *parts.get(1).unwrap_or(&0),
            *parts.get(2).unwrap_or(&0),
        );

        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "termide");
        res.set("FileDescription", "Termide — terminal workspace");
        res.set("OriginalFilename", "termide.exe");
        res.set("LegalCopyright", "MIT License");
        res.set_version_info(
            winresource::VersionInfo::PRODUCTVERSION,
            major << 48 | minor << 32 | patch << 16,
        );
        res.set_version_info(
            winresource::VersionInfo::FILEVERSION,
            major << 48 | minor << 32 | patch << 16,
        );
        // Metadata only: a missing resource compiler (`rc.exe`, or
        // `llvm-rc`/`windres` when cross-building) leaves the binary without
        // it rather than failing the build.
        if let Err(error) = res.compile() {
            println!("cargo:warning=no Windows version resource embedded: {error}");
        }
    }
}
