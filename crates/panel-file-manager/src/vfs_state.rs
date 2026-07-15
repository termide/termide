//! VFS (Virtual File System) state and operations for FileManager.
//!
//! This module provides the integration layer between FileManager and the VFS system,
//! enabling support for network filesystems (SFTP, FTP, SMB, NFS) alongside local files.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use termide_vfs::{
    ConnectOptions, DirCache, VfsEntry, VfsError, VfsManager, VfsMetadata, VfsOperation, VfsPath,
    VfsProtocol, VfsResult,
};

/// Result type for pending VFS operations.
pub enum PendingVfsOperation {
    /// Directory listing operation.
    ListDir(VfsOperation<Vec<VfsEntry>>),
    /// Connection operation.
    Connect(VfsOperation<()>),
    /// File read operation (infrastructure for future VFS operations).
    #[allow(dead_code)]
    ReadFile(VfsOperation<Vec<u8>>),
    /// Generic operation (infrastructure for future VFS operations).
    #[allow(dead_code)]
    Generic(VfsOperation<()>),
    /// Resolve a remote symlink's target type (stat follows the link) to
    /// decide whether to navigate into it (directory) or open it (file).
    ResolveSymlink {
        /// Pending metadata lookup on the symlink target.
        op: VfsOperation<VfsMetadata>,
        /// The symlink path (current_path joined with the entry name).
        target: VfsPath,
        /// Entry name, used for `navigate_down` when it resolves to a dir.
        name: String,
    },
}

/// VFS state for FileManager.
///
/// Manages the VFS manager, current path (local or remote), and pending async operations.
pub struct VfsState {
    /// Shared VFS manager.
    manager: Arc<VfsManager>,
    /// Current path (can be local or remote).
    current_path: VfsPath,
    /// Previous path before remote navigation (for restore on failure/cancel).
    previous_path: Option<VfsPath>,
    /// Pending async operation (if any).
    pending_operation: Option<PendingVfsOperation>,
    /// Connection status for display.
    connection_status: Option<String>,
    /// Whether we're waiting for a password from the user.
    awaiting_password: bool,
    /// When connection started (for elapsed time display).
    connection_started: Option<Instant>,
    /// A remote symlink that resolved to a file and should be opened in
    /// the editor. Taken by the FileManager on the next tick.
    resolved_file_open: Option<VfsPath>,
}

impl Default for VfsState {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsState {
    /// Create new VFS state with local filesystem.
    pub fn new() -> Self {
        let current_path = std::env::current_dir()
            .map(VfsPath::local)
            .unwrap_or_else(|_| VfsPath::local("/"));

        Self {
            manager: Arc::new(VfsManager::new()),
            current_path,
            previous_path: None,
            pending_operation: None,
            connection_status: None,
            awaiting_password: false,
            connection_started: None,
            resolved_file_open: None,
        }
    }

    /// Create VFS state for a specific path.
    pub fn with_path(path: VfsPath, manager: Option<Arc<VfsManager>>) -> Self {
        Self {
            manager: manager.unwrap_or_else(|| Arc::new(VfsManager::new())),
            current_path: path,
            previous_path: None,
            pending_operation: None,
            connection_status: None,
            awaiting_password: false,
            connection_started: None,
            resolved_file_open: None,
        }
    }

    /// Get reference to the VFS manager.
    pub fn manager(&self) -> &VfsManager {
        &self.manager
    }

    /// Get shared reference to the VFS manager (for passing to Editor::open_remote_file).
    pub fn manager_arc(&self) -> Arc<VfsManager> {
        Arc::clone(&self.manager)
    }

    /// Get the current path.
    pub fn current_path(&self) -> &VfsPath {
        &self.current_path
    }

    /// Get current path as local PathBuf (for backwards compatibility).
    ///
    /// Returns Some for local paths, None for remote paths.
    pub fn local_path(&self) -> Option<&Path> {
        if self.current_path.is_local() {
            Some(&self.current_path.path)
        } else {
            None
        }
    }

