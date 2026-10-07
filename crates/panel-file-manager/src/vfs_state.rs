//! VFS (Virtual File System) state and operations for FileManager.
//!
//! This module provides the integration layer between FileManager and the VFS system,
//! enabling support for network filesystems (SFTP, FTP, SMB, NFS) alongside local files.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use termide_core::{CredentialAttempt, SecretText};
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
    /// A remote file or directory being created. Its completion has to reach
    /// the FileManager so the listing reloads and the new entry is revealed.
    CreateEntry {
        /// Pending remote write / mkdir.
        op: VfsOperation<()>,
        /// Local-equivalent path of the new entry, selected after the reload.
        reveal: PathBuf,
        /// Whether a directory was requested, for the status message.
        is_dir: bool,
    },
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

/// Password handling for one remote connection attempt.
#[derive(Debug, Default)]
struct RemoteLogin {
    /// The password the connection in flight uses.
    attempt: Option<CredentialAttempt>,
    /// A refused connection waiting for a password: the path to open once
    /// it comes. Its URL is the key the app answers with.
    waiting: Option<VfsPath>,
    /// Raised on refusal; taken by the FileManager on the next tick.
    request: Option<(String, CredentialAttempt)>,
    /// Raised when a password-authenticated connection succeeded.
    accepted: Option<String>,
}

