//! Localized wording for why an operation failed.

use std::io::ErrorKind;

use termide_vfs::VfsError;

/// A VFS error in the user's language: the common causes are translated,
/// with the path they concern; anything else keeps its own wording.
pub fn vfs_error_text(error: &VfsError) -> String {
    let t = termide_i18n::t();
    let with_path = |text: &str, path: &std::path::Path| format!("{text}: {}", path.display());
    match error {
        VfsError::PasswordRequired => t.op_error_password_required().to_string(),
        VfsError::WrongPassword => t.op_error_wrong_password().to_string(),
        VfsError::NotFound { path } => with_path(t.op_error_not_found(), path),
        VfsError::PermissionDenied { path } => with_path(t.op_error_permission_denied(), path),
        VfsError::AlreadyExists { path } => with_path(t.op_error_already_exists(), path),
        VfsError::Archive(detail) => format!("{} ({detail})", t.op_error_archive()),
        VfsError::Io(io) => match io.kind() {
            ErrorKind::StorageFull => t.op_error_no_space().to_string(),
            ErrorKind::PermissionDenied => t.op_error_permission_denied().to_string(),
            ErrorKind::NotFound => t.op_error_not_found().to_string(),
            ErrorKind::AlreadyExists => t.op_error_already_exists().to_string(),
            _ => io.to_string(),
        },
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_causes_are_worded_and_keep_their_path() {
        let text = vfs_error_text(&VfsError::PermissionDenied {
            path: "/srv/a".into(),
        });
        assert_eq!(text, "permission denied: /srv/a");
        let full = VfsError::Io(std::io::Error::from(ErrorKind::StorageFull));
        assert_eq!(vfs_error_text(&full), "not enough disk space");
        assert!(vfs_error_text(&VfsError::Archive("bad header".into())).contains("bad header"));
    }
}
