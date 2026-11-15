//! SMB/CIFS VFS provider.
//!
//! Uses the `pavao` crate for SMB connectivity (requires libsmbclient).
//! This feature is optional and requires the `smb` feature to be enabled.

use std::path::{Path, PathBuf};

use crate::error::{VfsError, VfsResult};
use crate::traits::VfsProvider;
use crate::types::{
    AuthMethod, ConnectOptions, ConnectionState, VfsEntry, VfsMetadata, VfsOperation, VfsPath,
    VfsProtocol,
};

/// Default SMB port.
const DEFAULT_PORT: u16 = 445;

/// SMB filesystem provider.
///
/// Note: This requires the `smb` feature and libsmbclient to be installed.
pub struct SmbProvider {
    /// SMB server hostname.
    host: String,
    /// SMB port.
    port: u16,
    /// Share name.
    share: Option<String>,
    /// Username for authentication.
    username: Option<String>,
    /// Workgroup/domain.
    workgroup: Option<String>,
    /// Current connection state.
    state: ConnectionState,
    #[cfg(feature = "smb")]
    /// SMB context.
    context: Option<pavao::SmbClient>,
}

impl SmbProvider {
    /// Create a new SMB provider.
    pub fn new(
        host: &str,
        port: Option<u16>,
        share: Option<&str>,
        username: Option<&str>,
        workgroup: Option<&str>,
    ) -> Self {
        Self {
            host: host.to_string(),
            port: port.unwrap_or(DEFAULT_PORT),
            share: share.map(String::from),
            username: username.map(String::from),
            workgroup: workgroup.map(String::from),
            state: ConnectionState::Disconnected,
            #[cfg(feature = "smb")]
            context: None,
        }
    }

    /// Convert a VfsPath to an SMB URL string (smb://server/share/path).
    pub fn to_smb_url(&self, path: &VfsPath) -> VfsResult<String> {
        if !matches!(path.protocol, VfsProtocol::Smb) {
            return Err(VfsError::InvalidPath(format!(
                "Expected SMB path, got: {}",
                path
            )));
        }

        let host = path.host.as_deref().unwrap_or(&self.host);

        // The share can come from the provider or from the first path component
        let (share, remaining_path) = if let Some(ref share) = self.share {
            (share.as_str(), path.path.display().to_string())
        } else {
            // Extract share from path: /share/rest/of/path
            let path_str = path.path.display().to_string();
            let trimmed = path_str.trim_start_matches('/');
            if let Some(pos) = trimmed.find('/') {
                (&trimmed[..pos], format!("/{}", &trimmed[pos + 1..]))
            } else if !trimmed.is_empty() {
                (trimmed, "/".to_string())
            } else {
                return Err(VfsError::InvalidPath(
                    "SMB path must include a share name".to_string(),
                ));
            }
        };

        Ok(format!("smb://{}/{}{}", host, share, remaining_path))
    }

    /// Extract password from ConnectOptions.
    fn extract_password(options: &ConnectOptions) -> String {
        match &options.auth {
            AuthMethod::Password(pwd) => pwd.clone(),
            _ => String::new(),
        }
    }
}

