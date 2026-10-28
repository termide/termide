//! The buffer as the highlighter sees it: its text plus the edits that led
//! there, so the syntax tree is updated instead of rebuilt.

use ropey::Rope;
use termide_buffer::{EditPoint, EditRevision, SyntaxEdit, TextBuffer};
use termide_highlight::{DocumentRevision, DocumentSource};
use tree_sitter::{InputEdit, Point};

pub(crate) struct BufferDocument<'a>(pub &'a TextBuffer);

impl DocumentSource for BufferDocument<'_> {
    fn rope(&self) -> &Rope {
        self.0.rope()
    }

    fn revision(&self) -> DocumentRevision {
        let EditRevision { document, seq } = self.0.edit_revision();
        DocumentRevision {
            document,
            edit: seq,
        }
    }

    fn edits_since(&self, since: DocumentRevision) -> Option<Vec<InputEdit>> {
        let since = EditRevision {
            document: since.document,
            seq: since.edit,
        };
        Some(self.0.syntax_edits_since(since)?.map(input_edit).collect())
    }
}

fn input_edit(edit: &SyntaxEdit) -> InputEdit {
    let point = |p: EditPoint| Point::new(p.row, p.column);
    InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: point(edit.start_position),
        old_end_position: point(edit.old_end_position),
        new_end_position: point(edit.new_end_position),
    }
}
