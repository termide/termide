use anyhow::{bail, Result};
use std::fs;
use std::path::Component;

use termide_core::{util::is_binary_file, PanelEvent};
use termide_git::GitStatus;

use super::{FileEntry, FileManager};

/// How a file should be opened
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum FileOpenMode {
    /// Open with default action (Enter): auto-detect type
    Default,
    /// Force open in editor (F4): treat everything as text
    ForceEdit,
    /// View mode (F3): similar to Default but executables are treated as text
    View,
    /// Open with system default app (Shift+Enter)
    External,
}

/// Determine the appropriate PanelEvent for opening a file based on its type and open mode.
/// Returns None if the operation should not proceed (e.g., deleted files, directories).
pub(super) fn determine_file_open_event(
    entry: &FileEntry,
    file_path: &std::path::Path,
    mode: FileOpenMode,
) -> Option<PanelEvent> {
    // Prohibit operations on deleted files
    if entry.git_status == GitStatus::Deleted {
        return None;
    }

    // Directories and ".." - do nothing for file operations
    if entry.is_dir || entry.name == ".." {
        return None;
    }

    match mode {
        FileOpenMode::External => {
            // Always open with system default
            Some(PanelEvent::OpenExternal(file_path.to_path_buf()))
        }
        FileOpenMode::ForceEdit => {
            // Binary files open in the hex editor (the text editor can't load
            // them); everything else opens in the text editor.
            if is_binary_file(file_path) {
                return Some(PanelEvent::EditBinary(file_path.to_path_buf()));
            }
            Some(PanelEvent::OpenFile(file_path.to_path_buf()))
        }
        FileOpenMode::View => {
            // View mode: open in read-only editor
            // 1. Raster images → ImagePanel
            if is_raster_image(&entry.name) {
                return Some(PanelEvent::PreviewMedia(file_path.to_path_buf()));
            }

            // 2. Vector images, video → xdg-open
            if is_vector_image(&entry.name) || is_video(&entry.name) {
                return Some(PanelEvent::OpenExternal(file_path.to_path_buf()));
            }

            // 3. Markdown → rendered preview
            if is_markdown(&entry.name) {
                return Some(PanelEvent::ViewMarkdown(file_path.to_path_buf()));
            }

            // 4. Mermaid → diagram viewer
            if is_mermaid(&entry.name) {
                return Some(PanelEvent::ViewMermaid(file_path.to_path_buf()));
            }

            // 5. HTML → rendered preview
            if is_html(&entry.name) {
                return Some(PanelEvent::ViewHtml(file_path.to_path_buf()));
            }

            // 6. SQLite database → database viewer
            if is_database_file(&entry.name) {
                return Some(PanelEvent::ViewDatabase(file_path.to_path_buf()));
            }

            // 7. Binary files → hex viewer
            if is_binary_file(file_path) {
                return Some(PanelEvent::ViewBinary(file_path.to_path_buf()));
            }

            // 8. Text files → read-only editor
            Some(PanelEvent::ViewFile(file_path.to_path_buf()))
        }
        FileOpenMode::Default => {
            // Default mode: auto-detect action
            // 1. Raster images → ImagePanel
            if is_raster_image(&entry.name) {
                return Some(PanelEvent::PreviewMedia(file_path.to_path_buf()));
            }

            // 2. Vector images, video → xdg-open
            if is_vector_image(&entry.name) || is_video(&entry.name) {
                return Some(PanelEvent::OpenExternal(file_path.to_path_buf()));
            }

            // 3. Source/text files with known extensions → editor (even if executable).
            //    Markdown opens in the editor on Enter; F3 (View) shows the
            //    rendered preview instead.
            if is_source_file(&entry.name) {
                return Some(PanelEvent::OpenFile(file_path.to_path_buf()));
            }

            // 4. SQLite database → database viewer
            if is_database_file(&entry.name) {
                return Some(PanelEvent::ViewDatabase(file_path.to_path_buf()));
            }

            // 5. Executable binary → run in terminal
            if entry.is_executable {
                return Some(PanelEvent::ExecuteFile(file_path.to_path_buf()));
            }

            // 6. Binary files → hex viewer
            if is_binary_file(file_path) {
                return Some(PanelEvent::ViewBinary(file_path.to_path_buf()));
            }

            // 6. Text files → editor (editable)
            Some(PanelEvent::OpenFile(file_path.to_path_buf()))
        }
    }
}

