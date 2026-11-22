//! SMB2/3 VFS provider, on the pure-Rust `smb2` client.
//!
//! One provider is one signed-in session with a server: `smb://host/` lists
//! the server's shares as directories, and the first component of a path is
//! the share, connected the first time a path on it is used. The session and
//! the shares it connected live behind one async mutex, so the calls on a
//! connection go one at a time, as with SFTP.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use smb2::{ErrorKind, SmbClient, Tree};
use tokio::sync::Mutex;

use crate::chunked::{self, ChunkRead, ChunkWrite, ChunkedRemote};
use crate::error::{VfsError, VfsResult};
use crate::traits::VfsProvider;
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, VfsDownloadOperation, VfsEntry, VfsFileType,
    VfsMetadata, VfsOperation, VfsPath, VfsUploadOperation,
};
use crate::{rt, MAX_RECURSION_DEPTH};

/// Default SMB port.
const DEFAULT_PORT: u16 = 445;

/// How long a connection attempt and each request may take, unless the
/// connect options say otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A disk share (`STYPE_DISKTREE`); the high bits mark special and
/// temporary shares.
const STYPE_DISKTREE: u32 = 0;
const STYPE_TYPE_MASK: u32 = 0x0FFF_FFFF;

/// The signed-in session and the shares connected on it.
struct Session {
    client: SmbClient,
    trees: HashMap<String, Tree>,
}

impl Session {
    /// The connected share `name`, connecting it on first use.
    async fn tree(&mut self, name: &str) -> VfsResult<(&mut SmbClient, &mut Tree)> {
        if !self.trees.contains_key(name) {
            let tree = self
                .client
                .connect_share(name)
                .await
                .map_err(|e| map_error(e, Path::new(name)))?;
            self.trees.insert(name.to_string(), tree);
        }
        let tree = self.trees.get_mut(name).expect("inserted above");
        Ok((&mut self.client, tree))
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = VfsResult<T>> + Send + 'a>>;

/// SMB filesystem provider.
pub struct SmbProvider {
    host: String,
    port: u16,
    username: Option<String>,
    state: ConnectionState,
    session: Option<Arc<Mutex<Session>>>,
}

impl SmbProvider {
    /// A provider for the server at `host`, not yet connected. A username
    /// given as `DOMAIN\user` or `DOMAIN;user` signs in to that domain.
    pub fn new(host: &str, port: Option<u16>, username: Option<&str>) -> Self {
        Self {
            host: host.to_string(),
            port: port.unwrap_or(DEFAULT_PORT),
            username: username.map(String::from),
            state: ConnectionState::Disconnected,
            session: None,
        }
    }

    /// The session, for a transfer.
    fn remote(&self) -> VfsResult<Arc<dyn ChunkedRemote>> {
        let session = self.session()?;
        Ok(Arc::new(SmbRemote(Self {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            state: self.state,
            session: Some(session),
        })))
    }

    fn session(&self) -> VfsResult<Arc<Mutex<Session>>> {
        self.session.clone().ok_or(VfsError::NotConnected)
    }

    /// Run `f` on the share `path` is on, with the path inside the share.
    fn on_share<T, F>(&self, path: &VfsPath, f: F) -> VfsOperation<T>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&'a mut SmbClient, &'a mut Tree, String) -> BoxFuture<'a, T>
            + Send
            + 'static,
    {
        let session = match self.session() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let Some((share, inner)) = split_share(path) else {
            return VfsOperation::error(VfsError::NotSupported(
                "the server's root holds shares, not files".to_string(),
            ));
        };
        rt::spawn(async move {
            let mut session = session.lock().await;
            let (client, tree) = session.tree(&share).await?;
            f(client, tree, inner).await
        })
    }
}

/// A second handle on a provider's session, as transfers use it: one call
/// per piece, so other requests get their turn in between. Dropping it
/// leaves the session as it is.
struct SmbRemote(SmbProvider);

impl SmbRemote {
    /// Run `f` on the share `path` is on, from a transfer's thread.
    fn on_share<T, F>(&self, path: &VfsPath, f: F) -> VfsResult<T>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&'a mut SmbClient, &'a mut Tree, String) -> BoxFuture<'a, T>
            + Send
            + 'static,
    {
        self.0.on_share(path, f).recv()
    }
}

