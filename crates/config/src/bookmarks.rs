//! Bookmarks configuration for termide.
//!
//! Provides bookmark storage with grouping support and smart type detection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::get_config_dir;

/// Bookmarks configuration containing all user bookmarks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookmarksConfig {
    /// List of all bookmarks
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
}

/// A single bookmark entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    /// Path or URL (required)
    pub path: String,
    /// Creation timestamp (required)
    pub created_at: DateTime<Utc>,
    /// Optional description (if None, filename is used for display)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional group name (if None, goes to "Ungrouped")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Whether this bookmark comes from a project-local `.termide/bookmarks.toml`.
    #[serde(skip)]
    pub is_project: bool,
}

/// Type of bookmark based on path analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkType {
    /// Path is a directory
    Directory,
    /// Path is a text file (.txt, .md, .rs, .toml, etc.)
    TextFile,
    /// Path is a viewer file (.pdf, .png, .jpg, etc.) - open with external viewer
    ViewerFile,
    /// Path is an HTTP/HTTPS link
    HttpLink,
    /// SSH connection (ssh://user@host) - opens terminal
    SshConnection,
    /// SFTP remote path (sftp://user@host/path)
    SftpPath,
    /// FTP remote path (ftp://host/path)
    FtpPath,
    /// SMB/CIFS remote path (smb://server/share/path)
    SmbPath,
    /// NFS remote path (nfs://server/export/path)
    NfsPath,
    /// Database connection (postgres://, mysql://, sqlite://) - opens DB viewer
    Database,
    /// Unknown type (fallback)
    Unknown,
}

impl BookmarkType {
    /// Get icon for this bookmark type.
    #[must_use]
    pub fn icon(&self) -> &'static str {
        match self {
            BookmarkType::Directory => "📁",
            BookmarkType::TextFile => "📄",
            BookmarkType::ViewerFile => "🖼",
            BookmarkType::HttpLink => "🌐",
            BookmarkType::SshConnection => "💻",
            BookmarkType::SftpPath | BookmarkType::FtpPath => "📡",
            BookmarkType::SmbPath | BookmarkType::NfsPath => "🖧",
            BookmarkType::Database => "📋",
            BookmarkType::Unknown => "📌",
        }
    }

    /// Check if this bookmark type is a remote/network path.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            BookmarkType::SftpPath
                | BookmarkType::FtpPath
                | BookmarkType::SmbPath
                | BookmarkType::NfsPath
        )
    }

    /// Check if this bookmark type is a web link.
    #[must_use]
    pub fn is_web(&self) -> bool {
        matches!(self, BookmarkType::HttpLink)
    }

    /// Check if this bookmark type is a local filesystem path.
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(
            self,
            BookmarkType::Directory | BookmarkType::TextFile | BookmarkType::ViewerFile
        )
    }
}

impl Bookmark {
    /// Create a new bookmark with the given path.
    pub fn new(path: String) -> Self {
        Self {
            path,
            created_at: Utc::now(),
            description: None,
            group: None,
            is_project: false,
        }
    }

    /// Create a new bookmark with description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Create a new bookmark with group.
    #[must_use]
    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Determine the bookmark type based on path.
    #[must_use]
    pub fn bookmark_type(&self) -> BookmarkType {
        // Check for HTTP links first
        if self.path.starts_with("http://") || self.path.starts_with("https://") {
            return BookmarkType::HttpLink;
        }

        // Check for SSH connection (terminal)
        if self.path.starts_with("ssh://") {
            return BookmarkType::SshConnection;
        }

        // Check for network filesystem URLs
        if self.path.starts_with("sftp://") {
            return BookmarkType::SftpPath;
        }
        if self.path.starts_with("ftp://") {
            return BookmarkType::FtpPath;
        }
        if self.path.starts_with("smb://") || self.path.starts_with("cifs://") {
            return BookmarkType::SmbPath;
        }
        if self.path.starts_with("nfs://") {
            return BookmarkType::NfsPath;
        }

        // Database connection URLs (opened by the DB viewer panel).
        if self.path.starts_with("postgres://")
            || self.path.starts_with("postgresql://")
            || self.path.starts_with("mysql://")
            || self.path.starts_with("mariadb://")
            || self.path.starts_with("sqlite://")
        {
            return BookmarkType::Database;
        }

        let path = Path::new(&self.path);

        // Check if it's a directory
        if path.is_dir() {
            return BookmarkType::Directory;
        }

        // Check extension for viewer files
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                // Image formats
                "pdf" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico" |
                // Audio formats
                "mp3" | "wav" | "ogg" | "flac" |
                // Video formats
                "mp4" | "mkv" | "avi" | "mov" | "webm" => {
                    return BookmarkType::ViewerFile;
                }
                _ => {}
            }
        }

        // Default to text file if path exists, unknown otherwise
        if path.exists() {
            BookmarkType::TextFile
        } else {
            BookmarkType::Unknown
        }
    }

    /// Check if this bookmark is a remote/network path.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.bookmark_type().is_remote()
    }

    /// Get display name for the bookmark.
    ///
    /// Returns description if set, otherwise extracts filename from path.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.description.as_deref().unwrap_or_else(|| {
            Path::new(&self.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&self.path)
        })
    }

    /// Get group name for the bookmark.
    ///
    /// Returns group if set, otherwise "Ungrouped".
    #[must_use]
    pub fn group_name(&self) -> &str {
        self.group.as_deref().unwrap_or("Ungrouped")
    }
}

