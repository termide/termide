//! On-disk representation, atomic writes and the cross-process lock.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{EntryInfo, Error, FORMAT_VERSION};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VaultRecord {
    pub version: u32,
    pub kdf: KdfRecord,
    pub key: KeyRecord,
    #[serde(default, rename = "entry")]
    pub entries: Vec<EntryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KdfRecord {
    pub algorithm: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyRecord {
    /// X25519 public key, base64.
    pub public: String,
    pub nonce: String,
    /// Private key sealed with the master key, base64.
    pub private: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EntryRecord {
    pub kind: String,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub created: u64,
    pub updated: u64,
    pub ephemeral: String,
    pub nonce: String,
    pub secret: String,
}

impl EntryRecord {
    pub fn info(&self) -> EntryInfo {
        EntryInfo {
            kind: self.kind.clone(),
            origin: self.origin.clone(),
            user: self.user.clone(),
            label: self.label.clone(),
            created: self.created,
            updated: self.updated,
        }
    }
}

const HEADER: &str = "# termide password vault. Secrets are encrypted under the master \
password;\n# do not edit by hand: changed entries stop decrypting.\n\n";

pub(crate) fn read(path: &Path) -> Result<Option<VaultRecord>, Error> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // Peek at the version first so a newer format reports itself instead
    // of failing on an unknown field.
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }
    let v: Version = toml::from_str(&text).map_err(|e| Error::Format(e.to_string()))?;
    if v.version > FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(v.version));
    }
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| Error::Format(e.to_string()))
}

/// Write via a temporary sibling and rename, so a crash never leaves a
/// half-written vault. The file is private to the user.
pub(crate) fn write(path: &Path, record: &VaultRecord) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let body = toml::to_string(record).map_err(|e| Error::Format(e.to_string()))?;
    let tmp = sibling(path, "tmp");
    {
        let mut f = private_options().truncate(true).open(&tmp)?;
        f.write_all(HEADER.as_bytes())?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn sibling(path: &Path, ext: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(ext);
    path.with_file_name(name)
}

fn private_options() -> OpenOptions {
    let mut o = OpenOptions::new();
    o.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

/// Exclusive advisory lock held for the guard's lifetime.
pub(crate) struct LockGuard(File);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(crate) fn lock(path: &Path) -> Result<LockGuard, Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let f = private_options().open(sibling(path, "lock"))?;
    f.lock()?;
    Ok(LockGuard(f))
}
