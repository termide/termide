//! Text buffer with rope data structure for termide.
//!
//! Provides efficient text storage and manipulation using ropey,
//! along with cursor management, history (undo/redo), viewport, and search.

mod buffer;
mod cursor;
mod history;
mod line_marks;
mod search;
mod viewport;
mod wrap;

pub use buffer::{LspContentChange, TextBuffer};
pub use cursor::{Cursor, Selection};
pub use history::{Action, History};
pub use search::SearchState;
pub use viewport::Viewport;
pub use wrap::{
    calculate_wrap_point, display_column, display_width, grapheme_columns, is_word_boundary,
};

/// Line ending type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    LF, // Unix \n
    CRLF, // Windows \r\n
}
