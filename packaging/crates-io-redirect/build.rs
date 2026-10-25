// Surface the redirect at install time too: `cargo install termide` prints
// this warning while compiling, before the user even runs the binary.
fn main() {
    println!(
        "cargo:warning=The `termide` crate on crates.io is obsolete and is not the Termide IDE. Install from GitHub: cargo install --git https://github.com/termide/termide --locked"
    );
}
