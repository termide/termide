//! Detached instances for termide.
//!
//! A detached instance is an ordinary termide running inside a PTY owned by a
//! daemonised parent. Clients attach to that daemon over a unix socket and
//! pump bytes; closing a client leaves the daemon, the hosted termide and
//! everything it spawned — shells, LSP servers, watchers — untouched.
//!
//! The split is deliberate. Nothing about the running instance is serialised,
//! so nothing can be lost in translation: reattaching re-enters the terminal
//! modes and repaints, and the state was never anywhere but in the still-live
//! process.
//!
//! Unix only. Windows has no `fork`/`setsid` and its ConPTY model needs a
//! different host, so the CLI surface is compiled out there.

pub mod paths;
pub mod protocol;
pub mod registry;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod reattach;

#[cfg(unix)]
pub use daemon::{
    hosted_instance_id, is_hosted, request_detach_from_host, spawn_detached, ID_ENV, SOCKET_ENV,
};
#[cfg(unix)]
pub use reattach::{
    adopt_client_terminal, install_handler as install_reattach_handler,
    take_request as take_reattach_request, ClientTerminal,
};

use anyhow::Result;

/// Render the detached-instance list as a table, or a hint when there is none.
pub fn format_instance_list() -> Result<String> {
    let instances = registry::list()?;
    if instances.is_empty() {
        return Ok("No detached instances. Start one with `termide --detached`.\n".to_string());
    }

    let id_width = instances
        .iter()
        .map(|s| s.id.len())
        .chain(std::iter::once("ID".len()))
        .max()
        .unwrap_or(2);

    let mut out = format!(
        "{:<id_width$}  {:<8}  {:<7}  {:<8}  {}\n",
        "ID",
        "PID",
        "UPTIME",
        "STATE",
        "PROJECT",
        id_width = id_width
    );
    for instance in instances {
        out.push_str(&format!(
            "{:<id_width$}  {:<8}  {:<7}  {:<8}  {}\n",
            instance.id,
            instance.pid,
            instance.uptime(),
            if instance.attached {
                "attached"
            } else {
                "detached"
            },
            instance.project.display(),
            id_width = id_width
        ));
    }
    Ok(out)
}
