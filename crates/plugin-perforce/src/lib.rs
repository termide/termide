//! termide's built-in Perforce plugin.
//!
//! termide runs it as `termide --plugin perforce`, a process of its own
//! that speaks the plugin protocol (`doc/en/plugins.md`) on stdin and
//! stdout like any installed plugin: it starts only in a project that
//! enables it, and stops with the project.
//!
//! - [`p4`]: running `p4` and reading what it prints.
//! - [`termide`]: the plugin's end of the protocol.
//! - `plugin`: what the plugin does with it.

pub mod p4;
mod plugin;
pub mod termide;

use std::io::{Read, Write};
use std::path::PathBuf;

pub use plugin::describe;

/// The plugin's name, as config's `[plugins.perforce]` names it.
pub const NAME: &str = "perforce";
/// One line on what it does.
pub const DESCRIPTION: &str =
    "Perforce: the file's state, open for edit, revert, diff, opened files, submit and sync";

/// Serve termide on stdin and stdout, running the `p4` found on `PATH`.
pub fn run() {
    serve(std::io::stdin(), std::io::stdout(), PathBuf::from("p4"));
}

/// Serve termide over `reader` and `writer`, running `program` as `p4`,
/// until termide says `exit` or closes the connection.
pub fn serve(
    reader: impl Read + Send + 'static,
    writer: impl Write + Send + 'static,
    program: PathBuf,
) {
    termide::serve(reader, writer, move |termide, params| {
        let root = params["project"]["root"]
            .as_str()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let plugin = plugin::Perforce::new(termide.clone(), p4::P4::new(program, root));
        (plugin, plugin::contributions())
    });
}