    /// Get current path as PathBuf (for backwards compatibility).
    ///
    /// For remote paths, returns the path component only.
    pub fn path_buf(&self) -> PathBuf {
        self.current_path.path.clone()
    }

    /// Check if current path is local.
    pub fn is_local(&self) -> bool {
        self.current_path.is_local()
    }

    /// Check if current path is remote.
    pub fn is_remote(&self) -> bool {
        self.current_path.is_remote()
    }

    /// Get display string for current path.
    pub fn display_path(&self) -> String {
        self.current_path.to_url_string()
    }

    /// Get connection status message for display.
    pub fn connection_status(&self) -> Option<&str> {
        self.connection_status.as_deref()
    }

    /// Get connection status with elapsed time.
    /// Returns (status_message, elapsed_seconds) if connecting.
    pub fn connection_status_with_elapsed(&self) -> Option<(String, Option<u64>)> {
        if let Some(status) = &self.connection_status {
            let elapsed = self.connection_started.map(|t| t.elapsed().as_secs());
            Some((status.clone(), elapsed))
        } else {
            None
        }
    }

    /// Get elapsed connection time in seconds.
    pub fn connection_elapsed_secs(&self) -> Option<u64> {
        self.connection_started.map(|t| t.elapsed().as_secs())
    }

    /// Check if we're waiting for a password.
    pub fn awaiting_password(&self) -> bool {
        self.awaiting_password
    }

    /// Check if there's a pending operation.
    pub fn has_pending_operation(&self) -> bool {
        self.pending_operation.is_some()
    }

    /// Check if VFS operation is currently in progress (for loading spinners).
    pub fn is_loading(&self) -> bool {
        self.pending_operation.is_some()
    }

    /// Set the current path.
    pub fn set_path(&mut self, path: VfsPath) {
        self.current_path = path;
    }

    /// Navigate to a VfsPath.
    pub fn navigate_to(&mut self, path: VfsPath) -> VfsResult<()> {
        // For local paths, just update current_path
        if path.is_local() {
            if path.path.is_dir() {
                self.current_path = path;
                return Ok(());
            } else if let Some(parent) = path.parent() {
                self.current_path = parent;
                return Ok(());
            }
            return Err(VfsError::NotFound { path: path.path });
        }

        // For remote paths, check if we're connected
        if !self.manager.is_connected(&path) {
            // Save current local path before attempting remote connection
            if self.current_path.is_local() {
                self.previous_path = Some(self.current_path.clone());
            }
            // Need to connect first
            self.connection_status = Some(format!(
                "Connecting to {}...",
                path.host.as_deref().unwrap_or("remote")
            ));
            self.connection_started = Some(Instant::now());
            self.start_connect(path)?;
            return Ok(());
        }

        // Already connected, just navigate
        self.current_path = path;
        Ok(())
    }

    /// Navigate to parent directory.
    /// Returns the current directory name (for cursor restoration) if navigation occurred,
    /// or None if already at root.
    pub fn navigate_up(&mut self) -> Option<String> {
        // Check if we can go up
        let parent = self.current_path.parent()?;

        // Save current directory name for cursor restoration
        let current_name = self
            .current_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());

        self.current_path = parent;
        current_name
    }

    /// Navigate into a subdirectory.
    pub fn navigate_down(&mut self, name: &str) {
        self.current_path = self.current_path.join(name);
    }

    /// Begin resolving a remote symlink entry. A remote directory listing
    /// reports a symlink with its own type, not its target's, so we can't
    /// tell from the listing whether it points to a directory or a file.
    /// This issues a `stat` (which follows the link) and stores a pending
    /// [`PendingVfsOperation::ResolveSymlink`]; `tick()` then navigates
    /// into it (directory) or hands it back to be opened (file).
    pub fn start_resolve_symlink(&mut self, name: &str) {
        let target = self.current_path.join(name);
        self.connection_status = Some("Resolving link...".to_string());
        let op = self.manager.metadata(&target);
        self.pending_operation = Some(PendingVfsOperation::ResolveSymlink {
            op,
            target,
            name: name.to_string(),
        });
    }

