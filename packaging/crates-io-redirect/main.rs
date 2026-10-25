//! Redirect shim for the obsolete `termide` crate on crates.io.
//!
//! The real Termide is a Cargo workspace distributed via GitHub, not
//! crates.io. Anyone who ends up with this binary (e.g. via
//! `cargo install termide`) gets a clear pointer instead of a stale,
//! half-broken build of an ancient release.

fn main() {
    eprintln!(
        "\
This `termide` package from crates.io is OBSOLETE and is NOT the Termide IDE.

Termide is a multi-crate Cargo workspace and is not published to crates.io.
Install the real thing from GitHub instead:

    cargo install --git https://github.com/termide/termide --locked

or download a prebuilt binary / run the installer:

    https://github.com/termide/termide
"
    );
    std::process::exit(1);
}
