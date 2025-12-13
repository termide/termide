// Allow clippy lints for file operations
#![allow(clippy::only_used_in_recursion)]

use anyhow::Result;
use std::fs;
use std::path::PathBuf;

use super::FileManager;
use termide_ui::path_utils;

impl FileManager {
    /// Create a new file
    pub fn create_file(&mut self, name: String) -> Result<()> {
        let file_path = self.current_path.join(&name);
        fs::write(&file_path, "")?;
        self.load_directory()?;
        Ok(())
    }

    /// Create a new directory
    pub fn create_directory(&mut self, name: String) -> Result<()> {
        let dir_path = self.current_path.join(&name);
        fs::create_dir(&dir_path)?;
        self.load_directory()?;
        Ok(())
    }

    /// Delete file or directory
    pub fn delete_path(&mut self, path: PathBuf) -> Result<()> {
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        self.load_directory()?;
        Ok(())
    }

    /// Copy file or directory
    pub fn copy_path(&mut self, source: PathBuf, destination: PathBuf) -> Result<()> {
        if source.is_dir() {
            self.copy_directory_recursive(&source, &destination)?;
        } else {
            let dest_path = path_utils::resolve_destination_path(&source, &destination);
            fs::copy(&source, &dest_path)?;
        }
        self.load_directory()?;
        Ok(())
    }

    /// Recursively copy directory
    fn copy_directory_recursive(&self, source: &PathBuf, destination: &PathBuf) -> Result<()> {
        self.copy_directory_recursive_with_depth(source, destination, 0)
    }

    /// Recursively copy directory with depth limit
    fn copy_directory_recursive_with_depth(
        &self,
        source: &PathBuf,
        destination: &PathBuf,
        depth: usize,
    ) -> Result<()> {
        const MAX_DEPTH: usize = termide_ui::constants::MAX_DIRECTORY_COPY_DEPTH;

        if depth > MAX_DEPTH {
            return Err(anyhow::anyhow!(
                "Directory nesting too deep (> {})",
                MAX_DEPTH
            ));
        }

        // Create target directory if it doesn't exist
        if !destination.exists() {
            fs::create_dir_all(destination)?;
        }

        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let source_path = entry.path();
            let dest_path = destination.join(entry.file_name());

            // Check metadata without following symlinks
            let metadata = fs::symlink_metadata(&source_path)?;

            if metadata.is_symlink() {
                // Copy symlink as symlink (don't follow it)
                #[cfg(unix)]
                {
                    use std::os::unix::fs as unix_fs;
                    let link_target = fs::read_link(&source_path)?;
                    unix_fs::symlink(link_target, &dest_path)?;
                }
                #[cfg(not(unix))]
                {
                    // On Windows, just copy as file
                    fs::copy(&source_path, &dest_path)?;
                }
            } else if metadata.is_dir() {
                // Recursively copy directory with incremented depth counter
                self.copy_directory_recursive_with_depth(&source_path, &dest_path, depth + 1)?;
            } else {
                // Regular file
                fs::copy(&source_path, &dest_path)?;
            }
        }

        Ok(())
    }

    /// Move file or directory
    pub fn move_path(&mut self, source: PathBuf, destination: PathBuf) -> Result<()> {
        let dest_path = path_utils::resolve_destination_path(&source, &destination);

        // Try simple rename (works only within same filesystem)
        if fs::rename(&source, &dest_path).is_err() {
            // If that failed - copy and delete
            if source.is_dir() {
                self.copy_directory_recursive(&source, &dest_path)?;
                fs::remove_dir_all(&source)?;
            } else {
                fs::copy(&source, &dest_path)?;
                fs::remove_file(&source)?;
            }
        }

        self.load_directory()?;
        Ok(())
    }
}
