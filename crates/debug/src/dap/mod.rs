//! A Debug Adapter Protocol client: debug adapters run as child processes
//! that speak DAP on their standard input and output.

mod adapter;
mod session;
mod transport;

pub use adapter::{debugpy_available, find_in_path, Adapter};
pub use session::{Frame, LaunchSpec, OutputKind, OutputLine, Session, Status, Variable};
pub use transport::split_words;

/// A session shared by the app, which starts and polls it, and the debug
/// panel, which shows and drives it; `None` before the first launch.
pub type SharedSession = std::sync::Arc<std::sync::Mutex<Option<Session>>>;