    /// Take a remote file path that a symlink resolved to, so the caller
    /// can open it in the editor. Cleared once taken.
    pub fn take_resolved_file_open(&mut self) -> Option<VfsPath> {
        self.resolved_file_open.take()
    }

    /// Start a connection to a remote path.
    fn start_connect(&mut self, path: VfsPath) -> VfsResult<()> {
        // Start async connection based on protocol
        let operation = match path.protocol {
            VfsProtocol::Sftp => {
                // SFTP is enabled by default in termide-vfs
                self.manager.connect_sftp(&path, ConnectOptions::default())
            }
            VfsProtocol::Ftp | VfsProtocol::Ftps => {
                self.manager.connect_ftp(&path, ConnectOptions::default())
            }
            VfsProtocol::Smb => self.manager.connect_smb(&path, ConnectOptions::default()),
            VfsProtocol::Nfs => {
                return Err(VfsError::NotSupported(
                    "NFS connections not yet fully implemented".to_string(),
                ));
            }
            VfsProtocol::Local => {
                // Local paths don't need connection
                return Err(VfsError::InvalidPath(
                    "Local paths don't require connection".to_string(),
                ));
            }
        };

        // Store pending operation - will be polled in tick()
        self.pending_operation = Some(PendingVfsOperation::Connect(operation));
        self.current_path = path;
        Ok(())
    }

    /// Start a directory listing operation.
    ///
    /// For remote paths, this will automatically start a connection first if not connected.
    /// The connection completion will trigger directory listing in tick().
    pub fn start_list_dir(&mut self) {
        // For remote paths, check if connected and start connection if needed
        if self.current_path.is_remote() && !self.manager.is_connected(&self.current_path) {
            self.connection_status = Some(format!(
                "Connecting to {}...",
                self.current_path.host.as_deref().unwrap_or("remote")
            ));
            self.connection_started = Some(Instant::now());
            // Start connection - tick() will call start_list_dir() again after connection completes
            if let Err(e) = self.start_connect(self.current_path.clone()) {
                log::error!("VfsState: Failed to start connection: {}", e);
                self.connection_status = None;
            }
            return;
        }

        // Set connection status to show loading spinner
        self.connection_status = Some("Loading directory...".to_string());
        let operation = self.manager.list_dir(&self.current_path);
        self.pending_operation = Some(PendingVfsOperation::ListDir(operation));
    }

