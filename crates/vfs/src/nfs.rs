//! NFSv3 VFS provider, on the pure-Rust `nfs3_client`.
//!
//! One provider is one server: `nfs://host/` shows the server's exports as
//! directories (an export `/srv/media` shows as `srv`, then `media` in it),
//! and the export a path is on is mounted the first time a path on it is
//! used. The calls on a server go one at a time, as with SFTP.
//!
//! Two kinds of export cannot be opened here, and say so: one that takes
//! requests only from a privileged source port (the Linux default, `secure`),
//! when termide runs without root on a system that keeps those ports for it;
//! and one that needs Kerberos (`sec=krb5`). The OS's own client mounts
//! either, and the mount point opens as a local directory.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nfs3_client::net::Connector;
use nfs3_client::nfs3_types::mount::mountstat3;
use nfs3_client::nfs3_types::nfs3::{
    createhow3, diropargs3, fattr3, filename3, ftype3, nfs_fh3, nfsstat3, nfstime3, sattr3,
    stable_how, COMMIT3args, CREATE3args, FSINFO3args, GETATTR3args, LOOKUP3args, MKDIR3args,
    Nfs3Option, Nfs3Result, READ3args, READDIRPLUS3args, REMOVE3args, RENAME3args, RMDIR3args,
    SETATTR3args, WRITE3args,
};
use nfs3_client::nfs3_types::rpc::{auth_stat, auth_unix, opaque_auth};
use nfs3_client::nfs3_types::xdr_codec::Opaque;
use nfs3_client::nfs3_types::{mount, portmap};
use nfs3_client::tokio::{TokioConnector, TokioIo};
use nfs3_client::{
    ConnectError, MountClient, MountError, Nfs3Connection, Nfs3ConnectionBuilder, PortmapperClient,
    RpcError,
};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::chunked::{self, ChunkRead, ChunkWrite, ChunkedRemote};
use crate::error::{VfsError, VfsResult};
use crate::traits::VfsProvider;
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, VfsDownloadOperation, VfsEntry, VfsFileType,
    VfsMetadata, VfsOperation, VfsPath, VfsUploadOperation,
};
use crate::{rt, MAX_RECURSION_DEPTH};

type Io = TokioIo<TcpStream>;
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = VfsResult<T>> + Send + 'a>>;

/// How long connecting, and each call after it, may take unless the connect
/// options say otherwise. A call that overruns leaves its reply on the wire,
/// so its mount is dropped and made again on next use.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Transfer size when the server does not say what it prefers. Kept within
/// one RPC record: `nfs3_client` does not take a reply split in fragments.
const DEFAULT_TRANSFER: u32 = 64 * 1024;
const MAX_TRANSFER: u32 = 1024 * 1024;

/// Directory listing reply sizes.
const READDIR_DIRCOUNT: u32 = 16 * 1024;
const READDIR_MAXCOUNT: u32 = 64 * 1024;

/// Modes given to what termide creates; the server applies its own umask
/// and squashing on top.
const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;

/// RPC auth flavors a mount may ask for that this client speaks.
const AUTH_NONE: u32 = 0;
const AUTH_SYS: u32 = 1;

const PRIVILEGED_PORT_REFUSED: &str = "the server takes NFS only from a privileged port, which \
     needs root here: add `insecure` to the export's options on the server, or mount the export \
     with the system and open the mount point";
const KERBEROS_REQUIRED: &str = "the export needs Kerberos (sec=krb5), which termide's NFS \
     client does not speak: mount the export with the system and open the mount point";

/// NFSv3 filesystem provider.
pub struct NfsProvider {
    host: String,
    port: Option<u16>,
    initial: PathBuf,
    state: ConnectionState,
    server: Option<Arc<Mutex<Server>>>,
}

impl NfsProvider {
    /// A provider for the server at `host`, not yet connected. `port` is the
    /// NFS service's when the server's portmapper should not be asked;
    /// `initial` is the path being opened, whose export `connect` mounts.
    pub fn new(host: &str, port: Option<u16>, initial: &Path) -> Self {
        Self {
            host: host.to_string(),
            port,
            initial: initial.to_path_buf(),
            state: ConnectionState::Disconnected,
            server: None,
        }
    }

    /// The server, for a transfer.
    fn remote(&self) -> VfsResult<Arc<dyn ChunkedRemote>> {
        let server = self.server.clone().ok_or(VfsError::NotConnected)?;
        Ok(Arc::new(NfsRemote(server)))
    }

    /// Run `f` on the server, one call at a time.
    fn run<T, F>(&self, f: F) -> VfsOperation<T>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&'a mut Server) -> BoxFuture<'a, T> + Send + 'static,
    {
        let Some(server) = self.server.clone() else {
            return VfsOperation::error(VfsError::NotConnected);
        };
        rt::spawn(async move {
            let mut server = server.lock().await;
            f(&mut server).await
        })
    }
}

/// A path as `/`-separated text with a leading slash and no trailing one.
fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = text
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    format!("/{}", parts.join("/"))
}

/// The path inside `export` when `path` is on it.
fn inside<'p>(export: &str, path: &'p str) -> Option<&'p str> {
    if export == "/" {
        return Some(path.trim_start_matches('/'));
    }
    let rest = path.strip_prefix(export)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix('/')
    }
}

/// The export of `exports` that `path` is on: the deepest one, since an
/// export may sit inside another.
fn export_of<'e>(exports: &'e [String], path: &str) -> Option<&'e str> {
    exports
        .iter()
        .filter(|export| inside(export, path).is_some())
        .max_by_key(|export| export.len())
        .map(String::as_str)
}

/// The names under `path` on the way to the exports below it.
fn virtual_children(exports: &[String], path: &str) -> BTreeSet<String> {
    let prefix = if path == "/" {
        "/".to_string()
    } else {
        format!("{path}/")
    };
    exports
        .iter()
        .filter_map(|export| export.strip_prefix(&prefix))
        .filter_map(|rest| rest.split('/').find(|part| !part.is_empty()))
        .map(String::from)
        .collect()
}

