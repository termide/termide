//! Master-password encrypted credential vault.
//!
//! The vault is a single TOML file (normally `~/.config/termide/vault.toml`).
//! Entry metadata — kind, origin, user, label — stays readable so termide
//! can tell whether a secret exists without asking for the master password;
//! it is bound to the encrypted secret as AEAD associated data, so editing
//! it makes the entry fail to decrypt instead of handing the secret to
//! another host. See [`crypto`] for the construction.
//!
//! Saving a secret needs only the vault's public key; reading one needs the
//! private key, which is unlocked by the master password and kept in memory
//! until [`Vault::lock`]. A forgotten master password cannot be recovered:
//! the vault can only be deleted and created anew.

mod crypto;
mod file;
pub mod origin;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use file::{EntryRecord, KdfRecord, KeyRecord, VaultRecord};

/// A secret value; wiped from memory when dropped.
pub type Secret = Zeroizing<String>;

/// Current on-disk format version.
pub const FORMAT_VERSION: u32 = 1;

/// Errors of vault operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("vault I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("vault file is malformed: {0}")]
    Format(String),
    #[error("vault format version {0} is newer than this termide supports")]
    UnsupportedVersion(u32),
    #[error("vault already exists")]
    AlreadyExists,
    #[error("wrong master password")]
    WrongPassword,
    #[error("vault is locked")]
    Locked,
    #[error("stored secret failed authentication (the entry was altered)")]
    Tampered,
    #[error("system random generator failed: {0}")]
    Random(String),
}

/// What a secret is used for. Lookups key on `(kind, origin, user)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    /// Login password of a network service: SFTP, FTP, SMB, database.
    Password,
    /// Passphrase of an SSH private key; origin is the key's path.
    SshPassphrase,
    /// Git credentials over HTTPS; origin is `https://host[:port]`.
    GitHttps,
}

impl SecretKind {
    /// Stable identifier used in the vault file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::SshPassphrase => "ssh-passphrase",
            Self::GitHttps => "git-https",
        }
    }

    /// Parse a stored identifier; unknown kinds (from newer versions) are
    /// kept in the file untouched but are not addressable.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "password" => Some(Self::Password),
            "ssh-passphrase" => Some(Self::SshPassphrase),
            "git-https" => Some(Self::GitHttps),
            _ => None,
        }
    }
}

