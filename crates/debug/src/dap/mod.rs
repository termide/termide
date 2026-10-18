//! A Debug Adapter Protocol client: debug adapters run as child processes
//! that speak DAP on their standard input and output.

mod adapter;
mod session;
mod transport;

pub use adapter::{
    debugpy_available, debugpy_available_with, find_in_path, resolve_program, Adapter, Console,
};
pub use session::{
    Frame, LaunchSpec, OutputKind, OutputLine, Session, Status, TerminalRequest, Variable,
};
pub use termide_config::split_words;

/// A session shared by the app, which starts and polls it, and the debug
/// panel, which shows and drives it; `None` before the first launch.
pub type SharedSession = std::sync::Arc<std::sync::Mutex<Option<Session>>>;