/// URL that identifies a remote login: the server root, password-free.
fn login_url(path: &VfsPath) -> String {
    let mut root = path.clone();
    root.path = PathBuf::from("/");
    root.to_url_string()
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
    /// Remote login state: which password the connection in flight uses,
    /// and the path waiting for one. See `termide_core::credentials`.
    login: RemoteLogin,
    /// When connection started (for elapsed time display).
    connection_started: Option<Instant>,
    /// A remote symlink that resolved to a file and should be opened in
    /// the editor. Taken by the FileManager on the next tick.
    resolved_file_open: Option<VfsPath>,
    /// Outcome of a finished remote create: the new entry's local-equivalent
    /// path and whether it is a directory. Taken by the FileManager on the
    /// next tick.
    completed_create: Option<VfsResult<(PathBuf, bool)>>,
    /// An archive that needs a password: its root, and whether a password
    /// was given and rejected. Taken by the FileManager on the next tick.
    password_request: Option<(VfsPath, bool)>,
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
            login: RemoteLogin::default(),
            connection_started: None,
            resolved_file_open: None,
            completed_create: None,
            password_request: None,
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
            login: RemoteLogin::default(),
            connection_started: None,
            resolved_file_open: None,
            completed_create: None,
            password_request: None,
        }
    }

    /// Get reference to the VFS manager.
    pub fn manager(&self) -> &VfsManager {
        &self.manager
    }

    /// Get shared reference to the VFS manager (for remote reads and transfers).
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

    /// Check if a refused remote connection is waiting for a password.
    pub fn awaiting_password(&self) -> bool {
        self.login.waiting.is_some()
    }

    /// Take the request for a remote password: the login URL and which
    /// password was just refused.
    pub fn take_credential_request(&mut self) -> Option<(String, CredentialAttempt)> {
        self.login.request.take()
    }

    /// Take the URL of a login whose password was just accepted.
    pub fn take_accepted_login(&mut self) -> Option<String> {
        self.login.accepted.take()
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
            self.connection_status = Some(connecting_status(&path));
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

    /// Open the archive file `archive` (local, remote or inside an open
    /// archive) at its root. The table of contents is read asynchronously; a
    /// failure returns to the current directory.
    pub fn enter_archive(&mut self, archive: VfsPath) {
        self.previous_path = Some(self.current_path.clone());
        self.current_path = VfsPath::archive(archive, "/");
        self.start_list_dir();
    }

    /// Whether the panel shows the root of an archive, where going up leaves
    /// the archive.
    pub fn at_archive_root(&self) -> bool {
        self.current_path.is_archive() && self.current_path.parent().is_none()
    }

    /// At an archive's root, return to the directory holding the archive file
    /// and close the archive. Returns the archive's file name, for placing
    /// the cursor on it, or `None` when not at an archive root.
    pub fn leave_archive(&mut self) -> Option<String> {
        if !self.at_archive_root() {
            return None;
        }
        let archive = self.current_path.container()?.clone();
        let directory = archive.parent()?;
        // An extraction still running keeps its own handle on the archive.
        self.manager.disconnect(&self.current_path.connection_key());
        self.previous_path = None;
        self.current_path = directory;
        archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Begin resolving a remote symlink entry. A remote directory listing
    /// reports a symlink with its own type, not its target's, so we can't
    /// tell from the listing whether it points to a directory or a file.
    /// This issues a `stat` (which follows the link) and stores a pending
    /// [`PendingVfsOperation::ResolveSymlink`]; `tick()` then navigates
    /// into it (directory) or hands it back to be opened (file).
    pub fn start_resolve_symlink(&mut self, name: &str) {
        let target = self.current_path.join(name);
        self.connection_status = Some(termide_i18n::t().status_vfs_resolving_link().to_string());
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

    /// Start a remote create, to be polled by `tick` instead of blocked on.
    ///
    /// Returns `false` when another VFS operation is already in flight:
    /// `pending_operation` is a single slot, and replacing a directory listing
    /// would leave the panel waiting for a result that never arrives.
    pub fn start_create(&mut self, op: VfsOperation<()>, reveal: PathBuf, is_dir: bool) -> bool {
        if self.pending_operation.is_some() {
            return false;
        }
        self.pending_operation = Some(PendingVfsOperation::CreateEntry { op, reveal, is_dir });
        true
    }

    /// Take the pending request for an archive password, if any.
    pub fn take_password_request(&mut self) -> Option<(VfsPath, bool)> {
        self.password_request.take()
    }

    /// Open the encrypted archive whose root is `root` with `password`. A
    /// wrong password asks again through [`Self::take_password_request`].
    pub fn enter_archive_with_password(&mut self, root: VfsPath, password: String) {
        self.previous_path = Some(self.current_path.clone());
        self.connection_status = Some(connecting_status(&root));
        self.connection_started = Some(Instant::now());
        let operation = self
            .manager
            .connect_archive(&root, ConnectOptions::with_password(password));
        self.pending_operation = Some(PendingVfsOperation::Connect(operation));
        self.current_path = root;
    }

    /// Take the outcome of a finished remote create, if one is ready.
    pub fn take_completed_create(&mut self) -> Option<VfsResult<(PathBuf, bool)>> {
        self.completed_create.take()
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
            VfsProtocol::Archive => self
                .manager
                .connect_archive(&path, ConnectOptions::default()),
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
            self.connection_status = Some(connecting_status(&self.current_path));
            self.connection_started = Some(Instant::now());
            // Start connection - tick() will call start_list_dir() again after connection completes
            if let Err(e) = self.start_connect(self.current_path.clone()) {
                log::error!("VfsState: Failed to start connection: {}", e);
                self.connection_status = None;
            }
            return;
        }

        // Set connection status to show loading spinner
        self.connection_status = Some(termide_i18n::t().status_vfs_loading().to_string());
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
                        // Still pending, put it back.
                        self.pending_operation = Some(PendingVfsOperation::ListDir(op));
                        None
                    }
                }
            }
            PendingVfsOperation::Connect(op) => {
                match op.try_recv() {
                    Some(Ok(())) => {
                        // Connection succeeded, start listing
                        if matches!(
                            self.login.attempt.take(),
                            Some(CredentialAttempt::Stored | CredentialAttempt::Typed)
                        ) {
                            self.login.accepted = Some(login_url(&self.current_path));
                        }
                        self.connection_status =
                            Some(termide_i18n::t().status_vfs_connected().to_string());
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
                    Some(Err(VfsError::AuthenticationFailed(msg)))
                        if takes_password(self.current_path.protocol) =>
                    {
                        // Ask for a password instead of reporting an error;
                        // the panel shows where it was meanwhile.
                        log::info!("VfsState: authentication refused: {msg}");
                        let attempt = self
                            .login
                            .attempt
                            .take()
                            .unwrap_or(CredentialAttempt::Initial);
                        self.login.request = Some((login_url(&self.current_path), attempt));
                        self.login.waiting = Some(self.current_path.clone());
                        self.connection_status = None;
                        self.clear_connection_tracking();
                        if let Some(prev) = self.previous_path.take() {
                            self.current_path = prev;
                        }
                        None
                    }
                    Some(Err(e @ (VfsError::PasswordRequired | VfsError::WrongPassword)))
                        if self.current_path.is_archive() =>
                    {
                        // Ask for the password instead of reporting an error;
                        // the panel stays where it was meanwhile.
                        let wrong = matches!(e, VfsError::WrongPassword);
                        self.password_request = Some((self.current_path.clone(), wrong));
                        self.connection_status = None;
                        self.clear_connection_tracking();
                        if let Some(prev) = self.previous_path.take() {
                            self.current_path = prev;
                        }
                        None
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
            PendingVfsOperation::CreateEntry { op, reveal, is_dir } => {
                match op.try_recv() {
                    Some(Ok(())) => {
                        self.completed_create = Some(Ok((reveal, is_dir)));
                        None
                    }
                    Some(Err(e)) => {
                        log::error!("VfsState: remote create failed: {}", e);
                        self.completed_create = Some(Err(e));
                        None
                    }
                    None => {
                        // Still pending, put it back.
                        self.pending_operation =
                            Some(PendingVfsOperation::CreateEntry { op, reveal, is_dir });
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

    /// Retry the refused login for `url` with `password`. Returns false when
    /// this panel is not waiting for `url`.
    pub fn provide_password(
        &mut self,
        url: &str,
        password: &SecretText,
        source: CredentialAttempt,
    ) -> bool {
        let Some(path) = self.login.waiting.take_if(|p| login_url(p) == url) else {
            return false;
        };
        let options = ConnectOptions::with_password(password.expose());
        let operation = match path.protocol {
            VfsProtocol::Sftp => self.manager.connect_sftp(&path, options),
            VfsProtocol::Ftp | VfsProtocol::Ftps => self.manager.connect_ftp(&path, options),
            VfsProtocol::Smb => self.manager.connect_smb(&path, options),
            _ => return false,
        };
        self.login.attempt = Some(source);
        self.connection_status = Some(connecting_status(&path));
        self.connection_started = Some(Instant::now());
        self.previous_path = Some(self.current_path.clone());
        self.current_path = path;
        self.pending_operation = Some(PendingVfsOperation::Connect(operation));
        true
    }

    /// The user declined to give a password for `url`.
    pub fn cancel_password(&mut self, url: &str) -> bool {
        self.login
            .waiting
            .take_if(|p| login_url(p) == url)
            .is_some()
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
            self.login = RemoteLogin::default();
            return Some(termide_i18n::t().status_vfs_cancelled().to_string());
        }
        // Other operations just get dropped
        self.login = RemoteLogin::default();
        None
    }

    /// Clear connection tracking state (called after connection completes).
    fn clear_connection_tracking(&mut self) {
        self.connection_started = None;
    }
}

/// Whether a refused login on `protocol` can be retried with a password.
fn takes_password(protocol: VfsProtocol) -> bool {
    matches!(
        protocol,
        VfsProtocol::Sftp | VfsProtocol::Ftp | VfsProtocol::Ftps | VfsProtocol::Smb
    )
}

/// Status line while the provider for `path` is being set up.
fn connecting_status(path: &VfsPath) -> String {
    let t = termide_i18n::t();
    match path.container() {
        Some(archive) => t.status_vfs_opening(
            &archive
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default(),
        ),
        None => t.status_vfs_connecting(path.host.as_deref().unwrap_or("remote")),
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

    /// A remote create must not block the UI thread: `start_create` parks the
    /// operation and `tick` hands the outcome back through
    /// `take_completed_create`.
    #[test]
    fn a_finished_create_is_handed_back_by_tick() {
        let mut state = VfsState::new();
        let reveal = PathBuf::from("/remote/dir/notes.txt");

        assert!(state.start_create(VfsOperation::ready(Ok(())), reveal.clone(), false));
        assert!(state.has_pending_operation());
        // Nothing is ready to report until the operation is polled.
        assert!(state.take_completed_create().is_none());

        assert!(
            state.tick().is_none(),
            "a create is not a directory listing"
        );

        let (path, is_dir) = state
            .take_completed_create()
            .expect("the create should have completed")
            .expect("and succeeded");
        assert_eq!(path, reveal);
        assert!(!is_dir);
        assert!(!state.has_pending_operation());
        assert!(state.take_completed_create().is_none(), "taken only once");
    }

    #[test]
    fn a_failed_create_is_reported_as_an_error() {
        let mut state = VfsState::new();
        let op = VfsOperation::ready(Err(VfsError::RemoteError {
            message: "permission denied".to_string(),
        }));

        assert!(state.start_create(op, PathBuf::from("/remote/dir/sub"), true));
        state.tick();

        let result = state.take_completed_create().expect("an outcome");
        assert!(result.is_err());
    }

    /// `pending_operation` is a single slot, so a create must not evict an
    /// in-flight listing — the panel would then wait on a result that never
    /// arrives.
    #[test]
    fn a_create_does_not_evict_another_pending_operation() {
        let mut state = VfsState::new();
        state.start_list_dir();
        assert!(state.has_pending_operation());

        assert!(!state.start_create(
            VfsOperation::ready(Ok(())),
            PathBuf::from("/remote/dir/notes.txt"),
            false
        ));
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

    fn tick_until_done(state: &mut VfsState) -> Option<VfsResult<Vec<VfsEntry>>> {
        for _ in 0..500 {
            if let Some(result) = state.tick() {
                return Some(result);
            }
            if !state.has_pending_operation() {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the VFS operation never finished");
    }

    /// A one-file zip written with the stored method.
    fn write_zip(path: &Path) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        zip.start_file("docs/readme.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hi").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn entering_an_archive_lists_its_root_and_leaving_returns_to_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("pack.zip");
        write_zip(&archive);
        let mut state = VfsState::with_path(VfsPath::local(dir.path()), None);

        state.enter_archive(VfsPath::local(&archive));
        assert_eq!(state.connection_status(), Some("Opening pack.zip..."));
        let entries = tick_until_done(&mut state).unwrap().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "docs");
        assert!(state.at_archive_root());
        assert!(state.manager().is_connected(state.current_path()));

        let root = state.current_path().clone();
        state.navigate_down("docs");
        assert!(!state.at_archive_root());
        assert_eq!(state.leave_archive(), None, "only the root leaves");
        state.navigate_up();

        assert_eq!(state.leave_archive(), Some("pack.zip".to_string()));
        assert_eq!(state.current_path(), &VfsPath::local(dir.path()));
        assert!(
            !state.manager().is_connected(&root),
            "the archive is closed"
        );
    }

    #[test]
    fn a_broken_archive_returns_to_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("broken.zip");
        std::fs::write(&archive, b"PK\x03\x04 but nothing else").unwrap();
        let mut state = VfsState::with_path(VfsPath::local(dir.path()), None);

        state.enter_archive(VfsPath::local(&archive));
        let result = tick_until_done(&mut state).unwrap();
        assert!(result.unwrap_err().is_archive_error());
        assert_eq!(state.current_path(), &VfsPath::local(dir.path()));
        assert!(state.connection_status().is_none());
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
        state.login.waiting = Some(VfsPath::remote(VfsProtocol::Sftp, "h", "/x"));
        let _ = state.cancel_pending();
        assert!(!state.awaiting_password());
    }

    // =========================================================================
    // Remote login (password requests)
    // =========================================================================

    #[test]
    fn login_url_is_the_server_root() {
        let path = VfsPath::remote(VfsProtocol::Sftp, "h", "/srv/data").with_username("bob");
        assert_eq!(login_url(&path), "sftp://bob@h/");
    }

    #[test]
    fn a_password_for_another_login_is_not_taken() {
        let mut state = VfsState::new();
        let path = VfsPath::remote(VfsProtocol::Sftp, "h", "/srv").with_username("bob");
        state.login.waiting = Some(path);
        let pw = SecretText::new("pw");
        assert!(!state.provide_password("sftp://alice@h/", &pw, CredentialAttempt::Typed));
        assert!(state.awaiting_password());
        assert!(!state.cancel_password("sftp://alice@h/"));
        assert!(state.cancel_password("sftp://bob@h/"));
        assert!(!state.awaiting_password());
    }

    #[test]
    fn a_provided_password_starts_the_connection_to_the_waiting_path() {
        let mut state = VfsState::new();
        let path = VfsPath::remote(VfsProtocol::Sftp, "127.0.0.1", "/srv").with_port(1);
        state.login.waiting = Some(path.clone());
        let pw = SecretText::new("pw");
        assert!(state.provide_password(&login_url(&path), &pw, CredentialAttempt::Stored));
        assert!(!state.awaiting_password());
        assert!(state.is_connecting());
        assert_eq!(state.current_path(), &path);
        assert_eq!(state.login.attempt, Some(CredentialAttempt::Stored));
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
