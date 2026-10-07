//! Cryptographic primitives of the vault.
//!
//! - The master password goes through Argon2id into a 32-byte key that seals
//!   the vault's X25519 private key with XChaCha20-Poly1305.
//! - Each secret is sealed to the vault's public key with an ephemeral
//!   X25519 exchange (the construction age and libsodium's sealed boxes
//!   use): `key = HKDF-SHA256(salt = eph_pub || vault_pub, ikm = shared)`,
//!   then XChaCha20-Poly1305 with the entry's identity as AAD. Storing a
//!   secret therefore needs only the public key — no master password.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::{Error, KdfParams};

pub(crate) const KEY_LEN: usize = 32;
pub(crate) const NONCE_LEN: usize = 24;
pub(crate) const SALT_LEN: usize = 16;

const ENTRY_INFO: &[u8] = b"termide-vault-v1 entry";

pub(crate) fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| Error::Random(e.to_string()))?;
    Ok(buf)
}

/// Derive the key-encryption key from the master password.
pub(crate) fn derive_master_key(
    master: &str,
    salt: &[u8],
    params: &KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, Error> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(KEY_LEN))
        .map_err(|e| Error::Format(format!("argon2 parameters: {e}")))?;
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, p)
        .hash_password_into(master.as_bytes(), salt, out.as_mut())
        .map_err(|e| Error::Format(format!("argon2: {e}")))?;
    Ok(out)
}

fn cipher(key: &[u8]) -> XChaCha20Poly1305 {
    // Every key passed here is exactly KEY_LEN bytes.
    XChaCha20Poly1305::new_from_slice(key).expect("32-byte key")
}

pub(crate) fn seal(key: &[u8], nonce: &[u8; NONCE_LEN], msg: &[u8], aad: &[u8]) -> Vec<u8> {
    cipher(key)
        .encrypt(&XNonce::from(*nonce), Payload { msg, aad })
        .expect("XChaCha20-Poly1305 encryption of a bounded message")
}

pub(crate) fn open(key: &[u8], nonce: &[u8], ct: &[u8], aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    cipher(key)
        .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad })
        .ok()
        .map(Zeroizing::new)
}

fn entry_key(shared: &[u8; 32], eph_pub: &[u8; 32], vault_pub: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(eph_pub);
    salt[32..].copy_from_slice(vault_pub);
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared);
    let mut okm = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(ENTRY_INFO, okm.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 length");
    okm
}

/// A secret sealed to the vault's public key.
pub(crate) struct Sealed {
    pub ephemeral: [u8; 32],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

pub(crate) fn seal_to(vault_pub: &[u8; 32], secret: &[u8], aad: &[u8]) -> Result<Sealed, Error> {
    let eph = StaticSecret::from(random::<32>()?);
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&PublicKey::from(*vault_pub));
    if !shared.was_contributory() {
        return Err(Error::Format(
            "vault public key is a low-order point".into(),
        ));
    }
    let key = entry_key(shared.as_bytes(), &eph_pub, vault_pub);
    let nonce = random::<NONCE_LEN>()?;
    Ok(Sealed {
        ephemeral: eph_pub,
        nonce,
        ciphertext: seal(key.as_ref(), &nonce, secret, aad),
    })
}

pub(crate) fn open_from(
    private: &StaticSecret,
    ephemeral: &[u8],
    nonce: &[u8],
    ct: &[u8],
    aad: &[u8],
) -> Option<Zeroizing<Vec<u8>>> {
    let eph_pub: [u8; 32] = ephemeral.try_into().ok()?;
    let shared = private.diffie_hellman(&PublicKey::from(eph_pub));
    if !shared.was_contributory() {
        return None;
    }
    let vault_pub = PublicKey::from(private).to_bytes();
    let key = entry_key(shared.as_bytes(), &eph_pub, &vault_pub);
    open(key.as_ref(), nonce, ct, aad)
}