impl BookmarksConfig {
    /// Get the path to the bookmarks config file.
    pub fn config_file_path() -> anyhow::Result<PathBuf> {
        let config_dir = get_config_dir()?;
        Ok(config_dir.join("bookmarks.toml"))
    }

    /// Load bookmarks from a config directory.
    pub fn load_from_dir(config_dir: &Path) -> Self {
        let path = config_dir.join("bookmarks.toml");
        if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// Load bookmarks from a project-local `.termide/bookmarks.toml`.
    /// All loaded bookmarks are marked with `is_project = true`.
    pub fn load_from_project(project_root: &Path) -> Option<Self> {
        let path = project_root.join(".termide").join("bookmarks.toml");
        if !path.exists() {
            return None;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        let mut config: Self = toml::from_str(&content).ok()?;
        for bookmark in &mut config.bookmarks {
            bookmark.is_project = true;
        }
        Some(config)
    }

    /// Load bookmarks from the default config directory.
    pub fn load() -> Self {
        match get_config_dir() {
            Ok(config_dir) => Self::load_from_dir(&config_dir),
            Err(_) => Self::default(),
        }
    }

    /// Save bookmarks to a config directory.
    pub fn save_to_dir(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("bookmarks.toml");
        let content = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, content)
    }

    /// Save bookmarks to the default config directory.
    pub fn save(&self) -> std::io::Result<()> {
        let config_dir = get_config_dir().map_err(std::io::Error::other)?;
        self.save_to_dir(&config_dir)
    }

    /// Add a bookmark (ignores duplicates within same group).
    ///
    /// Same path can exist in different groups, but not duplicated within same group.
    pub fn add(&mut self, bookmark: Bookmark) {
        let exists = self
            .bookmarks
            .iter()
            .any(|b| b.path == bookmark.path && b.group == bookmark.group);
        if !exists {
            self.bookmarks.push(bookmark);
        }
    }

    /// Remove a bookmark by path (removes all with this path).
    pub fn remove(&mut self, path: &str) {
        self.bookmarks.retain(|b| b.path != path);
    }

    /// Remove a bookmark by path and group.
    pub fn remove_in_group(&mut self, path: &str, group: Option<&str>) {
        self.bookmarks
            .retain(|b| !(b.path == path && b.group.as_deref() == group));
    }

    /// Remove all bookmarks in a group.
    pub fn remove_group(&mut self, group: &str) {
        self.bookmarks.retain(|b| b.group_name() != group);
    }

    /// Check if a bookmark exists for the given path.
    pub fn contains(&self, path: &str) -> bool {
        self.bookmarks.iter().any(|b| b.path == path)
    }

    /// Find a bookmark by path.
    pub fn find(&self, path: &str) -> Option<&Bookmark> {
        self.bookmarks.iter().find(|b| b.path == path)
    }

    /// Find a bookmark by path and group.
    pub fn find_in_group(&self, path: &str, group: Option<&str>) -> Option<&Bookmark> {
        self.bookmarks
            .iter()
            .find(|b| b.path == path && b.group.as_deref() == group)
    }

    /// Find a mutable bookmark by path.
    pub fn find_mut(&mut self, path: &str) -> Option<&mut Bookmark> {
        self.bookmarks.iter_mut().find(|b| b.path == path)
    }

    /// Get bookmarks grouped by group name, sorted alphabetically.
    ///
    /// Returns a BTreeMap where keys are group names and values are
    /// vectors of bookmark references sorted by display name.
    pub fn grouped(&self) -> BTreeMap<String, Vec<&Bookmark>> {
        let mut groups: BTreeMap<String, Vec<&Bookmark>> = BTreeMap::new();

        for bookmark in &self.bookmarks {
            let group = bookmark
                .group
                .clone()
                .unwrap_or_else(|| "Ungrouped".to_string());
            groups.entry(group).or_default().push(bookmark);
        }

        // Sort bookmarks within each group alphabetically by display name
        for bookmarks in groups.values_mut() {
            bookmarks.sort_by(|a, b| a.display_name().cmp(b.display_name()));
        }

        groups
    }

    /// Get only directory bookmarks (for Ctrl+P integration).
    #[must_use]
    pub fn directories(&self) -> Vec<&Bookmark> {
        self.bookmarks
            .iter()
            .filter(|b| b.bookmark_type() == BookmarkType::Directory)
            .collect()
    }

    /// Get all unique group names.
    #[must_use]
    pub fn group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .bookmarks
            .iter()
            .filter_map(|b| b.group.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Get ungrouped bookmarks (those without a group).
    #[must_use]
    pub fn ungrouped(&self) -> Vec<&Bookmark> {
        let mut items: Vec<&Bookmark> = self
            .bookmarks
            .iter()
            .filter(|b| b.group.is_none())
            .collect();
        items.sort_by(|a, b| a.display_name().cmp(b.display_name()));
        items
    }

    /// Get named groups (excluding ungrouped bookmarks).
    #[must_use]
    pub fn named_groups(&self) -> BTreeMap<String, Vec<&Bookmark>> {
        let mut groups: BTreeMap<String, Vec<&Bookmark>> = BTreeMap::new();
        for bookmark in &self.bookmarks {
            if let Some(group) = &bookmark.group {
                groups.entry(group.clone()).or_default().push(bookmark);
            }
        }
        for bookmarks in groups.values_mut() {
            bookmarks.sort_by(|a, b| a.display_name().cmp(b.display_name()));
        }
        groups
    }

    /// Check if there are any bookmarks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bookmarks.is_empty()
    }

    /// Get the total number of bookmarks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bookmarks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bookmark_new() {
        let bookmark = Bookmark::new("/home/user/project".to_string());
        assert_eq!(bookmark.path, "/home/user/project");
        assert!(bookmark.description.is_none());
        assert!(bookmark.group.is_none());
    }

