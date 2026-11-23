//! Filesystem layout for detached instances.
//!
//! Sockets and their sidecar `.info` files live in a per-user runtime
//! directory, never in a shared `/tmp`: a world-writable directory invites
//! symlink races on a multi-user host, and a stale socket there would let
//! another account impersonate an instance.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Directory holding one `<id>.sock` + `<id>.info` pair per detached instance.
///
/// `dirs::runtime_dir()` is `$XDG_RUNTIME_DIR` on Linux/BSD and `None` on
/// macOS, where the data directory is the closest per-user equivalent that
/// survives for the lifetime of the login session.
pub fn runtime_dir() -> Result<PathBuf> {
    let base = match dirs::runtime_dir() {
        Some(dir) => dir.join("termide"),
        None => dirs::data_dir()
            .map(|p| p.join("termide").join("run"))
            .context("Failed to determine a runtime directory for detached instances")?,
    };
    std::fs::create_dir_all(&base)
        .with_context(|| format!("Failed to create {}", base.display()))?;
    restrict_to_owner(&base)?;
    Ok(base)
}

/// Make a directory owner-only (`0700`).
///
/// Sockets inherit the directory's protection: a connect() needs search
/// permission on every component of the path, so an owner-only parent is
/// what keeps another local user from attaching to the instance.
#[cfg(unix)]
fn restrict_to_owner(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(dir, perms)
        .with_context(|| format!("Failed to restrict permissions on {}", dir.display()))
}

#[cfg(not(unix))]
fn restrict_to_owner(_dir: &Path) -> Result<()> {
    Ok(())
}

/// Path of the control socket for instance `id`.
pub fn socket_path(id: &str) -> Result<PathBuf> {
    Ok(runtime_dir()?.join(format!("{id}.sock")))
}

/// Path of the metadata sidecar for instance `id`.
pub fn info_path(id: &str) -> Result<PathBuf> {
    Ok(runtime_dir()?.join(format!("{id}.info")))
}

/// Longest instance id stem. The socket is `<runtime dir>/<id>.sock`, and
/// a unix socket path is limited to `sun_path` (104 bytes on macOS and the
/// BSDs, 108 on Linux), with the runtime directory already taking much of it.
pub const MAX_STEM_LEN: usize = 24;

/// Characters of the name kept before the hash in a shortened stem.
const SHORT_PREFIX_LEN: usize = MAX_STEM_LEN - 9;

/// Longest socket path `bind` accepts: `sun_path` less its terminating NUL.
#[cfg(unix)]
pub fn max_socket_path_len() -> usize {
    // SAFETY: sockaddr_un is plain old data; all-zero is a valid value.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    std::mem::size_of_val(&addr.sun_path) - 1
}

/// Fail with a clear message when `socket` is too long to bind, instead of
/// the bare "path must be shorter than SUN_LEN" of `bind`.
#[cfg(unix)]
pub fn check_socket_path(socket: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let len = socket.as_os_str().as_bytes().len();
    let max = max_socket_path_len();
    if len > max {
        anyhow::bail!(
            "Socket path {} is {len} bytes, longer than the {max} a unix socket allows; \
             set XDG_RUNTIME_DIR to a shorter directory",
            socket.display()
        );
    }
    Ok(())
}

/// FNV-1a: a hash stable across Rust releases, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Turn a project directory into an instance id stem.
///
/// The stem is the directory name reduced to characters that are safe both
/// as a filename and as something the user retypes into `--attach`. A name
/// longer than [`MAX_STEM_LEN`] keeps its start and gets a hash of the whole
/// name, so that two long names sharing a prefix stay apart.
pub fn id_stem_for_project(project_root: &Path) -> String {
    let raw = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "instance".to_string()
    } else if trimmed.len() > MAX_STEM_LEN {
        // `cleaned` is ASCII, so byte slicing is on character boundaries.
        let prefix = trimmed[..SHORT_PREFIX_LEN].trim_end_matches('-');
        format!("{prefix}-{:08x}", fnv1a(raw.as_bytes()))
    } else {
        trimmed
    }
}

/// First free id for `project_root`: the bare stem, then `stem-2`, `stem-3`, …
///
/// "Free" means no socket file with that name exists. A socket left behind by
/// a crashed daemon is reclaimed by [`crate::registry::prune_dead`], which the
/// caller runs first, so this only ever skips ids that are genuinely live.
pub fn allocate_id(project_root: &Path) -> Result<String> {
    let stem = id_stem_for_project(project_root);
    for suffix in 1..1000u32 {
        let candidate = if suffix == 1 {
            stem.clone()
        } else {
            format!("{stem}-{suffix}")
        };
        if !socket_path(&candidate)?.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("Too many detached instances for project '{stem}'")
}

/// Path of the `$TERM` handover file for instance `id`.
///
/// Written by the daemon on every attach and read by the hosted termide when
/// it refreshes its capabilities: the hosted process inherited its own `TERM`
/// from whichever terminal started the instance, which says nothing about the
/// terminal now looking at it.
pub fn term_path(id: &str) -> Result<PathBuf> {
    Ok(runtime_dir()?.join(format!("{id}.term")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_stem_uses_the_directory_name() {
        assert_eq!(id_stem_for_project(Path::new("/home/u/termide")), "termide");
    }

    #[test]
    fn id_stem_sanitises_and_lowercases() {
        assert_eq!(
            id_stem_for_project(Path::new("/home/u/My Project!")),
            "my-project"
        );
    }

    #[test]
    fn id_stem_falls_back_when_there_is_no_name() {
        assert_eq!(id_stem_for_project(Path::new("/")), "instance");
        assert_eq!(id_stem_for_project(Path::new("...")), "instance");
    }

    #[test]
    fn long_names_are_shortened_and_stay_distinct() {
        let long = "a-really-long-project-directory-name-that-goes-on-and-on";
        let a = id_stem_for_project(&Path::new("/p").join(format!("{long}-one")));
        let b = id_stem_for_project(&Path::new("/p").join(format!("{long}-two")));
        assert!(
            a.len() <= MAX_STEM_LEN && b.len() <= MAX_STEM_LEN,
            "{a} {b}"
        );
        assert!(a.starts_with("a-really-long"), "{a}");
        assert_ne!(a, b);
        // Stable: the same directory gives the same stem.
        assert_eq!(
            a,
            id_stem_for_project(&Path::new("/p").join(format!("{long}-one")))
        );
        // Exactly at the limit: kept as is.
        let fits = "x".repeat(MAX_STEM_LEN);
        assert_eq!(id_stem_for_project(&Path::new("/p").join(&fits)), fits);
    }

    #[cfg(unix)]
    #[test]
    fn overlong_socket_paths_are_refused_before_bind() {
        let max = max_socket_path_len();
        assert!(max >= 103, "{max}");
        let fits = PathBuf::from(format!("/{}", "s".repeat(max - 1)));
        assert!(check_socket_path(&fits).is_ok());
        let long = PathBuf::from(format!("/{}", "s".repeat(max)));
        let err = check_socket_path(&long).unwrap_err().to_string();
        assert!(err.contains("longer than"), "{err}");
    }
}