fn get_extension(filename: &str) -> String {
    filename
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

fn is_raster_image(filename: &str) -> bool {
    matches!(
        get_extension(filename).as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif"
    )
}

fn is_vector_image(filename: &str) -> bool {
    matches!(get_extension(filename).as_str(), "svg" | "ico")
}

fn is_video(filename: &str) -> bool {
    matches!(
        get_extension(filename).as_str(),
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v"
    )
}

fn is_markdown(filename: &str) -> bool {
    matches!(get_extension(filename).as_str(), "md" | "markdown")
}

fn is_mermaid(filename: &str) -> bool {
    matches!(get_extension(filename).as_str(), "mmd" | "mermaid")
}

fn is_database_file(filename: &str) -> bool {
    matches!(
        get_extension(filename).as_str(),
        "db" | "sqlite" | "sqlite3" | "db3"
    )
}

fn is_html(filename: &str) -> bool {
    matches!(get_extension(filename).as_str(), "html" | "htm")
}

fn is_source_file(filename: &str) -> bool {
    matches!(
        get_extension(filename).as_str(),
        // Shell scripts
        "sh" | "bash" | "zsh" | "fish" | "ksh" | "csh" |
        // Scripting languages
        "py" | "rb" | "pl" | "pm" | "lua" | "tcl" | "awk" |
        // Compiled languages (source files)
        "rs" | "go" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" |
        "java" | "kt" | "scala" | "cs" | "hs" | "lhs" |
        // Web / JS / TS
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" |
        "html" | "htm" | "css" | "scss" | "sass" | "less" |
        // Config / data
        "json" | "yaml" | "yml" | "toml" | "xml" | "ini" | "cfg" |
        "conf" | "env" | "properties" |
        // Markup / docs
        "md" | "rst" | "txt" | "tex" | "adoc" |
        // Nix / PHP / other
        "nix" | "php" | "r" | "jl" | "ex" | "exs" | "erl" |
        // Build / CI
        "cmake" | "make" | "mk" | "gradle" | "sbt" |
        // SQL / DB
        "sql" |
        // Docker / misc
        "dockerfile"
    ) || filename.eq_ignore_ascii_case("Makefile")
        || filename.eq_ignore_ascii_case("Dockerfile")
        || filename.eq_ignore_ascii_case("Rakefile")
        || filename.eq_ignore_ascii_case("Gemfile")
        || filename.eq_ignore_ascii_case("Vagrantfile")
        || filename.eq_ignore_ascii_case(".gitignore")
        || filename.eq_ignore_ascii_case(".env")
}

/// Validate that a user-provided file/directory name does not escape the parent directory.
/// Rejects names containing `..`, absolute paths, and path separators.
fn validate_entry_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Name cannot be empty");
    }

    let path = std::path::Path::new(name);

    // Reject absolute paths
    if path.is_absolute() {
        bail!("Absolute paths are not allowed");
    }

    // Reject any component that is `..` or contains path separators leading outside
    for component in path.components() {
        match component {
            Component::ParentDir => bail!("Path traversal ('..') is not allowed"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("Absolute paths are not allowed")
            }
            _ => {}
        }
    }

    Ok(())
}