/// `parent/name` inside an export.
fn child(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn system_time(time: nfstime3) -> Option<SystemTime> {
    Some(SystemTime::UNIX_EPOCH + Duration::new(u64::from(time.seconds), time.nseconds))
}

fn metadata_of(attr: &fattr3) -> VfsMetadata {
    let file_type = match attr.type_ {
        ftype3::NF3REG => VfsFileType::File,
        ftype3::NF3DIR => VfsFileType::Directory,
        ftype3::NF3LNK => VfsFileType::Symlink,
        _ => VfsFileType::Other,
    };
    VfsMetadata {
        size: if file_type == VfsFileType::Directory {
            0
        } else {
            attr.size
        },
        file_type,
        modified: system_time(attr.mtime),
        created: None,
        accessed: system_time(attr.atime),
        readonly: attr.mode & 0o222 == 0,
        permissions: Some(attr.mode & 0o7777),
    }
}

fn status_error(status: nfsstat3, path: &str) -> VfsError {
    let shown = PathBuf::from(path);
    match status {
        nfsstat3::NFS3ERR_NOENT => VfsError::NotFound { path: shown },
        nfsstat3::NFS3ERR_ACCES | nfsstat3::NFS3ERR_PERM => {
            VfsError::PermissionDenied { path: shown }
        }
        nfsstat3::NFS3ERR_EXIST => VfsError::AlreadyExists { path: shown },
        nfsstat3::NFS3ERR_NOTEMPTY => VfsError::DirectoryNotEmpty { path: shown },
        nfsstat3::NFS3ERR_ROFS => VfsError::Nfs(format!("{path}: the export is read-only")),
        nfsstat3::NFS3ERR_NOSPC | nfsstat3::NFS3ERR_DQUOT => {
            VfsError::Nfs(format!("{path}: no space left on the export"))
        }
        nfsstat3::NFS3ERR_XDEV => {
            VfsError::NotSupported("a rename cannot move between filesystems".to_string())
        }
        other => VfsError::Nfs(format!("{path}: {other}")),
    }
}

fn rpc_error(e: RpcError, privileged: bool) -> VfsError {
    match e {
        RpcError::Io(e) => VfsError::Io(e),
        RpcError::Auth(auth_stat::AUTH_TOOWEAK) if !privileged => {
            VfsError::Nfs(PRIVILEGED_PORT_REFUSED.to_string())
        }
        RpcError::Auth(auth_stat::AUTH_TOOWEAK) => VfsError::Nfs(KERBEROS_REQUIRED.to_string()),
        other => VfsError::Nfs(other.to_string()),
    }
}

fn mount_error(e: ConnectError, export: &str, privileged: bool) -> VfsError {
    match e {
        ConnectError::Io(e) => VfsError::ConnectionFailed(e.to_string()),
        ConnectError::Portmap(e) => VfsError::ConnectionFailed(e.to_string()),
        ConnectError::Mount(MountError::Rpc(e)) => rpc_error(e, privileged),
        ConnectError::Mount(MountError::Denied(status)) => match status {
            mountstat3::MNT3ERR_ACCES | mountstat3::MNT3ERR_PERM if !privileged => {
                VfsError::Nfs(PRIVILEGED_PORT_REFUSED.to_string())
            }
            mountstat3::MNT3ERR_ACCES | mountstat3::MNT3ERR_PERM => VfsError::PermissionDenied {
                path: PathBuf::from(export),
            },
            mountstat3::MNT3ERR_NOENT | mountstat3::MNT3ERR_NOTDIR => VfsError::NotFound {
                path: PathBuf::from(export),
            },
            other => VfsError::Nfs(format!("{export}: {other}")),
        },
    }
}

/// The value of an NFS reply, or its status as an error about `path`.
fn reply<T, E>(result: Nfs3Result<T, E>, path: &str) -> VfsResult<T> {
    match result {
        Nfs3Result::Ok(value) => Ok(value),
        Nfs3Result::Err((status, _)) => Err(status_error(status, path)),
    }
}

/// The credential the OS client would send: the local user and groups,
/// which the server maps (or squashes) by its own rules.
fn credential() -> opaque_auth<'static> {
    #[cfg(unix)]
    let (uid, gid, gids, machine) = {
        // SAFETY: plain getters; the buffers are sized and the counts checked.
        unsafe {
            let mut groups = [0 as libc::gid_t; 16];
            let n = libc::getgroups(groups.len() as libc::c_int, groups.as_mut_ptr());
            let gids = if n > 0 {
                groups[..n as usize].to_vec()
            } else {
                Vec::new()
            };
            let mut name = [0u8; 256];
            let machine = if libc::gethostname(name.as_mut_ptr().cast(), name.len()) == 0 {
                let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                name[..len].to_vec()
            } else {
                Vec::new()
            };
            (libc::getuid(), libc::getgid(), gids, machine)
        }
    };
    // `nobody`, as a server squashes an unknown user anyway.
    #[cfg(not(unix))]
    let (uid, gid, gids, machine) = (65534, 65534, Vec::new(), Vec::new());
    opaque_auth::auth_unix(&auth_unix {
        stamp: 0,
        machinename: Opaque::owned(machine),
        uid,
        gid,
        gids,
    })
}

async fn timed<T>(
    timeout: Duration,
    privileged: bool,
    fut: impl Future<Output = Result<T, RpcError>>,
) -> VfsResult<T> {
    match tokio::time::timeout(timeout, fut).await {
        Ok(result) => result.map_err(|e| rpc_error(e, privileged)),
        Err(_) => Err(VfsError::Timeout(
            "the NFS server did not answer".to_string(),
        )),
    }
}

/// One mounted export.
struct Mount {
    conn: Nfs3Connection<Io>,
    root: nfs_fh3,
    privileged: bool,
    timeout: Duration,
    rsize: u32,
    wsize: u32,
}

/// One call on a mount, with its timeout and its errors as the VFS's.
macro_rules! call {
    ($mount:expr, $method:ident($args:expr), $path:expr) => {{
        let (timeout, privileged) = ($mount.timeout, $mount.privileged);
        let result = timed(timeout, privileged, $mount.conn.$method($args)).await?;
        reply(result, $path)
    }};
}

impl Mount {
    async fn getattr(&mut self, fh: &nfs_fh3, path: &str) -> VfsResult<fattr3> {
        let args = GETATTR3args { object: fh.clone() };
        Ok(call!(self, getattr(&args), path)?.obj_attributes)
    }

    async fn lookup(
        &mut self,
        dir: &nfs_fh3,
        name: &str,
        path: &str,
    ) -> VfsResult<(nfs_fh3, Option<fattr3>)> {
        let args = LOOKUP3args {
            what: diropargs3 {
                dir: dir.clone(),
                name: filename3::from(name.as_bytes()),
            },
        };
        let ok = call!(self, lookup(&args), path)?;
        let attr = match ok.obj_attributes {
            Nfs3Option::Some(attr) => Some(attr),
            Nfs3Option::None => None,
        };
        Ok((ok.object, attr))
    }

