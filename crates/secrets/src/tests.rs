use super::*;

/// Cheap Argon2 settings: the tests exercise the format, not the KDF cost.
const FAST: KdfParams = KdfParams {
    m_cost: 64,
    t_cost: 1,
    p_cost: 1,
};

fn new_vault(dir: &tempfile::TempDir) -> Vault {
    Vault::create(dir.path().join("vault.toml"), "master", FAST).unwrap()
}

#[test]
fn round_trip_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.put(SecretKind::Password, "sftp://h", Some("bob"), "pw1", None)
        .unwrap();

    let mut v = Vault::open(v.path()).unwrap().unwrap();
    assert!(!v.is_unlocked());
    assert!(v
        .find(SecretKind::Password, "sftp://h", Some("bob"))
        .is_some());
    assert!(matches!(
        v.get(SecretKind::Password, "sftp://h", Some("bob")),
        Err(Error::Locked)
    ));
    v.unlock("master").unwrap();
    let (info, secret) = v
        .get(SecretKind::Password, "sftp://h", Some("bob"))
        .unwrap()
        .unwrap();
    assert_eq!(secret.as_str(), "pw1");
    assert_eq!(info.user.as_deref(), Some("bob"));
}

#[test]
fn missing_entry_needs_no_unlock() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.lock();
    assert!(v
        .get(SecretKind::Password, "ftp://x", None)
        .unwrap()
        .is_none());
}

#[test]
fn wrong_master_password_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let v = new_vault(&dir);
    let mut v = Vault::open(v.path()).unwrap().unwrap();
    assert!(matches!(v.unlock("nope"), Err(Error::WrongPassword)));
    assert!(!v.is_unlocked());
}

#[test]
fn put_while_locked_and_replace() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.lock();
    v.put(SecretKind::Password, "ftp://h", Some("u"), "old", None)
        .unwrap();
    v.put(
        SecretKind::Password,
        "ftp://h",
        Some("u"),
        "new",
        Some("lbl"),
    )
    .unwrap();
    assert_eq!(v.entries().len(), 1);
    v.unlock("master").unwrap();
    let (info, s) = v
        .get(SecretKind::Password, "ftp://h", None)
        .unwrap()
        .unwrap();
    assert_eq!(s.as_str(), "new");
    assert_eq!(info.label.as_deref(), Some("lbl"));
}

#[test]
fn user_less_lookup_takes_any_user_and_kinds_are_separate() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.put(
        SecretKind::GitHttps,
        "https://git.example",
        Some("me"),
        "tok",
        None,
    )
    .unwrap();
    assert!(v
        .find(SecretKind::Password, "https://git.example", None)
        .is_none());
    let (info, s) = v
        .get(SecretKind::GitHttps, "https://git.example", None)
        .unwrap()
        .unwrap();
    assert_eq!((info.user.as_deref(), s.as_str()), (Some("me"), "tok"));
}

#[test]
fn edited_metadata_fails_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.put(SecretKind::Password, "sftp://good", Some("u"), "pw", None)
        .unwrap();
    let path = v.path().to_path_buf();
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("sftp://good", "sftp://evil")).unwrap();

    let mut v = Vault::open(&path).unwrap().unwrap();
    v.unlock("master").unwrap();
    assert!(matches!(
        v.get(SecretKind::Password, "sftp://evil", Some("u")),
        Err(Error::Tampered)
    ));
}

#[test]
fn swapped_public_key_is_detected_on_unlock() {
    let dir = tempfile::tempdir().unwrap();
    let a = new_vault(&dir);
    let other = tempfile::tempdir().unwrap();
    let b = Vault::create(other.path().join("v.toml"), "master", FAST).unwrap();
    let mut text = fs::read_to_string(a.path()).unwrap();
    text = text.replace(&a.record.key.public, &b.record.key.public);
    fs::write(a.path(), text).unwrap();
    let mut v = Vault::open(a.path()).unwrap().unwrap();
    assert!(v.unlock("master").is_err());
}

#[test]
fn change_master_keeps_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.put(SecretKind::SshPassphrase, "/k/id_ed25519", None, "pp", None)
        .unwrap();
    v.change_master("second", FAST).unwrap();

    let mut v = Vault::open(v.path()).unwrap().unwrap();
    assert!(matches!(v.unlock("master"), Err(Error::WrongPassword)));
    v.unlock("second").unwrap();
    let (_, s) = v
        .get(SecretKind::SshPassphrase, "/k/id_ed25519", None)
        .unwrap()
        .unwrap();
    assert_eq!(s.as_str(), "pp");
}

#[test]
fn change_master_requires_unlock() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.lock();
    assert!(matches!(v.change_master("x", FAST), Err(Error::Locked)));
}

#[test]
fn remove_and_create_twice() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.put(SecretKind::Password, "ftp://h", None, "p", None)
        .unwrap();
    assert!(v.remove(SecretKind::Password, "ftp://h", None).unwrap());
    assert!(!v.remove(SecretKind::Password, "ftp://h", None).unwrap());
    assert!(matches!(
        Vault::create(v.path(), "m", FAST),
        Err(Error::AlreadyExists)
    ));
}

#[test]
fn concurrent_writers_merge() {
    let dir = tempfile::tempdir().unwrap();
    let path = new_vault(&dir).path().to_path_buf();
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                // Each writer holds its own stale copy, as separate
                // processes would.
                let mut v = Vault::open(&path).unwrap().unwrap();
                for j in 0..5 {
                    let origin = format!("sftp://h{i}-{j}");
                    v.put(SecretKind::Password, &origin, None, "p", None)
                        .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let v = Vault::open(&path).unwrap().unwrap();
    assert_eq!(v.entries().len(), 40);
}

#[test]
fn reload_keeps_unlock_and_sees_other_writers() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = new_vault(&dir);
    let mut b = Vault::open(a.path()).unwrap().unwrap();
    b.put(SecretKind::Password, "ftp://x", None, "p", None)
        .unwrap();
    a.reload().unwrap();
    assert!(a.is_unlocked());
    assert_eq!(
        a.get(SecretKind::Password, "ftp://x", None)
            .unwrap()
            .unwrap()
            .1
            .as_str(),
        "p"
    );
}

#[test]
fn corrupt_and_future_files_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.toml");
    fs::write(&path, "not = [toml").unwrap();
    assert!(matches!(Vault::open(&path), Err(Error::Format(_))));
    fs::write(&path, "version = 99\n").unwrap();
    assert!(matches!(
        Vault::open(&path),
        Err(Error::UnsupportedVersion(99))
    ));
    assert!(Vault::open(dir.path().join("absent.toml"))
        .unwrap()
        .is_none());
}

#[cfg(unix)]
#[test]
fn file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let v = new_vault(&dir);
    let mode = fs::metadata(v.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn secrets_are_not_stored_in_clear() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = new_vault(&dir);
    v.put(
        SecretKind::Password,
        "ftp://h",
        None,
        "very-secret-value",
        None,
    )
    .unwrap();
    let text = fs::read_to_string(v.path()).unwrap();
    assert!(!text.contains("very-secret-value"));
    assert!(text.contains("ftp://h"));
}