    #[test]
    fn test_bookmark_with_description() {
        let bookmark =
            Bookmark::new("/home/user/project".to_string()).with_description("My project");
        assert_eq!(bookmark.description, Some("My project".to_string()));
    }

    #[test]
    fn test_bookmark_with_group() {
        let bookmark = Bookmark::new("/home/user/project".to_string()).with_group("Projects");
        assert_eq!(bookmark.group, Some("Projects".to_string()));
    }

    #[test]
    fn test_bookmark_display_name_with_description() {
        let bookmark =
            Bookmark::new("/home/user/project".to_string()).with_description("My project");
        assert_eq!(bookmark.display_name(), "My project");
    }

    #[test]
    fn test_bookmark_display_name_without_description() {
        let bookmark = Bookmark::new("/home/user/project".to_string());
        assert_eq!(bookmark.display_name(), "project");
    }

    #[test]
    fn test_bookmark_type_http() {
        let bookmark = Bookmark::new("https://docs.rs".to_string());
        assert_eq!(bookmark.bookmark_type(), BookmarkType::HttpLink);

        let bookmark = Bookmark::new("http://example.com".to_string());
        assert_eq!(bookmark.bookmark_type(), BookmarkType::HttpLink);
    }

    #[test]
    fn test_bookmark_type_viewer_file() {
        let bookmark = Bookmark::new("/home/user/image.png".to_string());
        assert_eq!(bookmark.bookmark_type(), BookmarkType::ViewerFile);

        let bookmark = Bookmark::new("/home/user/document.pdf".to_string());
        assert_eq!(bookmark.bookmark_type(), BookmarkType::ViewerFile);
    }

    #[test]
    fn test_bookmarks_config_add() {
        let mut config = BookmarksConfig::default();
        config.add(Bookmark::new("/path/one".to_string()));
        assert_eq!(config.len(), 1);

        // Adding duplicate (same path, same group) should be ignored
        config.add(Bookmark::new("/path/one".to_string()));
        assert_eq!(config.len(), 1);

        // Adding same path to different group should succeed
        config.add(Bookmark::new("/path/one".to_string()).with_group("Projects"));
        assert_eq!(config.len(), 2);

        // Adding same path to same group again should be ignored
        config.add(Bookmark::new("/path/one".to_string()).with_group("Projects"));
        assert_eq!(config.len(), 2);
    }

    #[test]
    fn test_bookmarks_config_remove() {
        let mut config = BookmarksConfig::default();
        config.add(Bookmark::new("/path/one".to_string()));
        config.add(Bookmark::new("/path/two".to_string()));
        assert_eq!(config.len(), 2);

        config.remove("/path/one");
        assert_eq!(config.len(), 1);
        assert!(!config.contains("/path/one"));
        assert!(config.contains("/path/two"));
    }

    #[test]
    fn test_bookmarks_config_grouped() {
        let mut config = BookmarksConfig::default();
        config.add(Bookmark::new("/projects/a".to_string()).with_group("Projects"));
        config.add(Bookmark::new("/projects/b".to_string()).with_group("Projects"));
        config.add(Bookmark::new("/config/c".to_string()).with_group("Config"));
        config.add(Bookmark::new("/misc/d".to_string())); // Ungrouped

        let grouped = config.grouped();
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped.get("Projects").map(|v| v.len()), Some(2));
        assert_eq!(grouped.get("Config").map(|v| v.len()), Some(1));
        assert_eq!(grouped.get("Ungrouped").map(|v| v.len()), Some(1));
    }

    #[test]
    fn test_bookmarks_config_group_names() {
        let mut config = BookmarksConfig::default();
        config.add(Bookmark::new("/a".to_string()).with_group("Projects"));
        config.add(Bookmark::new("/b".to_string()).with_group("Config"));
        config.add(Bookmark::new("/c".to_string())); // No group

        let names = config.group_names();
        assert_eq!(names, vec!["Config", "Projects"]);
    }
}
