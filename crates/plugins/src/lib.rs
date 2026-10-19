//! termide plugins: programs installed in the global plugins directory,
//! run beside termide for each open project and spoken to over JSON-RPC on
//! their stdin and stdout.
//!
//! - [`manifest`]: what is installed, and the program each one runs.
//! - [`host`]: the running processes, supervised and restarted.
//! - [`protocol`]: the methods and messages.
//! - [`rpc`]: the two-way JSON-RPC connection.
//!
//! See `doc/en/plugins-design.md`.

pub mod host;
pub mod manifest;
pub mod progress;
pub mod protocol;
pub mod rpc;

use std::path::{Path, PathBuf};

pub use host::{Host, HostEvent, Launch, PluginKey, PluginState, ProcessLauncher, Timing};
pub use manifest::{discover, Installed, Manifest};
pub use protocol::{CommandInfo, Contributions, ToolInfo};
pub use rpc::{Peer, RpcError};

/// The plugins directory inside termide's global config directory.
#[must_use]
pub fn plugins_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("plugins")
}