    /// Check pending operations and process results.
    ///
    /// Returns Some(entries) if directory listing completed, None otherwise.
    pub fn tick(&mut self) -> Option<VfsResult<Vec<VfsEntry>>> {
        let operation = self.pending_operation.take()?;

        match operation {
            PendingVfsOperation::ListDir(op) => {
                match op.try_recv() {
                    Some(Ok(entries)) => {
                        // Clear connection status to stop spinner
                        self.connection_status = None;
                        // Operation completed
                        Some(Ok(entries))
                    }
                    Some(Err(e)) => {
                        log::error!("VfsState: ListDir failed: {}", e);

                        // Clear connection status to stop spinner and status messages
                        self.connection_status = None;

                        // Restore previous path
                        if let Some(prev) = self.previous_path.take() {
                            self.current_path = prev;
                        }

                        Some(Err(e))
                    }
                    None => {
                        // Note: Using debug level to avoid flooding logs
                        // termide_logger::debug("VfsState: ListDir still pending".to_string());
                        // Still pending, put it back
                        self.pending_operation = Some(PendingVfsOperation::ListDir(op));
                        None
                    }
                }
            }
            PendingVfsOperation::Connect(op) => {
                match op.try_recv() {
                    Some(Ok(())) => {
                        // Connection succeeded, start listing
                        self.connection_status = Some("Connected".to_string());
                        self.clear_connection_tracking();

                        // If current path is root ("/") or empty, navigate to home directory
                        let path_str = self.current_path.path.to_string_lossy();
                        let is_root = path_str == "/" || path_str.is_empty();
                        if is_root {
                            if let Some(home) = self.manager.get_home_dir(&self.current_path) {
                                self.current_path = home;
                            }
                        }

                        self.start_list_dir();
                        None
                    }
                    Some(Err(VfsError::AuthenticationFailed(msg))) => {
                        // Treat authentication failure as a regular error
                        // Password modal not yet implemented
                        let e = VfsError::AuthenticationFailed(msg);
                        log::error!("VfsState: Authentication failed: {}", e);
                        self.connection_status = None;
                        self.clear_connection_tracking();
                        // Restore previous path
                        if let Some(prev) = self.previous_path.take() {
                            self.current_path = prev;
                        }
                        Some(Err(e))
                    }
                    Some(Err(e)) => {
                        log::error!("VfsState: Connection failed: {}", e);
                        // Connection failed - clear status (error shown via modal)
                        self.connection_status = None;
                        self.clear_connection_tracking();
                        // Restore previous path
                        if let Some(prev) = self.previous_path.take() {
                            self.current_path = prev;
                        }
                        Some(Err(e))
                    }
                    None => {
                        // Still connecting, put it back
                        self.pending_operation = Some(PendingVfsOperation::Connect(op));
                        None
                    }
                }
            }
            PendingVfsOperation::ReadFile(op) => {
                match op.try_recv() {
                    Some(_result) => {
                        // File read completed (caller handles result)
                        None
                    }
                    None => {
                        // Still pending
                        self.pending_operation = Some(PendingVfsOperation::ReadFile(op));
                        None
                    }
                }
            }
            PendingVfsOperation::Generic(op) => {
                match op.try_recv() {
                    Some(_result) => {
                        // Operation completed
                        None
                    }
                    None => {
                        // Still pending
                        self.pending_operation = Some(PendingVfsOperation::Generic(op));
                        None
                    }
                }
            }
            PendingVfsOperation::ResolveSymlink { op, target, name } => {
                match op.try_recv() {
                    Some(Ok(meta)) => {
                        if meta.file_type.is_dir() {
                            // Symlink points to a directory — navigate into it.
                            // Reuse the directory-listing path so an error
                            // (e.g. permission denied) restores the prior path.
                            self.previous_path = Some(self.current_path.clone());
                            self.navigate_down(&name);
                            self.start_list_dir();
                            None
                        } else {
                            // Symlink points to a file — hand it back to the
                            // FileManager to open in the editor.
                            self.connection_status = None;
                            self.resolved_file_open = Some(target);
                            None
                        }
                    }
                    Some(Err(e)) => {
                        log::error!("VfsState: symlink resolve failed: {}", e);
                        self.connection_status = None;
                        Some(Err(e))
                    }
                    None => {
                        // Still pending
                        self.pending_operation =
                            Some(PendingVfsOperation::ResolveSymlink { op, target, name });
                        None
                    }
                }
            }
        }
    }

    /// Provide password for pending authentication.
    ///
    /// Retries the connection to the current remote path using password auth.
    pub fn provide_password(&mut self, password: String) {
        self.awaiting_password = false;

        if !self.current_path.is_remote() {
            return;
        }

        let options = ConnectOptions::with_password(password);
        self.connection_status = Some(format!(
            "Connecting to {}...",
            self.current_path.host.as_deref().unwrap_or("remote")
        ));
        self.connection_started = Some(Instant::now());

        let operation = match self.current_path.protocol {
            VfsProtocol::Sftp => self.manager.connect_sftp(&self.current_path, options),
            VfsProtocol::Ftp | VfsProtocol::Ftps => {
                self.manager.connect_ftp(&self.current_path, options)
            }
            VfsProtocol::Smb => self.manager.connect_smb(&self.current_path, options),
            _ => {
                self.connection_status = None;
                return;
            }
        };

        self.pending_operation = Some(PendingVfsOperation::Connect(operation));
    }