    /// The handle and the attributes of `inner`, looked up a component at a
    /// time from the export's root. Handles are not kept: a few LOOKUPs on a
    /// LAN cost less than telling a stale handle from a live one.
    async fn resolve(&mut self, inner: &str) -> VfsResult<(nfs_fh3, fattr3)> {
        let mut fh = self.root.clone();
        let mut attr = None;
        let mut done = String::new();
        for part in inner.split('/').filter(|part| !part.is_empty()) {
            done = child(&done, part);
            let (next, next_attr) = self.lookup(&fh, part, &done).await?;
            fh = next;
            attr = next_attr;
        }
        let attr = match attr {
            Some(attr) => attr,
            None => self.getattr(&fh, inner).await?,
        };
        Ok((fh, attr))
    }

    /// The directory handle `inner` is in, and its own name there.
    async fn resolve_parent(&mut self, inner: &str) -> VfsResult<(nfs_fh3, String)> {
        let (parent, name) = match inner.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", inner),
        };
        if name.is_empty() {
            return Err(VfsError::NotSupported(
                "the root of an export cannot be changed".to_string(),
            ));
        }
        let (fh, _) = self.resolve(parent).await?;
        Ok((fh, name.to_string()))
    }

    async fn list(&mut self, inner: &str) -> VfsResult<Vec<(String, fattr3)>> {
        let (dir, _) = self.resolve(inner).await?;
        let mut entries = Vec::new();
        let mut cookie = 0;
        let mut cookieverf = Default::default();
        loop {
            let args = READDIRPLUS3args {
                dir: dir.clone(),
                cookie,
                cookieverf,
                dircount: READDIR_DIRCOUNT,
                maxcount: READDIR_MAXCOUNT,
            };
            let ok = call!(self, readdirplus(&args), inner)?;
            cookieverf = ok.cookieverf;
            let eof = ok.reply.eof;
            for entry in ok.reply.entries.into_inner() {
                cookie = entry.cookie;
                let name = String::from_utf8_lossy(entry.name.0.as_ref()).into_owned();
                if name == "." || name == ".." {
                    continue;
                }
                // No POSIX file system holds such a name; a server that
                // lists one is not trusted with where the entry would land.
                // (A download checks the stricter `is_plain_entry_name`.)
                if name.is_empty() || name.contains(['/', '\0']) {
                    log::warn!("NFS: {inner}: skipped an entry named {name:?}");
                    continue;
                }
                let attr = match entry.name_attributes {
                    Nfs3Option::Some(attr) => attr,
                    Nfs3Option::None => match entry.name_handle {
                        Nfs3Option::Some(fh) => self.getattr(&fh, &child(inner, &name)).await?,
                        Nfs3Option::None => {
                            let path = child(inner, &name);
                            let (fh, attr) = self.lookup(&dir, &name, &path).await?;
                            match attr {
                                Some(attr) => attr,
                                None => self.getattr(&fh, &path).await?,
                            }
                        }
                    },
                };
                entries.push((name, attr));
            }
            if eof {
                return Ok(entries);
            }
        }
    }

    /// The handle of the file `inner`, for reading.
    async fn open_file(&mut self, inner: &str) -> VfsResult<nfs_fh3> {
        let (fh, attr) = self.resolve(inner).await?;
        if attr.type_ == ftype3::NF3DIR {
            return Err(VfsError::Nfs(format!("{inner}: is a directory")));
        }
        Ok(fh)
    }

    /// The piece of `fh` at `offset`, and whether it is the last.
    async fn read_chunk(
        &mut self,
        fh: &nfs_fh3,
        offset: u64,
        path: &str,
    ) -> VfsResult<(Vec<u8>, bool)> {
        let args = READ3args {
            file: fh.clone(),
            offset,
            count: self.rsize,
        };
        let ok = call!(self, read(&args), path)?;
        let data = ok.data.as_ref().to_vec();
        let last = ok.eof || data.is_empty();
        Ok((data, last))
    }

    /// Create `inner`, or empty it when it is there; its handle.
    async fn create_empty(&mut self, inner: &str) -> VfsResult<nfs_fh3> {
        let (dir, name) = self.resolve_parent(inner).await?;
        let args = CREATE3args {
            where_: diropargs3 {
                dir,
                name: filename3::from(name.as_bytes()),
            },
            how: createhow3::UNCHECKED(sattr3 {
                mode: Nfs3Option::Some(FILE_MODE),
                size: Nfs3Option::Some(0),
                ..Default::default()
            }),
        };
        match call!(self, create(&args), inner) {
            Ok(created) => match created.obj {
                Nfs3Option::Some(fh) => Ok(fh),
                Nfs3Option::None => Ok(self.resolve(inner).await?.0),
            },
            // UNCHECKED should empty a file that is there; some servers
            // refuse instead, so empty it apart.
            Err(VfsError::AlreadyExists { .. }) => {
                let (fh, _) = self.resolve(inner).await?;
                let args = SETATTR3args {
                    object: fh.clone(),
                    new_attributes: sattr3 {
                        size: Nfs3Option::Some(0),
                        ..Default::default()
                    },
                    guard: Nfs3Option::None,
                };
                call!(self, setattr(&args), inner)?;
                Ok(fh)
            }
            Err(e) => Err(e),
        }
    }

    /// Write `data` into `fh` at `offset`, unstably, in as many WRITEs as
    /// the server takes; the write verifier of the last.
    async fn write_chunk(
        &mut self,
        fh: &nfs_fh3,
        mut offset: u64,
        mut data: &[u8],
        path: &str,
    ) -> VfsResult<Option<[u8; 8]>> {
        let mut verf = None;
        while !data.is_empty() {
            let take = data.len().min(self.wsize as usize);
            let args = WRITE3args {
                file: fh.clone(),
                offset,
                count: take as u32,
                stable: stable_how::UNSTABLE,
                data: Opaque::borrowed(&data[..take]),
            };
            let ok = call!(self, write(&args), path)?;
            if ok.count == 0 {
                return Err(VfsError::Nfs(format!("{path}: the server wrote nothing")));
            }
            verf = Some(ok.verf.0);
            offset += u64::from(ok.count);
            data = &data[(ok.count as usize).min(data.len())..];
        }
        Ok(verf)
    }

    /// Make the unstable writes to `fh` stay. `verf` is the verifier they
    /// were written under: a different one now means the server restarted
    /// and lost them.
    async fn commit(&mut self, fh: &nfs_fh3, verf: Option<[u8; 8]>, path: &str) -> VfsResult<()> {
        let args = COMMIT3args {
            file: fh.clone(),
            offset: 0,
            count: 0,
        };
        let committed = call!(self, commit(&args), path)?;
        if verf.is_some_and(|verf| verf != committed.verf.0) {
            return Err(VfsError::Nfs(format!(
                "{path}: the server restarted during the write; write it again"
            )));
        }
        Ok(())
    }

    /// The whole of the file `inner`.
    async fn read_all(&mut self, inner: &str) -> VfsResult<Vec<u8>> {
        let fh = self.open_file(inner).await?;
        let mut data = Vec::new();
        loop {
            let (piece, last) = self.read_chunk(&fh, data.len() as u64, inner).await?;
            data.extend_from_slice(&piece);
            if last {
                return Ok(data);
            }
        }
    }

    /// Make the file `inner` hold `data`.
    async fn write_all(&mut self, inner: &str, data: &[u8]) -> VfsResult<()> {
        let fh = self.create_empty(inner).await?;
        let mut verf = None;
        for (n, piece) in data.chunks(self.wsize as usize).enumerate() {
            let offset = (n * self.wsize as usize) as u64;
            verf = self.write_chunk(&fh, offset, piece, inner).await?.or(verf);
        }
        self.commit(&fh, verf, inner).await
    }

    async fn mkdir(&mut self, inner: &str) -> VfsResult<()> {
        let (dir, name) = self.resolve_parent(inner).await?;
        let args = MKDIR3args {
            where_: diropargs3 {
                dir,
                name: filename3::from(name.as_bytes()),
            },
            attributes: sattr3 {
                mode: Nfs3Option::Some(DIR_MODE),
                ..Default::default()
            },
        };
        call!(self, mkdir(&args), inner).map(|_| ())
    }

    /// Create `inner` and the directories above it that are missing. Each
    /// component is looked up before it is made: not every server answers
    /// a MKDIR on an existing name with EXIST.
    async fn mkdir_all(&mut self, inner: &str) -> VfsResult<()> {
        let mut fh = self.root.clone();
        let mut done = String::new();
        for part in inner.split('/').filter(|part| !part.is_empty()) {
            done = child(&done, part);
            let found = match self.lookup(&fh, part, &done).await {
                Ok(found) => found,
                Err(VfsError::NotFound { .. }) => {
                    match self.mkdir(&done).await {
                        Ok(()) | Err(VfsError::AlreadyExists { .. }) => {}
                        Err(e) => return Err(e),
                    }
                    self.lookup(&fh, part, &done).await?
                }
                Err(e) => return Err(e),
            };
            if found.1.is_some_and(|attr| attr.type_ != ftype3::NF3DIR) {
                return Err(VfsError::AlreadyExists {
                    path: PathBuf::from(&done),
                });
            }
            fh = found.0;
        }
        Ok(())
    }

    /// Delete the file, link or empty directory `inner`.
    async fn remove(&mut self, inner: &str, is_dir: bool) -> VfsResult<()> {
        let (dir, name) = self.resolve_parent(inner).await?;
        let object = diropargs3 {
            dir,
            name: filename3::from(name.as_bytes()),
        };
        if is_dir {
            call!(self, rmdir(&RMDIR3args { object }), inner).map(|_| ())
        } else {
            call!(self, remove(&REMOVE3args { object }), inner).map(|_| ())
        }
    }

    fn remove_tree<'a>(&'a mut self, inner: String, depth: usize) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if depth > MAX_RECURSION_DEPTH {
                return Err(VfsError::Nfs(format!("{inner}: nested too deep to delete")));
            }
            let (_, attr) = self.resolve(&inner).await?;
            let is_dir = attr.type_ == ftype3::NF3DIR;
            if is_dir {
                for (name, _) in self.list(&inner).await? {
                    self.remove_tree(child(&inner, &name), depth + 1).await?;
                }
            }
            self.remove(&inner, is_dir).await
        })
    }

    async fn rename(&mut self, from: &str, to: &str) -> VfsResult<()> {
        let (from_dir, from_name) = self.resolve_parent(from).await?;
        let (to_dir, to_name) = self.resolve_parent(to).await?;
        let args = RENAME3args {
            from: diropargs3 {
                dir: from_dir,
                name: filename3::from(from_name.as_bytes()),
            },
            to: diropargs3 {
                dir: to_dir,
                name: filename3::from(to_name.as_bytes()),
            },
        };
        call!(self, rename(&args), from).map(|_| ())
    }
}