impl FileManager {
    /// Handle Enter on the current item: open file/directory or navigate up.
    pub(crate) fn enter(&mut self) -> Option<PanelEvent> {
        // Extract all needed data upfront to avoid borrow conflicts
        let te = self.tree_entry_at(self.selected)?;
        let is_deleted = te.file_entry.git_status == GitStatus::Deleted;
        let is_parent = te.file_entry.name == "..";
        let is_dir = te.file_entry.is_dir;
        let is_symlink = te.file_entry.is_symlink;
        let entry_name = te.file_entry.name.clone();
        let full_path = te.full_path.clone();

        if is_deleted {
            return None;
        }

        if is_parent {
            if let Some(dir_name) = self.current_path.file_name() {
                self.navigation
                    .save_for_going_up(dir_name.to_string_lossy().into_owned());
            }

            if self.vfs.is_remote() {
                self.vfs.navigate_up();
                self.vfs.start_list_dir();
            } else if let Some(parent) = self.current_path.parent() {
                self.current_path = parent.to_path_buf();
                let _ = self.load_directory();
            }
            return None;
        }

        if is_dir {
            self.navigation.prepare_for_going_down();

            if self.vfs.is_remote() {
                self.vfs.navigate_down(&entry_name);
                self.vfs.start_list_dir();
            } else {
                self.current_path = full_path;
                let _ = self.load_directory();
            }
            return None;
        }

        // Remote symlink: the listing reports the link itself, not its
        // target, so `is_dir` is false even when it points to a directory.
        // Resolve the target type async (stat follows the link); tick()
        // then navigates into it (directory) or opens it (file). Without
        // this, Enter would treat it as a file and kick off a recursive
        // download of the target tree.
        if is_symlink && self.vfs.is_remote() {
            self.navigation.prepare_for_going_down();
            self.vfs.start_resolve_symlink(&entry_name);
            return None;
        }

        // File — check if remote
        if self.vfs.is_remote() {
            let vfs_path = self.vfs.current_path().join(&entry_name);
            return Some(PanelEvent::OpenRemoteFile(vfs_path.to_url_string()));
        }

        // Re-borrow entry for determine_file_open_event
        let entry = &self.tree_entry_at(self.selected)?.file_entry;
        determine_file_open_event(entry, &full_path, FileOpenMode::Default)
    }

    /// Open file for editing (F4)
    pub(crate) fn edit_file(&mut self) -> Option<PanelEvent> {
        let te = self.tree_entry_at(self.selected)?;
        if te.file_entry.git_status == GitStatus::Deleted {
            return None;
        }
        if te.file_entry.is_dir || te.file_entry.name == ".." {
            return None;
        }
        let entry_name = te.file_entry.name.clone();
        let full_path = te.full_path.clone();

        if self.vfs.is_remote() {
            let vfs_path = self.vfs.current_path().join(&entry_name);
            return Some(PanelEvent::OpenRemoteFile(vfs_path.to_url_string()));
        }

        let entry = &self.tree_entry_at(self.selected)?.file_entry;
        determine_file_open_event(entry, &full_path, FileOpenMode::ForceEdit)
    }

    /// View file without executing (F3)
    pub(crate) fn view_file(&mut self) -> Option<PanelEvent> {
        let te = self.tree_entry_at(self.selected)?;
        if te.file_entry.git_status == GitStatus::Deleted {
            return None;
        }
        if te.file_entry.is_dir || te.file_entry.name == ".." {
            return None;
        }
        let entry_name = te.file_entry.name.clone();
        let full_path = te.full_path.clone();

        if self.vfs.is_remote() {
            let vfs_path = self.vfs.current_path().join(&entry_name);
            return Some(PanelEvent::OpenRemoteFile(vfs_path.to_url_string()));
        }

        let entry = &self.tree_entry_at(self.selected)?.file_entry;
        determine_file_open_event(entry, &full_path, FileOpenMode::View)
    }