    /// Cancel pending authentication.
    pub fn cancel_auth(&mut self) {
        self.awaiting_password = false;
        self.connection_status = None;
        // Navigate back to local home if remote auth failed
        if let Some(home) = dirs::home_dir() {
            self.current_path = VfsPath::local(home);
        }
    }

    /// Drop the (possibly dead) provider for the current remote path and start
    /// a fresh connection to the same path. A no-op for local paths.
    pub fn reconnect(&mut self) {
        if self.current_path.is_remote() {
            let key = self.current_path.connection_key();
            self.manager.disconnect(&key);
            self.connection_status = None;
            self.pending_operation = None;
        }
        // `start_list_dir` now sees the path as disconnected and re-connects.
        self.start_list_dir();
    }

    /// Disconnect from current remote.
    pub fn disconnect(&mut self) {
        if self.current_path.is_remote() {
            let key = self.current_path.connection_key();
            self.manager.disconnect(&key);
            self.connection_status = None;
            // Navigate back to local home
            if let Some(home) = dirs::home_dir() {
                self.current_path = VfsPath::local(home);
            }
        }
    }

    /// Get cache for directory listings.
    pub fn cache(&self) -> &DirCache {
        self.manager.cache()
    }

    /// Invalidate cache for current path.
    pub fn invalidate_cache(&mut self) {
        self.manager
            .cache()
            .invalidate_with_parent(&self.current_path);
    }

    /// Check if a path exists.
    pub fn exists(&self, path: &VfsPath) -> bool {
        if path.is_local() {
            path.path.exists()
        } else {
            // For remote paths, we can't do synchronous check easily
            // Assume exists if connected
            self.manager.is_connected(path)
        }
    }

    /// Create local VfsPath from PathBuf.
    pub fn local_vfs_path(&self, path: PathBuf) -> VfsPath {
        VfsPath::local(path)
    }

    /// Join current path with a name.
    pub fn join(&self, name: &str) -> VfsPath {
        self.current_path.join(name)
    }

    /// Check if currently connecting to a remote.
    pub fn is_connecting(&self) -> bool {
        matches!(
            &self.pending_operation,
            Some(PendingVfsOperation::Connect(_))
        )
    }

    /// Cancel any pending operation.
    /// Returns Some(message) if a connection was cancelled for modal display.
    pub fn cancel_pending(&mut self) -> Option<String> {
        if let Some(PendingVfsOperation::Connect(_)) = self.pending_operation.take() {
            // Connection was cancelled - clear status
            self.connection_status = None;
            self.connection_started = None;
            // Restore to previous path, or home if none
            if let Some(prev) = self.previous_path.take() {
                self.current_path = prev;
            } else if let Some(home) = dirs::home_dir() {
                self.current_path = VfsPath::local(home);
            }
            self.awaiting_password = false;
            return Some("Connection cancelled".to_string());
        }
        // Other operations just get dropped
        self.awaiting_password = false;
        None
    }

    /// Clear connection tracking state (called after connection completes).
    fn clear_connection_tracking(&mut self) {
        self.connection_started = None;
    }
}

