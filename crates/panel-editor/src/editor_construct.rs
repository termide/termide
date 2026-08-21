//! Editor construction: constructors, file opening, and the `Default` impl.

use anyhow::Result;
use std::path::PathBuf;

use termide_buffer::{Cursor, TextBuffer, Viewport};
use termide_core::HotkeyTable;
use termide_git::GitDiffCache;

use crate::{
    config::*,
    file_io,
    state::{FileState, GitIntegration, InputState, LspState, RenderingCache, SearchController},
    vim::VimState,
};

use super::Editor;

impl Editor {
    /// Create new empty editor with default configuration
    pub fn new() -> Self {
        Self::with_config(EditorConfig::default())
    }

    /// Create new empty editor with specified configuration
    pub fn with_config(config: EditorConfig) -> Self {
        let mut file_state = FileState::new();
        file_state.initial_directory = config.initial_directory.clone();

        // Initialize Vim state if vim mode is enabled
        let vim = if config.vim_mode {
            Some(VimState::new())
        } else {
            None
        };

        Self {
            config,
            buffer: TextBuffer::new(),
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            scrollbars: termide_core::ScrollBars::default(),
            file_state,
            search: SearchController::new(),
            git: GitIntegration::new(),
            render_cache: RenderingCache::new(),
            input: InputState::new(),
            lsp: LspState::new(),
            vfs_manager: None,
            find_bar: None,
            find_bar_focus_buffer: false,
            modal_request: None,
            pending_upload: None,
            pending_remote_open: None,
            config_update: None,
            status_message: None,
            scroll_follows_cursor: true,
            vim,
            symbol_lines: Vec::new(),
            is_stale: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            tab_size_override: None,
            syntax_picker: None,
        }
    }

    /// Open file with specified configuration
    pub fn open_file_with_config(path: PathBuf, mut config: EditorConfig) -> Result<Self> {
        // Check file size before loading and get modification time
        let metadata = file_io::check_file_metadata(&path)?;
        let file_size = metadata.size;
        let file_mtime = metadata.mtime;

        let buffer = TextBuffer::from_file(&path)?;

        // Check file access rights for auto-detection of read-only
        if file_io::is_file_readonly(&path) {
            log::warn!("File detected as read-only: {}", path.display());
            config.read_only = true;
        }

        // Create file state
        let file_state = FileState::from_path(&path, file_mtime, file_size);

        // Create rendering cache and set syntax by file extension
        let mut render_cache = RenderingCache::new();
        if config.syntax_highlighting {
            render_cache.highlight.set_syntax_from_path(&path);
        }

        // Initialize git integration
        let mut git = GitIntegration::new();
        let mut cache = GitDiffCache::new(path.clone());
        match cache.update() {
            Ok(()) => {
                git.diff_cache = Some(cache);
            }
            Err(e) => {
                log::warn!("Editor: GitDiffCache update failed for {:?}: {}", path, e);
            }
        }

        // Start blame loading immediately (blame is enabled by default)
        if let Some(repo_root) = termide_git::find_repo_root(&path) {
            git.start_blame(&repo_root, &path);
        }

        // Initialize Vim state if vim mode is enabled
        let vim = if config.vim_mode {
            Some(VimState::new())
        } else {
            None
        };

        Ok(Self {
            config,
            buffer,
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            scrollbars: termide_core::ScrollBars::default(),
            file_state,
            search: SearchController::new(),
            git,
            render_cache,
            input: InputState::new(),
            lsp: LspState::new(),
            vfs_manager: None,
            find_bar: None,
            find_bar_focus_buffer: false,
            modal_request: None,
            pending_upload: None,
            pending_remote_open: None,
            config_update: None,
            status_message: None,
            scroll_follows_cursor: true,
            vim,
            symbol_lines: Vec::new(),
            is_stale: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            tab_size_override: None,
            syntax_picker: None,
        })
    }

    /// Create editor with text (for displaying help, etc.)
    pub fn from_text(content: &str, title: String) -> Self {
        use ropey::Rope;

        // Create buffer directly through Rope
        let rope = Rope::from_str(content);

        let mut file_state = FileState::new();
        file_state.title = title;

        // view_only mode doesn't have vim enabled
        Self {
            config: EditorConfig::view_only(),
            buffer: TextBuffer::from_rope(rope),
            cursor: Cursor::new(),
            selection: None,
            viewport: Viewport::default(),
            scrollbars: termide_core::ScrollBars::default(),
            file_state,
            search: SearchController::new(),
            git: GitIntegration::new(),
            render_cache: RenderingCache::new(),
            input: InputState::new(),
            lsp: LspState::new(),
            vfs_manager: None,
            find_bar: None,
            find_bar_focus_buffer: false,
            modal_request: None,
            pending_upload: None,
            pending_remote_open: None,
            config_update: None,
            status_message: None,
            scroll_follows_cursor: true,
            vim: None, // view_only mode doesn't have vim
            symbol_lines: Vec::new(),
            is_stale: false,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            tab_size_override: None,
            syntax_picker: None,
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}
