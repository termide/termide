//! Editor configuration and information types.

/// Editor mode configuration
#[derive(Debug, Clone)]
pub struct EditorConfig {
    /// Whether syntax highlighting is enabled
    pub syntax_highlighting: bool,
    /// Read-only mode
    pub read_only: bool,
    /// Automatic line wrapping by window width
    pub word_wrap: bool,
    /// Tab size (number of spaces)
    pub tab_size: usize,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: false,
            word_wrap: true,
            tab_size: 4,
        }
    }
}

impl EditorConfig {
    /// Create configuration for view mode (without editing)
    pub fn view_only() -> Self {
        Self {
            syntax_highlighting: true,
            read_only: true,
            word_wrap: true,
            tab_size: 4,
        }
    }
}

/// Editor information for status bar
#[derive(Debug, Clone)]
pub struct EditorInfo {
    pub line: usize,               // Current line (1-based)
    pub column: usize,             // Current column (1-based)
    pub tab_size: usize,           // Tab size
    pub encoding: String,          // Encoding (UTF-8)
    pub file_type: String,         // File type / syntax language
    pub read_only: bool,           // Read-only mode
    pub syntax_highlighting: bool, // Syntax highlighting enabled
}