/// Where a path on the server is.
enum Place {
    /// On the export, at the path inside it.
    Export(String, String),
    /// Above the exports: the names on the way to them.
    Above(BTreeSet<String>),
}

/// The server and what is mounted from it.
struct Server {
    host: String,
    ip: IpAddr,
    mount_port: u16,
    /// The NFS service's port when the URL gives one; else the portmapper's.
    nfs_port: Option<u16>,
    credential: opaque_auth<'static>,
    timeout: Duration,
    /// Whether to connect from a privileged port; turned off for good once
    /// the system refuses one to an unprivileged process.
    privileged: bool,
    /// The exports, `None` when the server does not list them; an export
    /// found by mounting it is added.
    exports: Option<Vec<String>>,
    mounts: HashMap<String, Mount>,
}

impl Server {
    async fn connect(host: &str, nfs_port: Option<u16>, timeout: Duration) -> VfsResult<Self> {
        let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host, portmap::PMAP_PORT))
            .await
            .map_err(|e| VfsError::ConnectionFailed(format!("{host}: {e}")))?
            .collect();
        // The privileged-port connector binds IPv4 only.
        let addr = resolved
            .iter()
            .find(|addr| addr.is_ipv4())
            .or(resolved.first())
            .ok_or_else(|| VfsError::ConnectionFailed(format!("{host}: no address")))?;
        let ip = addr.ip();
        let mount_port = match (
            Self::getport(ip, mount::PROGRAM, mount::VERSION).await,
            nfs_port,
        ) {
            (Ok(port), _) => port,
            // No portmapper, or one that does not know the mount service: a
            // server reached on a port of its own may answer it there too.
            (Err(_), Some(port)) => port,
            (Err(e), None) => {
                return Err(VfsError::ConnectionFailed(format!(
                    "{host}: no NFS mount service ({e})"
                )))
            }
        };
        let mut server = Self {
            host: host.to_string(),
            ip,
            mount_port,
            nfs_port,
            credential: credential(),
            timeout,
            privileged: addr.is_ipv4(),
            exports: None,
            mounts: HashMap::new(),
        };
        server.exports = server.list_exports().await?;
        Ok(server)
    }

    /// The TCP port of `program` from the portmapper of `ip`.
    async fn getport(ip: IpAddr, program: u32, version: u32) -> Result<u16, String> {
        let io = TokioConnector
            .connect(SocketAddr::new(ip, portmap::PMAP_PORT))
            .await
            .map_err(|e| e.to_string())?;
        match PortmapperClient::new(io)
            .getport(program, version, portmap::IPPROTO_TCP)
            .await
        {
            Ok(0) => Err("not registered".to_string()),
            Ok(port) => Ok(port),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The server's exports, from its mount service; `None` when it answers
    /// but will not list them.
    async fn list_exports(&self) -> VfsResult<Option<Vec<String>>> {
        let io = TokioConnector
            .connect(SocketAddr::new(self.ip, self.mount_port))
            .await
            .map_err(|e| VfsError::ConnectionFailed(format!("{}: {e}", self.host)))?;
        let mut client =
            MountClient::new_with_auth(io, self.credential.clone(), opaque_auth::default());
        match client.export().await {
            Ok(exports) => Ok(Some(
                exports
                    .into_inner()
                    .into_iter()
                    .map(|node| {
                        normalize(Path::new(&*String::from_utf8_lossy(node.ex_dir.0.as_ref())))
                    })
                    .collect(),
            )),
            Err(e) => {
                log::info!("NFS: {} does not list its exports: {e}", self.host);
                Ok(None)
            }
        }
    }

    async fn mount(&mut self, export: &str) -> VfsResult<()> {
        if self.mounts.contains_key(export) {
            return Ok(());
        }
        let attempt = |privileged: bool| {
            let mut builder =
                Nfs3ConnectionBuilder::new(TokioConnector, self.ip.to_string(), export)
                    .credential(self.credential.clone())
                    .connect_from_privileged_port(privileged)
                    .mount_port(self.mount_port);
            if let Some(port) = self.nfs_port {
                builder = builder.nfs3_port(port);
            }
            tokio::time::timeout(self.timeout, builder.mount())
        };
        let timed_out = || VfsError::Timeout("the NFS server did not answer".to_string());
        let mut result = attempt(self.privileged).await.map_err(|_| timed_out())?;
        if self.privileged {
            if let Err(ConnectError::Io(e)) = &result {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    // The system keeps ports below 1024 for root.
                    self.privileged = false;
                    result = attempt(false).await.map_err(|_| timed_out())?;
                }
            }
        }
        let mut conn = result.map_err(|e| mount_error(e, export, self.privileged))?;
        let flavors = &conn.mount_resok.auth_flavors;
        if !flavors.is_empty() && !flavors.iter().any(|f| *f == AUTH_SYS || *f == AUTH_NONE) {
            let _ = conn.unmount().await;
            return Err(VfsError::Nfs(KERBEROS_REQUIRED.to_string()));
        }
        let root = conn.root_nfs_fh3();
        let fsinfo = timed(
            self.timeout,
            self.privileged,
            conn.fsinfo(&FSINFO3args {
                fsroot: root.clone(),
            }),
        )
        .await?;
        let (rsize, wsize) = match fsinfo {
            Nfs3Result::Ok(info) => (info.rtpref, info.wtpref),
            Nfs3Result::Err(_) => (DEFAULT_TRANSFER, DEFAULT_TRANSFER),
        };
        let size = |pref: u32| {
            if pref == 0 {
                DEFAULT_TRANSFER
            } else {
                pref.min(MAX_TRANSFER)
            }
        };
        if let Some(exports) = self.exports.as_mut() {
            if !exports.iter().any(|known| known == export) {
                exports.push(export.to_string());
            }
        }
        self.mounts.insert(
            export.to_string(),
            Mount {
                conn,
                root,
                privileged: self.privileged,
                timeout: self.timeout,
                rsize: size(rsize),
                wsize: size(wsize),
            },
        );
        log::info!("NFS: mounted an export of {}", self.host);
        Ok(())
    }

    /// Where `path` is, mounting its export.
    async fn place(&mut self, path: &Path) -> VfsResult<Place> {
        let path = normalize(path);
        if let Some(exports) = &self.exports {
            if let Some(export) = export_of(exports, &path).map(String::from) {
                let inner = inside(&export, &path).unwrap_or_default().to_string();
                self.mount(&export).await?;
                return Ok(Place::Export(export, inner));
            }
            let children = virtual_children(exports, &path);
            if children.is_empty() && path != "/" {
                return Err(VfsError::NotFound { path: path.into() });
            }
            return Ok(Place::Above(children));
        }
        // The server keeps its exports to itself: the shortest leading part
        // of the path it mounts is the export.
        if let Some(export) = self
            .mounts
            .keys()
            .find(|export| inside(export, &path).is_some())
        {
            let export = export.clone();
            let inner = inside(&export, &path).unwrap_or_default().to_string();
            return Ok(Place::Export(export, inner));
        }
        if path == "/" {
            return Ok(Place::Above(BTreeSet::new()));
        }
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let mut last = None;
        for n in 1..=parts.len() {
            let export = format!("/{}", parts[..n].join("/"));
            match self.mount(&export).await {
                Ok(()) => {
                    let inner = parts[n..].join("/");
                    return Ok(Place::Export(export, inner));
                }
                Err(e @ (VfsError::NotFound { .. } | VfsError::PermissionDenied { .. })) => {
                    last = Some(e)
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or(VfsError::NotFound { path: path.into() }))
    }

    /// The export and the path inside it, for a call that needs a file.
    async fn on_export(&mut self, path: &Path) -> VfsResult<(String, String)> {
        match self.place(path).await? {
            Place::Export(export, inner) => Ok((export, inner)),
            Place::Above(_) => Err(VfsError::NotSupported(
                "the server's exports are not files; open one".to_string(),
            )),
        }
    }

    fn mount_of(&mut self, export: &str) -> &mut Mount {
        self.mounts.get_mut(export).expect("mounted by place()")
    }

    /// Keep `result`, dropping the mount when its connection is gone so the
    /// next call makes it again.
    fn settle<T>(&mut self, export: &str, result: VfsResult<T>) -> VfsResult<T> {
        if result.as_ref().is_err_and(VfsError::is_connection_lost) {
            self.mounts.remove(export);
        }
        result
    }

    async fn list_dir(&mut self, path: &Path) -> VfsResult<Vec<(String, VfsMetadata)>> {
        match self.place(path).await? {
            Place::Above(children) => Ok(children
                .into_iter()
                .map(|name| (name, VfsMetadata::directory()))
                .collect()),
            Place::Export(export, inner) => {
                let result = self.mount_of(&export).list(&inner).await;
                let entries = self.settle(&export, result)?;
                Ok(entries
                    .into_iter()
                    .map(|(name, attr)| (name, metadata_of(&attr)))
                    .collect())
            }
        }
    }

    async fn metadata(&mut self, path: &Path) -> VfsResult<VfsMetadata> {
        match self.place(path).await? {
            Place::Above(_) => Ok(VfsMetadata::directory()),
            Place::Export(export, inner) => {
                let result = self.mount_of(&export).resolve(&inner).await;
                self.settle(&export, result)
                    .map(|(_, attr)| metadata_of(&attr))
            }
        }
    }

    async fn exists(&mut self, path: &Path) -> VfsResult<bool> {
        match self.metadata(path).await {
            Ok(_) => Ok(true),
            Err(VfsError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn read_all(&mut self, path: &Path) -> VfsResult<Vec<u8>> {
        let (export, inner) = self.on_export(path).await?;
        let result = self.mount_of(&export).read_all(&inner).await;
        self.settle(&export, result)
    }

    async fn write_all(&mut self, path: &Path, data: &[u8]) -> VfsResult<()> {
        let (export, inner) = self.on_export(path).await?;
        let result = self.mount_of(&export).write_all(&inner, data).await;
        self.settle(&export, result)
    }

    /// Run `f` on the mount `export` if it is still there; drop it when its
    /// connection is gone.
    async fn on_mount<T>(
        &mut self,
        export: &str,
        f: impl for<'m> FnOnce(&'m mut Mount) -> BoxFuture<'m, T>,
    ) -> VfsResult<T> {
        let result = match self.mounts.get_mut(export) {
            Some(mount) => f(mount).await,
            None => Err(VfsError::ConnectionFailed(
                "the connection to the NFS server was lost".to_string(),
            )),
        };
        self.settle(export, result)
    }

    async fn mkdir(&mut self, path: &Path, all: bool) -> VfsResult<()> {
        let (export, inner) = self.on_export(path).await?;
        let mount = self.mount_of(&export);
        let result = if all {
            mount.mkdir_all(&inner).await
        } else {
            mount.mkdir(&inner).await
        };
        self.settle(&export, result)
    }

    async fn delete(&mut self, path: &Path, recursive: bool) -> VfsResult<()> {
        let (export, inner) = self.on_export(path).await?;
        let mount = self.mount_of(&export);
        let result = if recursive {
            mount.remove_tree(inner, 0).await
        } else {
            match mount.resolve(&inner).await {
                Ok((_, attr)) => mount.remove(&inner, attr.type_ == ftype3::NF3DIR).await,
                Err(e) => Err(e),
            }
        };
        self.settle(&export, result)
    }

    async fn rename(&mut self, from: &Path, to: &Path) -> VfsResult<()> {
        let (export, from_inner) = self.on_export(from).await?;
        let (to_export, to_inner) = self.on_export(to).await?;
        if export != to_export {
            return Err(VfsError::NotSupported(
                "a rename cannot move between exports".to_string(),
            ));
        }
        let result = self.mount_of(&export).rename(&from_inner, &to_inner).await;
        self.settle(&export, result)
    }

    async fn unmount_all(&mut self) {
        for (_, mount) in self.mounts.drain() {
            let _ = tokio::time::timeout(Duration::from_secs(2), mount.conn.unmount()).await;
        }
    }
}

/// The server, as transfers use it: one call per piece, so other requests
/// get their turn in between.
struct NfsRemote(Arc<Mutex<Server>>);

impl NfsRemote {
    /// The export and the path inside it of `path`, and a handle there.
    fn open<F>(&self, path: &VfsPath, open: F) -> VfsResult<(String, String, nfs_fh3)>
    where
        F: for<'m> FnOnce(&'m mut Mount, &'m str) -> BoxFuture<'m, nfs_fh3> + Send + 'static,
    {
        let server = Arc::clone(&self.0);
        let path = path.path.clone();
        rt::block_on(async move {
            let mut server = server.lock().await;
            let (export, inner) = server.on_export(&path).await?;
            let at = inner.clone();
            let fh = server
                .on_mount(&export, move |mount| {
                    Box::pin(async move { open(mount, &at).await })
                })
                .await?;
            Ok((export, inner, fh))
        })
    }
}

impl ChunkedRemote for NfsRemote {
    fn metadata(&self, path: &VfsPath) -> VfsResult<VfsMetadata> {
        let (server, path) = (Arc::clone(&self.0), path.path.clone());
        rt::block_on(async move { server.lock().await.metadata(&path).await })
    }

    fn list(&self, path: &VfsPath) -> VfsResult<Vec<VfsEntry>> {
        let (server, parent) = (Arc::clone(&self.0), path.clone());
        rt::block_on(async move {
            let entries = server.lock().await.list_dir(&parent.path).await?;
            Ok(entries
                .into_iter()
                .map(|(name, meta)| {
                    let path = parent.join(&name);
                    VfsEntry::new(name, path, meta)
                })
                .collect())
        })
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsResult<()> {
        let (server, path) = (Arc::clone(&self.0), path.path.clone());
        rt::block_on(async move { server.lock().await.mkdir(&path, true).await })
    }

    fn open_read(&self, path: &VfsPath) -> VfsResult<Box<dyn ChunkRead>> {
        let (export, inner, fh) =
            self.open(path, |mount, inner| Box::pin(mount.open_file(inner)))?;
        Ok(Box::new(NfsReader {
            server: Arc::clone(&self.0),
            export,
            inner,
            fh,
            offset: 0,
            done: false,
        }))
    }

    fn open_write(&self, path: &VfsPath) -> VfsResult<Box<dyn ChunkWrite>> {
        let (export, inner, fh) =
            self.open(path, |mount, inner| Box::pin(mount.create_empty(inner)))?;
        Ok(Box::new(NfsWriter {
            server: Arc::clone(&self.0),
            export,
            inner,
            fh,
            offset: 0,
            verf: None,
        }))
    }
}

struct NfsReader {
    server: Arc<Mutex<Server>>,
    export: String,
    inner: String,
    fh: nfs_fh3,
    offset: u64,
    done: bool,
}

impl ChunkRead for NfsReader {
    fn next_chunk(&mut self) -> VfsResult<Option<Vec<u8>>> {
        if self.done {
            return Ok(None);
        }
        let (server, export) = (Arc::clone(&self.server), self.export.clone());
        let (fh, inner, offset) = (self.fh.clone(), self.inner.clone(), self.offset);
        let (piece, last) = rt::block_on(async move {
            server
                .lock()
                .await
                .on_mount(&export, move |mount| {
                    Box::pin(async move { mount.read_chunk(&fh, offset, &inner).await })
                })
                .await
        })?;
        self.offset += piece.len() as u64;
        self.done = last;
        Ok(if piece.is_empty() { None } else { Some(piece) })
    }
}

struct NfsWriter {
    server: Arc<Mutex<Server>>,
    export: String,
    inner: String,
    fh: nfs_fh3,
    offset: u64,
    verf: Option<[u8; 8]>,
}

impl ChunkWrite for NfsWriter {
    fn write_chunk(&mut self, data: &[u8]) -> VfsResult<()> {
        let (server, export) = (Arc::clone(&self.server), self.export.clone());
        let (fh, inner, offset) = (self.fh.clone(), self.inner.clone(), self.offset);
        let data = data.to_vec();
        let len = data.len() as u64;
        let verf = rt::block_on(async move {
            server
                .lock()
                .await
                .on_mount(&export, move |mount| {
                    Box::pin(async move { mount.write_chunk(&fh, offset, &data, &inner).await })
                })
                .await
        })?;
        self.offset += len;
        self.verf = verf.or(self.verf);
        Ok(())
    }

    fn finish(self: Box<Self>) -> VfsResult<()> {
        let Self {
            server,
            export,
            inner,
            fh,
            verf,
            ..
        } = *self;
        rt::block_on(async move {
            server
                .lock()
                .await
                .on_mount(&export, move |mount| {
                    Box::pin(async move { mount.commit(&fh, verf, &inner).await })
                })
                .await
        })
    }

    /// What was written stays as far as the server keeps unstable writes;
    /// nothing to close in NFS.
    fn abort(self: Box<Self>) {}
}

impl VfsProvider for NfsProvider {
    fn name(&self) -> &'static str {
        "nfs"
    }

    fn connection_state(&self) -> ConnectionState {
        self.state
    }

    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()> {
        let timeout = options
            .timeout_secs
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
        let (host, port, initial) = (self.host.clone(), self.port, self.initial.clone());
        log::info!("NFS: connecting to {host}");
        // Called on the connection's own thread (`VfsManager::spawn_connect`).
        let result = rt::block_on(async move {
            let mut server = tokio::time::timeout(timeout, Server::connect(&host, port, timeout))
                .await
                .map_err(|_| {
                    VfsError::Timeout(format!("{host}: the NFS server did not answer"))
                })??;
            // Mount the export being opened now, so a refusal fails here.
            server.place(&initial).await?;
            Ok(server)
        });
        match result {
            Ok(server) => {
                self.server = Some(Arc::new(Mutex::new(server)));
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
        if let Some(server) = self.server.take() {
            let _ = rt::spawn(async move {
                server.lock().await.unmount_all().await;
                Ok(())
            });
        }
        self.state = ConnectionState::Disconnected;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        let parent = path.clone();
        self.run(move |server| {
            Box::pin(async move {
                Ok(server
                    .list_dir(&parent.path)
                    .await?
                    .into_iter()
                    .map(|(name, meta)| {
                        let path = parent.join(&name);
                        VfsEntry::new(name, path, meta)
                    })
                    .collect())
            })
        })
    }

    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.mkdir(&path, false).await }))
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.mkdir(&path, true).await }))
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.exists(&path).await }))
    }

    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.metadata(&path).await }))
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.read_all(&path).await }))
    }

    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        let (path, data) = (path.path.clone(), data.to_vec());
        self.run(move |server| Box::pin(async move { server.write_all(&path, &data).await }))
    }

    fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.delete(&path, false).await }))
    }

    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()> {
        let path = path.path.clone();
        self.run(move |server| Box::pin(async move { server.delete(&path, true).await }))
    }

    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let (from, to) = (from.path.clone(), to.path.clone());
        self.run(move |server| Box::pin(async move { server.rename(&from, &to).await }))
    }

    fn copy(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        // NFSv3 has no server-side copy: the data comes here and goes back.
        let (from, to) = (from.path.clone(), to.path.clone());
        self.run(move |server| {
            Box::pin(async move {
                let data = server.read_all(&from).await?;
                server.write_all(&to, &data).await
            })
        })
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        match self.remote() {
            Ok(nfs) => chunked::download(nfs, remote, local),
            Err(e) => VfsOperation::error(e),
        }
    }

    fn download_with_progress(&self, remote: &VfsPath, local: &Path) -> VfsDownloadOperation {
        match self.remote() {
            Ok(nfs) => chunked::download_with_progress(nfs, remote, local),
            Err(e) => VfsDownloadOperation::error(e),
        }
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        match self.remote() {
            Ok(nfs) => chunked::upload(nfs, local, remote),
            Err(e) => VfsOperation::error(e),
        }
    }

    fn upload_with_progress(&self, local: &Path, remote: &VfsPath) -> VfsUploadOperation {
        match self.remote() {
            Ok(nfs) => chunked::upload_with_progress(nfs, local, remote),
            Err(e) => VfsUploadOperation::error(e),
        }
    }

    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::None]
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

    fn exports(list: &[&str]) -> Vec<String> {
        list.iter().map(|e| e.to_string()).collect()
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(normalize(Path::new("")), "/");
        assert_eq!(normalize(Path::new("/")), "/");
        assert_eq!(normalize(Path::new("srv//media/")), "/srv/media");
        assert_eq!(normalize(Path::new("/srv/./media")), "/srv/media");
    }

    #[test]
    fn a_path_is_on_its_deepest_export() {
        let list = exports(&["/srv", "/srv/media", "/home"]);
        assert_eq!(export_of(&list, "/srv/media/films"), Some("/srv/media"));
        assert_eq!(export_of(&list, "/srv/media"), Some("/srv/media"));
        assert_eq!(export_of(&list, "/srv/other"), Some("/srv"));
        assert_eq!(export_of(&list, "/srvx"), None);
        assert_eq!(export_of(&list, "/"), None);
        assert_eq!(inside("/srv/media", "/srv/media/films/a"), Some("films/a"));
        assert_eq!(inside("/srv/media", "/srv/media"), Some(""));
        assert_eq!(inside("/", "/a/b"), Some("a/b"));
    }

    #[test]
    fn the_way_to_the_exports_is_listed() {
        let list = exports(&["/srv/nfs/media", "/srv/nfs/backup", "/home"]);
        let names = |path| {
            virtual_children(&list, path)
                .into_iter()
                .collect::<Vec<_>>()
        };
        assert_eq!(names("/"), ["home", "srv"]);
        assert_eq!(names("/srv"), ["nfs"]);
        assert_eq!(names("/srv/nfs"), ["backup", "media"]);
        assert!(names("/opt").is_empty());
    }

    #[test]
    fn refusals_explain_what_to_do() {
        let refused = mount_error(
            ConnectError::Mount(MountError::Denied(mountstat3::MNT3ERR_ACCES)),
            "/srv",
            false,
        );
        assert!(refused.to_string().contains("insecure"), "{refused}");
        let denied = mount_error(
            ConnectError::Mount(MountError::Denied(mountstat3::MNT3ERR_ACCES)),
            "/srv",
            true,
        );
        assert!(matches!(denied, VfsError::PermissionDenied { .. }));
        let weak = rpc_error(RpcError::Auth(auth_stat::AUTH_TOOWEAK), true);
        assert!(weak.to_string().contains("Kerberos"), "{weak}");
        // Neither reads as a lost connection, which would offer a reconnect.
        assert!(!refused.is_connection_lost() && !weak.is_connection_lost());
    }

    /// An in-memory NFSv3 server exporting `/data`, on a port of its own
    /// that answers portmap, MOUNT and NFS alike.
    fn serve() -> u16 {
        use nfs3_server::memfs::{MemFs, MemFsConfig};
        use nfs3_server::tcp::{NFSTcp, NFSTcpListener};

        let mut config = MemFsConfig::default();
        config.add_dir("/films");
        config.add_file("/films/a.txt", "hello");
        let fs = MemFs::new(config).expect("memfs");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let _ = rt.block_on(async move {
                let mut listener = NFSTcpListener::bind("127.0.0.1:0", fs).await?;
                listener.with_export_name("data");
                tx.send(listener.get_listen_port()).expect("port");
                listener.handle_forever().await
            });
        });
        rx.recv().expect("server started")
    }

    fn connected(port: u16, initial: &str) -> VfsResult<NfsProvider> {
        let mut provider = NfsProvider::new("127.0.0.1", Some(port), Path::new(initial));
        provider.connect(ConnectOptions::default()).recv()?;
        Ok(provider)
    }

    fn at(path: &str) -> VfsPath {
        VfsPath::remote(crate::types::VfsProtocol::Nfs, "127.0.0.1", path)
    }

    fn names(provider: &NfsProvider, path: &str) -> Vec<String> {
        let mut names: Vec<String> = provider
            .list_dir(&at(path))
            .recv()
            .expect("listing")
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn files_on_an_export_are_read_and_written() {
        let port = serve();
        let nfs = connected(port, "/data").expect("connected");
        assert_eq!(names(&nfs, "/"), ["data"]);
        assert_eq!(names(&nfs, "/data"), ["films"]);
        assert!(
            nfs.metadata(&at("/data/films")).recv().unwrap().file_type == VfsFileType::Directory
        );
        assert_eq!(
            nfs.read_file(&at("/data/films/a.txt")).recv().unwrap(),
            b"hello"
        );

        // Larger than one transfer, so it goes in several WRITEs and READs.
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        nfs.write_file(&at("/data/films/big.bin"), &big)
            .recv()
            .unwrap();
        assert_eq!(
            nfs.read_file(&at("/data/films/big.bin")).recv().unwrap(),
            big
        );
        // Writing again truncates.
        nfs.write_file(&at("/data/films/big.bin"), b"short")
            .recv()
            .unwrap();
        assert_eq!(
            nfs.read_file(&at("/data/films/big.bin")).recv().unwrap(),
            b"short"
        );

        nfs.create_dir_all(&at("/data/x/y")).recv().unwrap();
        nfs.rename(&at("/data/films/big.bin"), &at("/data/x/y/moved.bin"))
            .recv()
            .unwrap();
        assert_eq!(names(&nfs, "/data/x/y"), ["moved.bin"]);
        assert!(!nfs.exists(&at("/data/films/big.bin")).recv().unwrap());

        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("up.bin");
        std::fs::write(&local, &big).unwrap();
        nfs.upload(&local, &at("/data/x/up.bin")).recv().unwrap();
        let back = dir.path().join("down.bin");
        nfs.download(&at("/data/x/up.bin"), &back).recv().unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), big);

        // Made again by the upload: there is still one of it.
        assert_eq!(names(&nfs, "/data"), ["films", "x"]);
        nfs.delete_recursive(&at("/data/x")).recv().unwrap();
        assert!(!nfs.exists(&at("/data/x")).recv().unwrap());
        assert_eq!(names(&nfs, "/data"), ["films"]);
    }

    #[test]
    fn a_tree_goes_up_and_down_with_progress() {
        let port = serve();
        let nfs = connected(port, "/data").expect("connected");
        let dir = tempfile::tempdir().unwrap();
        let up = dir.path().join("up");
        std::fs::create_dir_all(up.join("sub/deeper")).unwrap();
        let big: Vec<u8> = (0..2_500_000u32).map(|i| (i % 249) as u8).collect();
        std::fs::write(up.join("big.bin"), &big).unwrap();
        std::fs::write(up.join("sub/a.txt"), b"a").unwrap();
        std::fs::write(up.join("sub/deeper/b.txt"), b"bb").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("..", up.join("sub/loop")).unwrap();

        let upload = nfs.upload_with_progress(&up, &at("/data/tree"));
        let result = loop {
            if let Some(result) = upload.try_recv() {
                break result;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        result.expect("uploaded");
        let last = upload.drain_progress().expect("progress");
        assert_eq!((last.files_uploaded, last.total_files), (3, 3));
        assert_eq!(last.bytes_uploaded, big.len() as u64 + 3);
        assert_eq!(names(&nfs, "/data/tree/sub"), ["a.txt", "deeper"]);

        let down = dir.path().join("down");
        let download = nfs.download_with_progress(&at("/data/tree"), &down);
        let result = loop {
            if let Some(result) = download.try_recv() {
                break result;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(result.expect("downloaded"), down);
        let last = download.drain_progress().expect("progress");
        assert_eq!((last.files_downloaded, last.total_files), (3, 3));
        assert_eq!(std::fs::read(down.join("big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(down.join("sub/deeper/b.txt")).unwrap(), b"bb");

        // Paused at once, then cancelled: the file never gets all through.
        let upload = nfs.upload_with_progress(&up.join("big.bin"), &at("/data/cut.bin"));
        upload.set_paused(true);
        std::thread::sleep(Duration::from_millis(100));
        upload.cancel();
        let result = loop {
            if let Some(result) = upload.try_recv() {
                break result;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(matches!(result, Err(VfsError::Cancelled)), "{result:?}");
        match nfs.metadata(&at("/data/cut.bin")).recv() {
            Ok(cut) => assert!(cut.size < big.len() as u64),
            Err(e) => assert!(matches!(e, VfsError::NotFound { .. }), "{e:?}"),
        }
    }

    #[test]
    fn a_path_off_the_exports_fails_the_connection() {
        let port = serve();
        assert!(matches!(
            connected(port, "/nope/films"),
            Err(VfsError::NotFound { .. })
        ));
        // The server's root connects and shows the exports.
        let nfs = connected(port, "/").expect("connected");
        assert_eq!(names(&nfs, "/"), ["data"]);
    }

    #[test]
    fn a_new_provider_is_not_connected() {
        let provider = NfsProvider::new("nas", None, Path::new("/srv"));
        assert_eq!(provider.name(), "nfs");
        assert_eq!(provider.connection_state(), ConnectionState::Disconnected);
        let path = VfsPath::remote(crate::types::VfsProtocol::Nfs, "nas", "/srv");
        assert!(matches!(
            provider.list_dir(&path).recv(),
            Err(VfsError::NotConnected)
        ));
    }
}
