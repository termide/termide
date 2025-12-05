mod buffer;
mod cursor;
mod highlighting;
mod history;
mod search;
mod viewport;
mod wrap;

pub use buffer::TextBuffer;
pub use cursor::{Cursor, Selection};
pub use highlighting::HighlightCache;
pub use history::{Action, History};
pub use search::SearchState;
pub use viewport::Viewport;
pub use wrap::{calculate_wrap_point, calculate_wrap_points_for_line};
