//! The tokio runtime the async network clients (SMB, NFS) run on, and the
//! bridge from the blocking [`VfsOperation`] surface to their futures.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

use crate::error::{VfsError, VfsResult};
use crate::types::VfsOperation;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The shared runtime, built on first use. An error rather than a panic when
/// the OS refuses the worker threads: the caller may be the UI thread.
pub(crate) fn runtime() -> VfsResult<&'static Runtime> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("vfs-net")
        .enable_all()
        .build()
        .map_err(|e| VfsError::RemoteError {
            message: format!("failed to build the network runtime: {e}"),
        })?;
    // A racing thread may have won the init; the loser's runtime is dropped.
    Ok(RUNTIME.get_or_init(|| rt))
}

/// Run `fut` on the runtime and hand its result over as a [`VfsOperation`],
/// without blocking the caller.
pub(crate) fn spawn<T, F>(fut: F) -> VfsOperation<T>
where
    T: Send + 'static,
    F: Future<Output = VfsResult<T>> + Send + 'static,
{
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(e) => return VfsOperation::error(e),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    rt.spawn(async move {
        let _ = tx.send(fut.await);
    });
    VfsOperation::new(rx)
}

/// Run `fut` on the runtime and wait for it. Not to be called from inside
/// the runtime.
pub(crate) fn block_on<T, F>(fut: F) -> VfsResult<T>
where
    F: Future<Output = VfsResult<T>>,
{
    runtime()?.block_on(fut)
}