#[cfg(feature = "smb")]
impl VfsProvider for SmbProvider {
    fn name(&self) -> &'static str {
        "smb"
    }

    fn connection_state(&self) -> ConnectionState {
        self.state
    }

    fn connect(&mut self, options: ConnectOptions) -> VfsOperation<()> {
        use std::sync::mpsc;
        use std::thread;

        let host = self.host.clone();
        let username = self.username.clone().unwrap_or_default();
        let password = Self::extract_password(&options);
        let workgroup = self
            .workgroup
            .clone()
            .unwrap_or_else(|| "WORKGROUP".to_string());

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = (|| -> VfsResult<pavao::SmbClient> {
                log::info!("SMB: Connecting to {}", host);

                let client = pavao::SmbClient::new(
                    pavao::SmbCredentials::default()
                        .server(&host)
                        .username(&username)
                        .password(&password)
                        .workgroup(&workgroup),
                )
                .map_err(|e| {
                    VfsError::ConnectionFailed(format!("SMB connection failed: {:?}", e))
                })?;

                log::info!("SMB: Connected to {}", host);
                Ok(client)
            })();

            let _ = tx.send(result);
        });

        match rx.recv() {
            Ok(Ok(client)) => {
                self.context = Some(client);
                self.state = ConnectionState::Connected;
                VfsOperation::ready(Ok(()))
            }
            Ok(Err(e)) => {
                self.state = ConnectionState::Failed;
                VfsOperation::ready(Err(e))
            }
            Err(_) => {
                self.state = ConnectionState::Failed;
                VfsOperation::error(VfsError::RemoteError {
                    message: "Connection thread failed".to_string(),
                })
            }
        }
    }

    fn disconnect(&mut self) {
        self.context = None;
        self.state = ConnectionState::Disconnected;
    }

    fn list_dir(&self, path: &VfsPath) -> VfsOperation<Vec<VfsEntry>> {
        use crate::types::VfsFileType;

        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };
        let parent_path = path.clone();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<Vec<VfsEntry>> {
                let dir = context
                    .opendir(&smb_url)
                    .map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to open directory '{}': {:?}", smb_url, e),
                    })?;

                let mut entries = Vec::new();
                for entry in dir {
                    let entry = entry.map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to read directory entry: {:?}", e),
                    })?;

                    let name = entry.name().to_string();
                    if name == "." || name == ".." {
                        continue;
                    }

                    let file_type = match entry.file_type() {
                        pavao::SmbDirentType::Dir => VfsFileType::Directory,
                        pavao::SmbDirentType::File => VfsFileType::File,
                        pavao::SmbDirentType::Link => VfsFileType::Symlink,
                        _ => VfsFileType::File,
                    };

                    let metadata = VfsMetadata {
                        file_type,
                        size: 0,
                        modified: None,
                        created: None,
                        accessed: None,
                        readonly: false,
                        permissions: None,
                    };

                    entries.push(VfsEntry::new(name, parent_path.join(&name), metadata));
                }

                Ok(entries)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn create_dir(&self, path: &VfsPath) -> VfsOperation<()> {
        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = context
                .mkdir(&smb_url, 0o755)
                .map_err(|e| VfsError::RemoteError {
                    message: format!("Failed to create directory: {:?}", e),
                });

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn create_dir_all(&self, path: &VfsPath) -> VfsOperation<()> {
        // SMB doesn't have mkdir -p, so just try to create the final directory
        self.create_dir(path)
    }

    fn exists(&self, path: &VfsPath) -> VfsOperation<bool> {
        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = Ok(context.stat(&smb_url).is_ok());
            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn metadata(&self, path: &VfsPath) -> VfsOperation<VfsMetadata> {
        use crate::types::VfsFileType;

        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<VfsMetadata> {
                let stat = context
                    .stat(&smb_url)
                    .map_err(|e| VfsError::Smb(format!("stat {}: {}", smb_url, e)))?;

                let file_type = if stat.is_dir() {
                    VfsFileType::Directory
                } else if stat.is_link() {
                    VfsFileType::Symlink
                } else {
                    VfsFileType::File
                };

                Ok(VfsMetadata {
                    file_type,
                    size: stat.size() as u64,
                    modified: Some(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(stat.mtime() as u64),
                    ),
                    created: Some(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(stat.ctime() as u64),
                    ),
                    accessed: Some(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(stat.atime() as u64),
                    ),
                    readonly: false,
                    permissions: Some(stat.mode()),
                })
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn read_file(&self, path: &VfsPath) -> VfsOperation<Vec<u8>> {
        use std::io::Read;

        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<Vec<u8>> {
                let mut file = context
                    .open_with(&smb_url, pavao::SmbOpenOptions::default().read(true))
                    .map_err(|e| VfsError::Smb(format!("open {}: {}", smb_url, e)))?;

                let mut data = Vec::new();
                file.read_to_end(&mut data).map_err(VfsError::Io)?;

                Ok(data)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn write_file(&self, path: &VfsPath, data: &[u8]) -> VfsOperation<()> {
        use std::io::Write;

        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };
        let data = data.to_vec();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                let mut file = context
                    .open_with(
                        &smb_url,
                        pavao::SmbOpenOptions::default()
                            .write(true)
                            .create(true)
                            .truncate(true),
                    )
                    .map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to open file for writing: {:?}", e),
                    })?;

                file.write_all(&data).map_err(VfsError::Io)?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn delete(&self, path: &VfsPath) -> VfsOperation<()> {
        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(path) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                // Try to delete as file first
                if context.unlink(&smb_url).is_ok() {
                    return Ok(());
                }

                // Try to delete as directory
                context.rmdir(&smb_url).map_err(|e| VfsError::RemoteError {
                    message: format!("Failed to delete: {:?}", e),
                })?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn delete_recursive(&self, path: &VfsPath) -> VfsOperation<()> {
        // Would need to implement recursive deletion
        self.delete(path)
    }

    fn rename(&self, from: &VfsPath, to: &VfsPath) -> VfsOperation<()> {
        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let from_url = match self.to_smb_url(from) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };
        let to_url = match self.to_smb_url(to) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = context
                .rename(&from_url, &to_url)
                .map_err(|e| VfsError::RemoteError {
                    message: format!("Failed to rename: {:?}", e),
                });

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn copy(&self, _from: &VfsPath, _to: &VfsPath) -> VfsOperation<()> {
        // SMB doesn't support server-side copy
        VfsOperation::error(VfsError::NotSupported(
            "SMB does not support server-side copy".to_string(),
        ))
    }

    fn download(&self, remote: &VfsPath, local: &Path) -> VfsOperation<PathBuf> {
        use std::io::{Read, Write};

        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(remote) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };
        let local_path = local.to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<PathBuf> {
                let mut file = context
                    .open_with(&smb_url, pavao::SmbOpenOptions::default().read(true))
                    .map_err(|e| VfsError::Smb(format!("open {}: {}", smb_url, e)))?;

                let mut data = Vec::new();
                file.read_to_end(&mut data).map_err(VfsError::Io)?;

                let mut local_file = std::fs::File::create(&local_path).map_err(VfsError::Io)?;
                local_file.write_all(&data).map_err(VfsError::Io)?;

                Ok(local_path)
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn upload(&self, local: &Path, remote: &VfsPath) -> VfsOperation<()> {
        use std::io::Write;

        let context = match &self.context {
            Some(c) => c.clone(),
            None => return VfsOperation::error(VfsError::NotConnected),
        };

        let smb_url = match self.to_smb_url(remote) {
            Ok(u) => u,
            Err(e) => return VfsOperation::error(e),
        };
        let local_path = local.to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = (|| -> VfsResult<()> {
                let data = std::fs::read(&local_path).map_err(VfsError::Io)?;

                let mut file = context
                    .open_with(
                        &smb_url,
                        pavao::SmbOpenOptions::default()
                            .write(true)
                            .create(true)
                            .truncate(true),
                    )
                    .map_err(|e| VfsError::RemoteError {
                        message: format!("Failed to open remote file for writing: {:?}", e),
                    })?;

                file.write_all(&data).map_err(VfsError::Io)?;

                Ok(())
            })();

            let _ = tx.send(result);
        });

        VfsOperation::new(rx)
    }

    fn supported_auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::Password(String::new())]
    }

    fn supports_recursive(&self) -> bool {
        false
    }

    fn home_dir(&self) -> Option<VfsPath> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smb_provider_creation() {
        let provider = SmbProvider::new("server", None, Some("share"), Some("user"), None);
        assert_eq!(provider.name(), "smb");
        assert_eq!(provider.connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_smb_url_generation() {
        let provider = SmbProvider::new("server", None, Some("share"), None, None);
        let path = VfsPath::remote(VfsProtocol::Smb, "server", "/path/to/file");
        let result = provider.to_smb_url(&path);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("server"));
    }
}