/// Argon2id cost parameters, stored in the file so they can be raised later
/// without breaking existing vaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_cost: u32,
    /// Iterations.
    pub t_cost: u32,
    /// Parallelism.
    pub p_cost: u32,
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes: a few hundred milliseconds on a laptop.
    fn default() -> Self {
        Self {
            m_cost: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

/// Readable metadata of a stored secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub kind: String,
    pub origin: String,
    pub user: Option<String>,
    pub label: Option<String>,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds.
    pub updated: u64,
}

impl EntryInfo {
    /// The entry's kind if this termide knows it.
    pub fn secret_kind(&self) -> Option<SecretKind> {
        SecretKind::parse(&self.kind)
    }
}

/// An open vault file, optionally unlocked.
pub struct Vault {
    path: PathBuf,
    record: VaultRecord,
    private: Option<StaticSecret>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("entries", &self.record.entries.len())
            .field("unlocked", &self.private.is_some())
            .finish()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn decode(field: &str, value: &str) -> Result<Vec<u8>, Error> {
    B64.decode(value)
        .map_err(|e| Error::Format(format!("{field}: {e}")))
}

fn decode_array<const N: usize>(field: &str, value: &str) -> Result<[u8; N], Error> {
    decode(field, value)?
        .try_into()
        .map_err(|_| Error::Format(format!("{field}: expected {N} bytes")))
}

/// Associated data binding the private key to its public half.
fn key_aad(public: &[u8; 32]) -> Vec<u8> {
    let mut aad = b"termide-vault-v1 key".to_vec();
    aad.extend_from_slice(public);
    aad
}

/// Associated data binding a secret to its identity; length-prefixed so no
/// two distinct identities serialize alike.
fn entry_aad(kind: &str, origin: &str, user: Option<&str>) -> Vec<u8> {
    let mut aad = b"termide-vault-v1 entry".to_vec();
    for part in [kind, origin, user.unwrap_or("")] {
        aad.extend_from_slice(&(part.len() as u32).to_le_bytes());
        aad.extend_from_slice(part.as_bytes());
    }
    aad.push(u8::from(user.is_some()));
    aad
}

fn matches(e: &EntryRecord, kind: SecretKind, origin: &str, user: Option<&str>) -> bool {
    e.kind == kind.as_str() && e.origin == origin && e.user.as_deref() == user
}

impl Vault {
    /// Default location: `<config dir>/termide/vault.toml`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("termide").join("vault.toml"))
    }

    /// Open an existing vault, locked. `Ok(None)` when the file is absent.
    pub fn open(path: impl Into<PathBuf>) -> Result<Option<Self>, Error> {
        let path = path.into();
        match file::read(&path)? {
            Some(record) => Ok(Some(Self {
                path,
                record,
                private: None,
            })),
            None => Ok(None),
        }
    }

    /// Create a new vault protected by `master`; returned unlocked.
    pub fn create(
        path: impl Into<PathBuf>,
        master: &str,
        params: KdfParams,
    ) -> Result<Self, Error> {
        let path = path.into();
        let _guard = file::lock(&path)?;
        if path.exists() {
            return Err(Error::AlreadyExists);
        }
        let private = StaticSecret::from(crypto::random::<32>()?);
        let key = Self::wrap_private(&private, master, params)?;
        let record = VaultRecord {
            version: FORMAT_VERSION,
            kdf: key.0,
            key: key.1,
            entries: Vec::new(),
        };
        file::write(&path, &record)?;
        Ok(Self {
            path,
            record,
            private: Some(private),
        })
    }

    fn wrap_private(
        private: &StaticSecret,
        master: &str,
        params: KdfParams,
    ) -> Result<(KdfRecord, KeyRecord), Error> {
        let public = PublicKey::from(private).to_bytes();
        let salt = crypto::random::<{ crypto::SALT_LEN }>()?;
        let kek = crypto::derive_master_key(master, &salt, &params)?;
        let nonce = crypto::random::<{ crypto::NONCE_LEN }>()?;
        let sealed = crypto::seal(kek.as_ref(), &nonce, private.as_bytes(), &key_aad(&public));
        Ok((
            KdfRecord {
                algorithm: "argon2id".into(),
                m_cost: params.m_cost,
                t_cost: params.t_cost,
                p_cost: params.p_cost,
                salt: B64.encode(salt),
            },
            KeyRecord {
                public: B64.encode(public),
                nonce: B64.encode(nonce),
                private: B64.encode(sealed),
            },
        ))
    }

    /// Path of the vault file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the private key is in memory.
    pub fn is_unlocked(&self) -> bool {
        self.private.is_some()
    }

    /// Unlock with the master password.
    pub fn unlock(&mut self, master: &str) -> Result<(), Error> {
        let kdf = &self.record.kdf;
        if kdf.algorithm != "argon2id" {
            return Err(Error::Format(format!("unknown KDF {}", kdf.algorithm)));
        }
        let params = KdfParams {
            m_cost: kdf.m_cost,
            t_cost: kdf.t_cost,
            p_cost: kdf.p_cost,
        };
        let salt = decode("kdf.salt", &kdf.salt)?;
        let public = decode_array::<32>("key.public", &self.record.key.public)?;
        let nonce = decode("key.nonce", &self.record.key.nonce)?;
        let sealed = decode("key.private", &self.record.key.private)?;
        let kek = crypto::derive_master_key(master, &salt, &params)?;
        let plain = crypto::open(kek.as_ref(), &nonce, &sealed, &key_aad(&public))
            .ok_or(Error::WrongPassword)?;
        let bytes: Zeroizing<[u8; 32]> = Zeroizing::new(
            plain
                .as_slice()
                .try_into()
                .map_err(|_| Error::Format("key.private: expected 32 bytes".into()))?,
        );
        let private = StaticSecret::from(*bytes);
        if PublicKey::from(&private).to_bytes() != public {
            return Err(Error::Format(
                "private key does not match public key".into(),
            ));
        }
        self.private = Some(private);
        Ok(())
    }

    /// Drop the private key from memory.
    pub fn lock(&mut self) {
        self.private = None;
    }

    /// Re-read the file from disk (another process may have changed it).
    /// Keeps the vault unlocked unless its key pair was replaced.
    pub fn reload(&mut self) -> Result<(), Error> {
        match file::read(&self.path)? {
            Some(record) => self.adopt(record),
            None => {
                self.record.entries.clear();
                self.private = None;
            }
        }
        Ok(())
    }

    fn adopt(&mut self, record: VaultRecord) {
        if record.key.public != self.record.key.public {
            self.private = None;
        }
        self.record = record;
    }

    /// Metadata of all entries.
    pub fn entries(&self) -> Vec<EntryInfo> {
        self.record.entries.iter().map(EntryRecord::info).collect()
    }

    /// Find the entry for `(kind, origin, user)`. With `user == None` the most
    /// recently updated entry for the origin is returned, whatever its user.
    pub fn find(&self, kind: SecretKind, origin: &str, user: Option<&str>) -> Option<EntryInfo> {
        self.find_record(kind, origin, user).map(EntryRecord::info)
    }

    fn find_record(
        &self,
        kind: SecretKind,
        origin: &str,
        user: Option<&str>,
    ) -> Option<&EntryRecord> {
        match user {
            Some(_) => self
                .record
                .entries
                .iter()
                .find(|e| matches(e, kind, origin, user)),
            None => self
                .record
                .entries
                .iter()
                .filter(|e| e.kind == kind.as_str() && e.origin == origin)
                .max_by_key(|e| e.updated),
        }
    }

    /// Decrypt the secret of the entry found as in [`Vault::find`].
    /// `Ok(None)` when no entry exists; [`Error::Locked`] when one exists but
    /// the vault is locked.
    pub fn get(
        &self,
        kind: SecretKind,
        origin: &str,
        user: Option<&str>,
    ) -> Result<Option<(EntryInfo, Secret)>, Error> {
        let Some(e) = self.find_record(kind, origin, user) else {
            return Ok(None);
        };
        let private = self.private.as_ref().ok_or(Error::Locked)?;
        let eph = decode("entry.ephemeral", &e.ephemeral)?;
        let nonce = decode("entry.nonce", &e.nonce)?;
        let ct = decode("entry.secret", &e.secret)?;
        let aad = entry_aad(&e.kind, &e.origin, e.user.as_deref());
        let plain = crypto::open_from(private, &eph, &nonce, &ct, &aad).ok_or(Error::Tampered)?;
        let text = String::from_utf8(plain.to_vec()).map_err(|_| Error::Tampered)?;
        Ok(Some((e.info(), Zeroizing::new(text))))
    }

    /// Store or replace a secret. Works while locked: only the public key
    /// is needed.
    pub fn put(
        &mut self,
        kind: SecretKind,
        origin: &str,
        user: Option<&str>,
        secret: &str,
        label: Option<&str>,
    ) -> Result<(), Error> {
        self.mutate(|record| {
            let public = decode_array::<32>("key.public", &record.key.public)?;
            let aad = entry_aad(kind.as_str(), origin, user);
            let sealed = crypto::seal_to(&public, secret.as_bytes(), &aad)?;
            let ts = now();
            let created = record
                .entries
                .iter()
                .find(|e| matches(e, kind, origin, user))
                .map_or(ts, |e| e.created);
            record.entries.retain(|e| !matches(e, kind, origin, user));
            record.entries.push(EntryRecord {
                kind: kind.as_str().into(),
                origin: origin.into(),
                user: user.map(str::to_owned),
                label: label.map(str::to_owned),
                created,
                updated: ts,
                ephemeral: B64.encode(sealed.ephemeral),
                nonce: B64.encode(sealed.nonce),
                secret: B64.encode(&sealed.ciphertext),
            });
            Ok(())
        })
    }

    /// Delete an entry; returns whether one existed.
    pub fn remove(
        &mut self,
        kind: SecretKind,
        origin: &str,
        user: Option<&str>,
    ) -> Result<bool, Error> {
        self.mutate(|record| {
            let before = record.entries.len();
            record.entries.retain(|e| !matches(e, kind, origin, user));
            Ok(record.entries.len() != before)
        })
    }

    /// Re-encrypt the private key under a new master password. Entries are
    /// untouched: they are sealed to the unchanged key pair.
    pub fn change_master(&mut self, new_master: &str, params: KdfParams) -> Result<(), Error> {
        let private = self.private.clone().ok_or(Error::Locked)?;
        let (kdf, key) = Self::wrap_private(&private, new_master, params)?;
        let ours = self.record.key.public.clone();
        self.mutate(move |record| {
            if record.key.public != ours {
                return Err(Error::Locked);
            }
            record.kdf = kdf;
            record.key = key;
            Ok(())
        })
    }

    /// Read-modify-write under an exclusive file lock, so concurrent termide
    /// processes merge their changes instead of overwriting each other.
    fn mutate<T>(
        &mut self,
        f: impl FnOnce(&mut VaultRecord) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let _guard = file::lock(&self.path)?;
        let mut record = file::read(&self.path)?.unwrap_or_else(|| self.record.clone());
        let out = f(&mut record)?;
        file::write(&self.path, &record)?;
        self.adopt(record);
        Ok(out)
    }

    /// Delete the vault file. The only way out of a forgotten master
    /// password.
    pub fn destroy(path: &Path) -> Result<(), Error> {
        let _guard = file::lock(path)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests;