    /// Force open file with system default application (Shift+Enter)
    pub(crate) fn open_external(&mut self) -> Option<PanelEvent> {
        let te = self.tree_entry_at(self.selected)?;
        let full_path = te.full_path.clone();

        let entry = &self.tree_entry_at(self.selected)?.file_entry;
        determine_file_open_event(entry, &full_path, FileOpenMode::External)
    }

    /// Pick the directory new files / dirs should land in. The rule
    /// is "create alongside the cursor": at the same tree level as
    /// the highlighted entry. So cursor on a root-level item creates
    /// in the panel's `current_path`; cursor anywhere inside an
    /// expanded subdir creates in that subdir (the parent of the
    /// highlighted entry).
    pub(crate) fn create_target_dir(&self) -> (std::path::PathBuf, Option<termide_vfs::VfsPath>) {
        if let Some(te) = self.tree_entry_at(self.selected) {
            // Top-level rows (depth == 0) and ".." always anchor at
            // current_path. For any nested entry we use the parent of
            // its full_path, which corresponds to the visible subdir
            // the row belongs to.
            if te.depth > 0 && te.file_entry.name != ".." {
                if let Some(parent) = te.full_path.parent() {
                    let local = parent.to_path_buf();
                    let vfs = if self.vfs.is_remote() {
                        self.remote_vfs_path_for(&local)
                    } else {
                        None
                    };
                    return (local, vfs);
                }
            }
        }
        (
            self.current_path.clone(),
            if self.vfs.is_remote() {
                Some(self.vfs.current_path().clone())
            } else {
                None
            },
        )
    }

    /// Create a new file
    pub fn create_file(&mut self, name: String) -> Result<()> {
        validate_entry_name(&name)?;

        let (local_target, vfs_target) = self.create_target_dir();
        let target_full_path = local_target.join(&name);

        if self.vfs.is_remote() {
            let base = vfs_target.unwrap_or_else(|| self.vfs.current_path().clone());
            let new_path = base.join(&name);
            let operation = self.vfs.manager().write_file(&new_path, &[]);

            // Block until completion
            operation.recv()?;
        } else {
            fs::write(&target_full_path, "")?;
        }

        self.navigation.set_newly_created_path(target_full_path);
        self.load_directory()?;
        Ok(())
    }

    /// Create a new directory
    pub fn create_directory(&mut self, name: String) -> Result<()> {
        validate_entry_name(&name)?;

        let (local_target, vfs_target) = self.create_target_dir();
        let target_full_path = local_target.join(&name);

        if self.vfs.is_remote() {
            let base = vfs_target.unwrap_or_else(|| self.vfs.current_path().clone());
            let new_path = base.join(&name);
            let operation = self.vfs.manager().create_dir(&new_path);

            // Block until completion (sync behavior for UI)
            operation.recv()?;
        } else {
            fs::create_dir(&target_full_path)?;
        }

        self.navigation.set_newly_created_path(target_full_path);
        self.load_directory()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            is_dir: false,
            is_symlink: false,
            is_executable: false,
            is_readonly: false,
            git_status: GitStatus::Unmodified,
            size: Some(0),
            modified: None,
        }
    }

    #[test]
    fn database_files_open_in_db_viewer() {
        // Enter (Default) and F3 (View) on a SQLite file route to the DB viewer,
        // not the hex/binary viewer. Extension match is case-insensitive.
        for name in ["data.db", "app.sqlite", "cache.sqlite3", "x.DB3"] {
            let e = entry(name);
            let p = std::path::Path::new(name);
            assert!(
                matches!(
                    determine_file_open_event(&e, p, FileOpenMode::View),
                    Some(PanelEvent::ViewDatabase(_))
                ),
                "F3 on {name} should open the DB viewer"
            );
            assert!(
                matches!(
                    determine_file_open_event(&e, p, FileOpenMode::Default),
                    Some(PanelEvent::ViewDatabase(_))
                ),
                "Enter on {name} should open the DB viewer"
            );
        }
    }
}