/// Drop implementation ensures cleanup when VfsState is dropped.
impl Drop for VfsState {
    fn drop(&mut self) {
        // Cancel any pending operation (ignore returned message)
        let _ = self.cancel_pending();

        // Disconnect from any remote connections
        if self.current_path.is_remote() {
            let key = self.current_path.connection_key();
            self.manager.disconnect(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vfs_state_default() {
        let state = VfsState::new();
        assert!(state.is_local());
        assert!(!state.is_remote());
        assert!(!state.has_pending_operation());
        assert!(!state.awaiting_password());
    }

    #[test]
    fn test_vfs_state_local_navigation() {
        let mut state = VfsState::new();

        // Navigate to temp directory
        let temp = std::env::temp_dir();
        let temp_path = VfsPath::local(&temp);
        assert!(state.navigate_to(temp_path).is_ok());
        assert_eq!(state.path_buf(), temp);
    }

    #[test]
    fn test_vfs_state_navigate_up() {
        let mut state = VfsState::new();

        // Set up a nested path
        let path = VfsPath::local("/home/user/documents");
        state.set_path(path);

        let name = state.navigate_up();
        assert_eq!(name, Some("documents".to_string()));
        assert_eq!(state.path_buf(), PathBuf::from("/home/user"));
    }

    #[test]
    fn test_vfs_state_navigate_down() {
        let mut state = VfsState::new();

        let path = VfsPath::local("/home/user");
        state.set_path(path);

        state.navigate_down("documents");
        assert_eq!(state.path_buf(), PathBuf::from("/home/user/documents"));
    }

    #[test]
    fn test_vfs_state_display_path() {
        let state = VfsState::new();
        // Should display current path as string
        assert!(!state.display_path().is_empty());
    }

    #[test]
    fn test_start_resolve_symlink_sets_pending_with_target() {
        // Entering a remote symlink must resolve its target type (not open
        // it as a file), so `start_resolve_symlink` queues a ResolveSymlink
        // op carrying the joined target path and the entry name.
        let mut state = VfsState::new();
        state.set_path(termide_vfs::parse_vfs_url("sftp://host/dir").unwrap());

        state.start_resolve_symlink("link");

        match &state.pending_operation {
            Some(PendingVfsOperation::ResolveSymlink { target, name, .. }) => {
                assert_eq!(name, "link");
                assert_eq!(target.path, PathBuf::from("/dir/link"));
            }
            _ => panic!("expected a pending ResolveSymlink operation"),
        }
        assert!(state.has_pending_operation());
        // Nothing to hand back to the editor until the stat resolves.
        assert!(state.take_resolved_file_open().is_none());
    }

    // =========================================================================
    // Cancel pending connection
    // =========================================================================

    #[test]
    fn test_cancel_pending_no_operation() {
        let mut state = VfsState::new();
        // No pending operation — should return None
        let result = state.cancel_pending();
        assert!(result.is_none());
    }

    #[test]
    fn test_cancel_pending_clears_state() {
        let mut state = VfsState::new();
        // Set some state that cancel_pending would clear
        state.awaiting_password = true;
        let _ = state.cancel_pending();
        assert!(!state.awaiting_password());
    }

    // =========================================================================
    // Previous path restoration
    // =========================================================================

    #[test]
    fn test_previous_path_stored_on_remote_navigate() {
        let mut state = VfsState::new();
        let local_path = VfsPath::local("/home/user/documents");
        state.set_path(local_path.clone());

        // previous_path is None initially
        assert!(state.previous_path.is_none());
    }

    #[test]
    fn test_with_path_constructor() {
        let path = VfsPath::local("/custom/path");
        let state = VfsState::with_path(path.clone(), None);
        assert_eq!(state.path_buf(), PathBuf::from("/custom/path"));
        assert!(state.is_local());
    }

    // =========================================================================
    // Connection status
    // =========================================================================

    #[test]
    fn test_connection_status_initially_none() {
        let state = VfsState::new();
        assert!(state.connection_status().is_none());
        assert!(state.connection_elapsed_secs().is_none());
    }

    #[test]
    fn test_is_not_connecting_initially() {
        let state = VfsState::new();
        assert!(!state.is_connecting());
        assert!(!state.is_loading());
    }

    // =========================================================================
    // VfsState path operations
    // =========================================================================

    #[test]
    fn test_join_path() {
        let mut state = VfsState::new();
        state.set_path(VfsPath::local("/home/user"));
        let joined = state.join("documents");
        assert_eq!(joined.path, PathBuf::from("/home/user/documents"));
    }

    #[test]
    fn test_local_vfs_path() {
        let state = VfsState::new();
        let path = state.local_vfs_path(PathBuf::from("/test/path"));
        assert!(path.is_local());
        assert_eq!(path.path, PathBuf::from("/test/path"));
    }

    #[test]
    fn test_exists_local_path() {
        let state = VfsState::new();
        let temp = std::env::temp_dir();
        let path = VfsPath::local(&temp);
        assert!(state.exists(&path));
    }
}