impl ChunkedRemote for SmbRemote {
    fn metadata(&self, path: &VfsPath) -> VfsResult<VfsMetadata> {
        self.0.metadata(path).recv()
    }

    fn list(&self, path: &VfsPath) -> VfsResult<Vec<VfsEntry>> {
        self.0.list_dir(path).recv()
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsResult<()> {
        self.0.create_dir_all(path).recv()
    }

    fn open_read(&self, path: &VfsPath) -> VfsResult<Box<dyn ChunkRead>> {
        let shown = path.path.clone();
        let download = self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .open_file_download(tree, &inner)
                    .await
                    .map_err(|e| map_error(e, &shown))
            })
        })?;
        Ok(Box::new(SmbReader {
            download,
            shown: path.path.clone(),
        }))
    }

    fn open_write(&self, path: &VfsPath) -> VfsResult<Box<dyn ChunkWrite>> {
        let shown = path.path.clone();
        let writer = self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .create_file_writer(tree, &inner)
                    .await
                    .map_err(|e| map_error(e, &shown))
            })
        })?;
        Ok(Box::new(SmbWriter {
            writer,
            shown: path.path.clone(),
        }))
    }
}

/// A download that holds its own clone of the connection, not the session.
struct SmbReader {
    download: smb2::FileDownload<'static>,
    shown: PathBuf,
}

impl ChunkRead for SmbReader {
    fn next_chunk(&mut self) -> VfsResult<Option<Vec<u8>>> {
        match rt::block_on(async { Ok(self.download.next_chunk().await) })? {
            Some(piece) => piece.map(Some).map_err(|e| map_error(e, &self.shown)),
            None => Ok(None),
        }
    }
}

/// A writer that holds its own clone of the connection, not the session.
struct SmbWriter {
    writer: smb2::FileWriter,
    shown: PathBuf,
}

impl ChunkWrite for SmbWriter {
    fn write_chunk(&mut self, data: &[u8]) -> VfsResult<()> {
        rt::block_on(async { Ok(self.writer.write_chunk(data).await) })?
            .map_err(|e| map_error(e, &self.shown))
    }

    fn finish(self: Box<Self>) -> VfsResult<()> {
        let Self { writer, shown } = *self;
        rt::block_on(async move { Ok(writer.finish().await) })?
            .map(|_| ())
            .map_err(|e| map_error(e, &shown))
    }

    fn abort(self: Box<Self>) {
        let _ = rt::block_on(async move { Ok(self.writer.abort().await) });
    }
}

/// The share a path is on and the path inside it, `/`-separated without a
/// leading slash (`""` for the share's root); `None` on the server's root.
fn split_share(path: &VfsPath) -> Option<(String, String)> {
    let text = path.path.to_string_lossy().replace('\\', "/");
    let mut parts = text.split('/').filter(|part| !part.is_empty());
    let share = parts.next()?.to_string();
    let inner = parts.collect::<Vec<_>>().join("/");
    Some((share, inner))
}

/// The domain and the user of a username written `DOMAIN\user` or
/// `DOMAIN;user` (the `smb://DOMAIN;user@host` form).
fn split_domain(username: &str) -> (&str, &str) {
    username.split_once(['\\', ';']).unwrap_or(("", username))
}

/// `smb2`'s error as the VFS's, `path` naming what it was about.
fn map_error(e: smb2::Error, path: &Path) -> VfsError {
    let path = path.to_path_buf();
    // Its message only counts the addresses; say what each one did.
    if let smb2::Error::ConnectFailed { attempts, .. } = &e {
        if !attempts.is_empty() {
            let tried: Vec<String> = attempts.iter().map(ToString::to_string).collect();
            return VfsError::ConnectionFailed(tried.join("; "));
        }
    }
    match e.kind() {
        // A wrong password is `STATUS_LOGON_FAILURE` (`AuthRequired`), or a
        // guest session the server fell back to for a named account.
        ErrorKind::AuthRequired | ErrorKind::SigningRequired | ErrorKind::GuestSessionRefused => {
            VfsError::AuthenticationFailed(e.to_string())
        }
        ErrorKind::AccessDenied => VfsError::PermissionDenied { path },
        ErrorKind::NotFound => VfsError::NotFound { path },
        ErrorKind::AlreadyExists => VfsError::AlreadyExists { path },
        ErrorKind::ConnectionLost | ErrorKind::SessionExpired => {
            VfsError::ConnectionFailed(e.to_string())
        }
        ErrorKind::TimedOut => VfsError::Timeout(e.to_string()),
        ErrorKind::Cancelled => VfsError::Cancelled,
        _ => VfsError::Smb(e.to_string()),
    }
}

