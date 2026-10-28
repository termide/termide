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
        None => pkg,
    };

    println!("cargo:rustc-env=TERMIDE_VERSION={version}");
}