fn metadata_of(
    is_directory: bool,
    is_link: bool,
    size: u64,
    modified: smb2::pack::FileTime,
    created: smb2::pack::FileTime,
    readonly: bool,
) -> VfsMetadata {
    VfsMetadata {
        file_type: if is_link {
            VfsFileType::Symlink
        } else if is_directory {
            VfsFileType::Directory
        } else {
            VfsFileType::File
        },
        size: if is_directory { 0 } else { size },
        modified: modified.to_system_time(),
        created: created.to_system_time(),
        accessed: None,
        readonly,
        permissions: None,
    }
}

fn file_metadata(info: &smb2::FileInfo) -> VfsMetadata {
    let mut meta = metadata_of(
        info.is_directory,
        info.is_link(),
        info.size,
        info.modified,
        info.created,
        info.attributes.is_readonly(),
    );
    meta.accessed = info.accessed.to_system_time();
    meta
}

fn entry_metadata(entry: &smb2::DirectoryEntry) -> VfsMetadata {
    metadata_of(
        entry.is_directory,
        entry.is_link(),
        entry.size,
        entry.modified,
        entry.created,
        entry.attributes.is_readonly(),
    )
}

/// `parent/name` inside a share.
fn child(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Delete `path` and, for a directory, everything in it. A link is deleted
/// as a link: what it points to is not touched.
fn delete_tree<'a>(
    client: &'a mut SmbClient,
    tree: &'a mut Tree,
    path: String,
    depth: usize,
) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        if depth > MAX_RECURSION_DEPTH {
            return Err(VfsError::RemoteError {
                message: format!("{path}: nested too deep to delete"),
            });
        }
        let info = client
            .stat(tree, &path)
            .await
            .map_err(|e| map_error(e, Path::new(&path)))?;
        if info.is_link() {
            return client
                .delete_link(tree, &path)
                .await
                .map_err(|e| map_error(e, Path::new(&path)));
        }
        if !info.is_directory {
            return client
                .delete_file(tree, &path)
                .await
                .map_err(|e| map_error(e, Path::new(&path)));
        }
        let entries = client
            .list_directory(tree, &path)
            .await
            .map_err(|e| map_error(e, Path::new(&path)))?;
        for entry in entries {
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            delete_tree(client, tree, child(&path, &entry.name), depth + 1).await?;
        }
        client
            .delete_directory(tree, &path)
            .await
            .map_err(|e| map_error(e, Path::new(&path)))
    })
}

impl VfsProvider for SmbProvider {
    fn name(&self) -> &'static str {
        "smb"
    }

    fn connection_state(&self) -> ConnectionState {
        self.state
    }

    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()> {
        let username = self.username.clone().unwrap_or_default();
        let (domain, user) = split_domain(&username);
        let password = match &options.auth {
            AuthMethod::Password(password) => password.clone(),
            _ => String::new(),
        };
        let timeout = options
            .timeout_secs
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
        let config = smb2::ClientConfig {
            addr: format!("{}:{}", self.host, self.port),
            timeout,
            username: user.to_string(),
            password,
            domain: domain.to_string(),
            auto_reconnect: true,
            ..Default::default()
        };
        log::info!("SMB: connecting to {}:{}", self.host, self.port);
        // Called on the connection's own thread (`VfsManager::spawn_connect`).
        let result = rt::block_on(async move {
            SmbClient::connect(config)
                .await
                .map_err(|e| match map_error(e, Path::new("/")) {
                    // A guest refused, or a wrong password: ask for one.
                    VfsError::PermissionDenied { .. } => {
                        VfsError::AuthenticationFailed("access denied".to_string())
                    }
                    other => other,
                })
        });
        match result {
            Ok(client) => {
                self.session = Some(Arc::new(Mutex::new(Session {
                    client,
                    trees: HashMap::new(),
                })));
                self.state = ConnectionState::Connected;
                VfsOperation::ready(Ok(()))
            }
            Err(e) => {
                self.state = ConnectionState::Failed;
                VfsOperation::ready(Err(e))
            }
        }
    }

    fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = rt::spawn(async move {
                let mut session = session.lock().await;
                let Session { client, trees } = &mut *session;
                for tree in trees.values() {
                    let _ = client.disconnect_share(tree).await;
                }
                Ok(())
            });
        }
        self.state = ConnectionState::Disconnected;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        let parent = path.clone();
        if split_share(path).is_none() {
            let session = match self.session() {
                Ok(s) => s,
                Err(e) => return VfsOperation::error(e),
            };
            return rt::spawn(async move {
                let mut session = session.lock().await;
                let shares = session
                    .client
                    .list_shares()
                    .await
                    .map_err(|e| map_error(e, Path::new("/")))?;
                Ok(shares
                    .into_iter()
                    .filter(|share| {
                        share.share_type & STYPE_TYPE_MASK == STYPE_DISKTREE
                            && !share.name.ends_with('$')
                    })
                    .map(|share| {
                        let path = parent.join(&share.name);
                        VfsEntry::new(share.name, path, VfsMetadata::directory())
                    })
                    .collect())
            });
        }
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                let entries = client
                    .list_directory(tree, &inner)
                    .await
                    .map_err(|e| map_error(e, &parent.path))?;
                Ok(entries
                    .iter()
                    .filter(|entry| entry.name != "." && entry.name != "..")
                    .map(|entry| {
                        VfsEntry::new(
                            entry.name.clone(),
                            parent.join(&entry.name),
                            entry_metadata(entry),
                        )
                    })
                    .collect())
            })
        })
    }

    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .create_directory(tree, &inner)
                    .await
                    .map_err(|e| map_error(e, &shown))
            })
        })
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()> {
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                let mut done = String::new();
                for part in inner.split('/').filter(|part| !part.is_empty()) {
                    done = child(&done, part);
                    match client.create_directory(tree, &done).await {
                        Ok(()) => {}
                        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(map_error(e, &shown)),
                    }
                }
                Ok(())
            })
        })
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        if split_share(path).is_none() {
            return VfsOperation::ready(Ok(true));
        }
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                match client.stat(tree, &inner).await {
                    Ok(_) => Ok(true),
                    Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
                    Err(e) => Err(map_error(e, &shown)),
                }
            })
        })
    }

    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        if split_share(path).is_none() {
            return VfsOperation::ready(Ok(VfsMetadata::directory()));
        }
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                if inner.is_empty() {
                    return Ok(VfsMetadata::directory());
                }
                client
                    .stat(tree, &inner)
                    .await
                    .map(|info| file_metadata(&info))
                    .map_err(|e| map_error(e, &shown))
            })
        })
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .read_file_pipelined(tree, &inner)
                    .await
                    .map_err(|e| map_error(e, &shown))
            })
        })
    }

    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        let shown = path.path.clone();
        let data = data.to_vec();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .write_file_pipelined(tree, &inner, &data)
                    .await
                    .map(|_| ())
                    .map_err(|e| map_error(e, &shown))
            })
        })
    }

    /// A CREATE with the `FileCreate` disposition: the server refuses a
    /// taken name itself, so no other client can slip in between.
    fn create_file(&self, path: &VfsPath) -> VfsOperation<()> {
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .write_file_compound_exclusive(tree, &inner, &[])
                    .await
                    .map(|_| ())
                    .map_err(|e| map_error(e, &shown))
            })
        })
    }

    fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        let shown = path.path.clone();
        self.on_share(path, move |client, tree, inner| {
            Box::pin(async move {
                match client.delete_file(tree, &inner).await {
                    Err(e) if e.kind() == ErrorKind::IsADirectory => client
                        .delete_directory(tree, &inner)
                        .await
                        .map_err(|e| map_error(e, &shown)),
                    other => other.map_err(|e| map_error(e, &shown)),
                }
            })
        })
    }

    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()> {
        self.on_share(path, move |client, tree, inner| {
            delete_tree(client, tree, inner, 0)
        })
    }

    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let (Some((from_share, _)), Some((to_share, to_inner))) =
            (split_share(from), split_share(to))
        else {
            return VfsOperation::error(VfsError::NotSupported(
                "shares cannot be renamed".to_string(),
            ));
        };
        if from_share != to_share {
            return VfsOperation::error(VfsError::NotSupported(
                "a rename cannot move between shares".to_string(),
            ));
        }
        let shown = from.path.clone();
        self.on_share(from, move |client, tree, inner| {
            Box::pin(async move {
                client
                    .rename(tree, &inner, &to_inner)
                    .await
                    .map_err(|e| map_error(e, &shown))
            })
        })
    }

    fn copy(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let session = match self.session() {
            Ok(s) => s,
            Err(e) => return VfsOperation::error(e),
        };
        let (Some((from_share, from_inner)), Some((to_share, to_inner))) =
            (split_share(from), split_share(to))
        else {
            return VfsOperation::error(VfsError::NotSupported(
                "shares cannot be copied".to_string(),
            ));
        };
        let (from_shown, to_shown) = (from.path.clone(), to.path.clone());
        rt::spawn(async move {
            let mut session = session.lock().await;
            let data = {
                let (client, tree) = session.tree(&from_share).await?;
                client
                    .read_file_pipelined(tree, &from_inner)
                    .await
                    .map_err(|e| map_error(e, &from_shown))?
            };
            let (client, tree) = session.tree(&to_share).await?;
            client
                .write_file_pipelined(tree, &to_inner, &data)
                .await
                .map(|_| ())
                .map_err(|e| map_error(e, &to_shown))
        })
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        match self.remote() {
            Ok(smb) => chunked::download(smb, remote, local),
            Err(e) => VfsOperation::error(e),
        }
    }

    fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        match self.remote() {
            Ok(smb) => chunked::download_with_progress(smb, remote, local),
            Err(e) => VfsDownloadOperation::error(e),
        }
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        match self.remote() {
            Ok(smb) => chunked::upload(smb, local, remote),
            Err(e) => VfsOperation::error(e),
        }
    }

    fn upload_with_progress(&self, local: &Path, remote: &VfsPath) -> VfsUploadOperation {
        match self.remote() {
            Ok(smb) => chunked::upload_with_progress(smb, local, remote),
            Err(e) => VfsUploadOperation::error(e),
        }
    }

    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::Password(String::new()), AuthMethod::None]
    }

    fn supports_recursive(&self) -> bool {
        true
    }

    fn home_dir(&self) -> Option<VfsPath> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::VfsProtocol;

    fn smb(path: &str) -> VfsPath {
        VfsPath::remote(VfsProtocol::Smb, "nas", path)
    }

    #[test]
    fn the_first_component_is_the_share() {
        assert_eq!(split_share(&smb("/")), None);
        assert_eq!(split_share(&smb("")), None);
        assert_eq!(
            split_share(&smb("/media")),
            Some(("media".into(), String::new()))
        );
        assert_eq!(
            split_share(&smb("/media/films/a b.mkv")),
            Some(("media".into(), "films/a b.mkv".into()))
        );
        assert_eq!(
            split_share(&smb("/media//films/")),
            Some(("media".into(), "films".into()))
        );
    }

    #[test]
    fn a_domain_is_taken_from_the_username() {
        assert_eq!(split_domain("bob"), ("", "bob"));
        assert_eq!(split_domain("CORP\\bob"), ("CORP", "bob"));
        assert_eq!(split_domain("CORP;bob"), ("CORP", "bob"));
    }

    /// Against a live server, named by `TERMIDE_SMB_TEST=host:port/share`
    /// with `TERMIDE_SMB_USER` and `TERMIDE_SMB_PASS`; for instance
    /// `docker run -p 14450:445 dockurr/samba` with `127.0.0.1:14450/Shared`,
    /// `samba` and `secret`.
    #[test]
    #[ignore = "needs an SMB server"]
    fn files_on_a_live_share_are_read_and_written() {
        let target = std::env::var("TERMIDE_SMB_TEST").expect("TERMIDE_SMB_TEST");
        let (addr, share) = target.split_once('/').expect("host:port/share");
        let (host, port) = addr.split_once(':').expect("host:port");
        let user = std::env::var("TERMIDE_SMB_USER").unwrap_or_default();
        let pass = std::env::var("TERMIDE_SMB_PASS").unwrap_or_default();
        let at = |path: &str| VfsPath::remote(VfsProtocol::Smb, host, format!("/{share}{path}"));

        let mut wrong = SmbProvider::new(host, port.parse().ok(), Some(&user));
        assert!(matches!(
            wrong
                .connect(ConnectOptions::with_password("not the password"))
                .recv(),
            Err(VfsError::AuthenticationFailed(_))
        ));

        let mut smb = SmbProvider::new(host, port.parse().ok(), Some(&user));
        smb.connect(ConnectOptions::with_password(&pass))
            .recv()
            .expect("connected");
        let root = smb.list_dir(&smb_root(host)).recv().expect("shares");
        assert!(root.iter().any(|entry| entry.name == share), "{root:?}");

        let _ = smb.delete_recursive(&at("/termide-test")).recv();
        smb.create_dir_all(&at("/termide-test/a/b")).recv().unwrap();
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        smb.write_file(&at("/termide-test/a/big.bin"), &big)
            .recv()
            .unwrap();
        assert_eq!(
            smb.read_file(&at("/termide-test/a/big.bin"))
                .recv()
                .unwrap(),
            big
        );
        let meta = smb.metadata(&at("/termide-test/a/big.bin")).recv().unwrap();
        assert_eq!(meta.size, big.len() as u64);
        smb.rename(
            &at("/termide-test/a/big.bin"),
            &at("/termide-test/a/b/moved.bin"),
        )
        .recv()
        .unwrap();
        assert!(!smb.exists(&at("/termide-test/a/big.bin")).recv().unwrap());

        let dir = tempfile::tempdir().unwrap();
        let back = dir.path().join("down.bin");
        smb.download(&at("/termide-test/a/b/moved.bin"), &back)
            .recv()
            .unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), big);
        smb.upload(&back, &at("/termide-test/up.bin"))
            .recv()
            .unwrap();
        smb.copy(&at("/termide-test/up.bin"), &at("/termide-test/copy.bin"))
            .recv()
            .unwrap();
        assert_eq!(
            smb.read_file(&at("/termide-test/copy.bin")).recv().unwrap(),
            big
        );
        let mut names: Vec<String> = smb
            .list_dir(&at("/termide-test"))
            .recv()
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        names.sort();
        assert_eq!(names, ["a", "copy.bin", "up.bin"]);

        // A whole tree, down and up again, reporting each file.
        let tree = dir.path().join("tree");
        let download = smb.download_with_progress(&at("/termide-test/a"), &tree);
        assert_eq!(wait(|| download.try_recv()).unwrap(), tree);
        let last = download.drain_progress().expect("progress");
        assert_eq!((last.files_downloaded, last.total_files), (1, 1));
        assert_eq!(std::fs::read(tree.join("b/moved.bin")).unwrap(), big);
        std::fs::write(tree.join("b/small.txt"), b"small").unwrap();
        let upload = smb.upload_with_progress(&tree, &at("/termide-test/tree"));
        wait(|| upload.try_recv()).unwrap();
        let last = upload.drain_progress().expect("progress");
        assert_eq!((last.files_uploaded, last.total_files), (2, 2));
        assert_eq!(
            smb.read_file(&at("/termide-test/tree/b/small.txt"))
                .recv()
                .unwrap(),
            b"small"
        );

        smb.delete(&at("/termide-test/up.bin")).recv().unwrap();
        smb.delete_recursive(&at("/termide-test")).recv().unwrap();
        assert!(!smb.exists(&at("/termide-test")).recv().unwrap());
        smb.disconnect();
    }

    fn wait<T>(poll: impl Fn() -> Option<T>) -> T {
        loop {
            if let Some(done) = poll() {
                return done;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn smb_root(host: &str) -> VfsPath {
        VfsPath::remote(VfsProtocol::Smb, host, "/")
    }

    #[test]
    fn a_new_provider_is_not_connected() {
        let provider = SmbProvider::new("nas", None, Some("bob"));
        assert_eq!(provider.name(), "smb");
        assert_eq!(provider.port, DEFAULT_PORT);
        assert_eq!(provider.connection_state(), ConnectionState::Disconnected);
        assert!(matches!(
            provider.list_dir(&smb("/media")).recv(),
            Err(VfsError::NotConnected)
        ));
    }
}
