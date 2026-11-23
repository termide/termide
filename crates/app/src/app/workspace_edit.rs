//! Apply LSP WorkspaceEdit: a file open in an editor is changed in its buffer
//! (left unsaved), and a file that is not open is written on disk.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf, TextEdit, WorkspaceEdit};

use crate::PanelExt;

use super::App;

impl App {
    /// Apply an LSP WorkspaceEdit: edit the buffers of files open in an
    /// editor, write the other files to disk.
    ///
    /// Every file on disk is read and edited in memory first; nothing is
    /// changed when one cannot be read, when a URI names no local file, or
    /// when the edit asks to create, rename or delete files, which this
    /// client does not do. The files are then written atomically, each
    /// either whole or not at all. Returns the number of files modified.
    pub(super) fn apply_workspace_edit(&mut self, edit: WorkspaceEdit) -> Result<usize> {
        let edits_by_file = collect_text_edits(edit)?;
        if edits_by_file.is_empty() {
            return Ok(0);
        }

        let mut resolved: Vec<(PathBuf, Vec<TextEdit>)> = Vec::with_capacity(edits_by_file.len());
        for (uri, text_edits) in edits_by_file {
            let path = termide_lsp::uri_to_path(&uri)
                .ok_or_else(|| anyhow!("not a local file: {}", uri.as_str()))?;
            resolved.push((path, text_edits));
        }

        // Prefer the open editor's buffer: the server positioned the edit
        // against that document (it may hold unsaved changes), so writing
        // disk and reloading would both misplace the edit and discard the
        // user's unsaved work. Only files not open in any editor are touched
        // on disk.
        let is_open = |app: &mut Self, path: &PathBuf| {
            app.layout_manager
                .iter_all_panels_mut()
                .filter_map(|panel| panel.as_editor_mut())
                .any(|editor| editor.file_path() == Some(path.as_path()))
        };

        let mut on_disk: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
        let mut in_editor: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
        for (path, text_edits) in resolved {
            if is_open(self, &path) {
                in_editor.push((path, text_edits));
            } else {
                on_disk.push((path, text_edits));
            }
        }
        // Disk first: a failure there is reported before any buffer changes.
        let written = write_staged(&stage_files(on_disk)?)?;

        // Files we edited in place (open in an editor) — these need a didChange
        // so the server's model matches and it doesn't re-apply the edit.
        let mut edited_in_editor: Vec<PathBuf> = Vec::new();
        for (path, text_edits) in in_editor {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    if editor.file_path() == Some(path.as_path()) {
                        editor.apply_workspace_edits(&text_edits);
                        break;
                    }
                }
            }
            if !edited_in_editor.contains(&path) {
                edited_in_editor.push(path);
            }
        }
        let file_count = written.len() + edited_in_editor.len();

        // Resync each edited open buffer with its language server. Without this,
        // a server that performed the edit via executeCommand (e.g. phpactor
        // "Import class") still believes the file is unchanged and re-applies
        // the same edit next time — duplicating the `use` statement.
        if let Some(lsp_manager) = self.state.lsp_manager.as_ref() {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    let is_edited = editor
                        .file_path()
                        .map(|p| edited_in_editor.iter().any(|m| m == p))
                        .unwrap_or(false);
                    if is_edited {
                        editor.notify_lsp_change(lsp_manager);
                    }
                }
            }
        }

        Ok(file_count)
    }
}

/// The new content of every file in `edits`, read from disk and edited in
/// memory; an error, and nothing staged, when one cannot be read. A later
/// entry for the same file edits the result of the earlier one, as the
/// specification orders them.
fn stage_files(edits: Vec<(PathBuf, Vec<TextEdit>)>) -> Result<Vec<(PathBuf, String)>> {
    let mut staged: Vec<(PathBuf, String)> = Vec::new();
    for (path, text_edits) in edits {
        match staged.iter_mut().find(|(p, _)| *p == path) {
            Some((_, content)) => *content = apply_text_edits(content, &text_edits),
            None => {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                let content = apply_text_edits(&content, &text_edits);
                staged.push((path, content));
            }
        }
    }
    Ok(staged)
}

/// Write each staged file atomically; the paths written, or an error naming
/// the file that failed and those already written before it.
fn write_staged(staged: &[(PathBuf, String)]) -> Result<Vec<PathBuf>> {
    let mut written: Vec<PathBuf> = Vec::new();
    for (path, content) in staged {
        if let Err(e) = termide_fsutil::write_atomic(path, content.as_bytes()) {
            let mut message = format!("cannot write {}: {e}", path.display());
            if !written.is_empty() {
                let names: Vec<String> = written.iter().map(|p| p.display().to_string()).collect();
                message.push_str(&format!("; already written: {}", names.join(", ")));
            }
            bail!(message);
        }
        written.push(path.clone());
    }
    Ok(written)
}

/// The text edits of `edit`, file by file, in the order they apply.
///
/// `changes` wins when both forms are present. `documentChanges` may hold
/// file operations (create, rename, delete) between its text edits; this
/// client does not perform them, and applying the text edits without them
/// would leave references to files that never appeared, so such an edit is
/// refused whole.
fn collect_text_edits(edit: WorkspaceEdit) -> Result<Vec<(lsp_types::Uri, Vec<TextEdit>)>> {
    let text_edits = |edits: Vec<OneOf<TextEdit, lsp_types::AnnotatedTextEdit>>| {
        edits
            .into_iter()
            .map(|e| match e {
                OneOf::Left(te) => te,
                OneOf::Right(annotated) => annotated.text_edit,
            })
            .collect::<Vec<_>>()
    };

    if let Some(changes) = edit.changes {
        return Ok(changes.into_iter().collect());
    }
    match edit.document_changes {
        None => Ok(Vec::new()),
        Some(DocumentChanges::Edits(edits)) => Ok(edits
            .into_iter()
            .map(|te| (te.text_document.uri, text_edits(te.edits)))
            .collect()),
        Some(DocumentChanges::Operations(operations)) => {
            let mut out = Vec::new();
            let mut unsupported = Vec::new();
            for operation in operations {
                match operation {
                    DocumentChangeOperation::Edit(te) => {
                        out.push((te.text_document.uri, text_edits(te.edits)));
                    }
                    DocumentChangeOperation::Op(op) => unsupported.push(match op {
                        lsp_types::ResourceOp::Create(c) => format!("create {}", c.uri.as_str()),
                        lsp_types::ResourceOp::Rename(r) => {
                            format!("rename {} to {}", r.old_uri.as_str(), r.new_uri.as_str())
                        }
                        lsp_types::ResourceOp::Delete(d) => format!("delete {}", d.uri.as_str()),
                    }),
                }
            }
            if !unsupported.is_empty() {
                bail!(
                    "file operations are not supported, nothing was changed ({})",
                    unsupported.join("; ")
                );
            }
            Ok(out)
        }
    }
}

/// Apply a list of `TextEdit`s to a string (content of a file).
///
/// Edits address the original document, so we resolve every `(line, character)`
/// position to an absolute character offset once, then splice from the end of
/// the document toward the start — that way earlier offsets stay valid as we go.
/// Edits that share a start offset (e.g. phpactor's "Import class" inserts both
/// a blank line and a `use` line at the same point) must end up in array order;
/// since we apply back-to-front, the later array entry is spliced first, hence
/// the descending original-index tie-break. The whole content (including line
/// endings) is preserved verbatim except where an edit replaces it.
///
/// Lines break at `\n`, `\r\n` and a lone `\r`, and `character` counts UTF-16
/// code units, as LSP states both; a `character` past the end of its line
/// stands for the line's end, before its line break.
fn apply_text_edits(content: &str, edits: &[TextEdit]) -> String {
    let chars: Vec<char> = content.chars().collect();

    // line_start[i] = index into `chars` where line `i` begins.
    let mut line_start = vec![0usize];
    for (i, &c) in chars.iter().enumerate() {
        let lone_cr = c == '\r' && chars.get(i + 1) != Some(&'\n');
        if c == '\n' || lone_cr {
            line_start.push(i + 1);
        }
    }

    let pos_to_offset = |line: u32, character: u32| -> usize {
        let l = line as usize;
        if l >= line_start.len() {
            return chars.len();
        }
        let base = line_start[l];
        // The line's text, without its line break: a character offset never
        // spills past it into the next line.
        let mut end = line_start.get(l + 1).copied().unwrap_or(chars.len());
        if end > base && chars[end - 1] == '\n' {
            end -= 1;
        }
        if end > base && chars[end - 1] == '\r' {
            end -= 1;
        }
        let mut units = 0;
        let mut offset = base;
        while offset < end {
            units += chars[offset].len_utf16();
            if units > character as usize {
                break;
            }
            offset += 1;
        }
        offset
    };

    // (start_offset, end_offset, new_text, original_index)
    let mut spans: Vec<(usize, usize, &str, usize)> = edits
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let start = pos_to_offset(e.range.start.line, e.range.start.character);
            let end = pos_to_offset(e.range.end.line, e.range.end.character);
            (start, end.max(start), e.new_text.as_str(), i)
        })
        .collect();
    spans.sort_by(|a, b| b.0.cmp(&a.0).then(b.3.cmp(&a.3)));

    let mut out = chars;
    for (start, end, new_text, _) in spans {
        let start = start.min(out.len());
        let end = end.min(out.len());
        out.splice(start..end, new_text.chars());
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{apply_text_edits, collect_text_edits, stage_files, write_staged};
    use lsp_types::{Position, Range, TextEdit};

    fn replace(line: u32, from: u32, to: u32, new_text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position {
                    line,
                    character: from,
                },
                end: Position {
                    line,
                    character: to,
                },
            },
            new_text: new_text.to_string(),
        }
    }

    /// LSP columns count UTF-16 code units: an emoji is two, so `old` after
    /// `"😀 "` starts at character 3, not at char 2.
    #[test]
    fn columns_are_utf16_code_units() {
        let content = "s = \"😀 old\";\nnext\n";
        let result = apply_text_edits(content, &[replace(0, 8, 11, "new")]);
        assert_eq!(result, "s = \"😀 new\";\nnext\n");
    }

    #[test]
    fn a_column_past_the_line_end_stops_before_the_line_break() {
        for content in ["ab\ncd\n", "ab\r\ncd\r\n"] {
            let result = apply_text_edits(content, &[replace(0, 99, 99, "!")]);
            assert_eq!(result, content.replacen("ab", "ab!", 1), "{content:?}");
        }
    }

    /// One unreadable file stops the edit before any file is written.
    #[test]
    fn nothing_is_written_when_one_file_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("a.txt");
        std::fs::write(&present, "old\n").unwrap();
        let missing = dir.path().join("missing.txt");
        let result = stage_files(vec![
            (present.clone(), vec![replace(0, 0, 3, "new")]),
            (missing, vec![replace(0, 0, 0, "x")]),
        ]);
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&present).unwrap(), "old\n");

        // Two entries for one file apply one after the other.
        let staged = stage_files(vec![
            (present.clone(), vec![replace(0, 0, 3, "new")]),
            (present.clone(), vec![replace(0, 3, 3, "er")]),
        ])
        .unwrap();
        assert_eq!(write_staged(&staged).unwrap(), vec![present.clone()]);
        assert_eq!(std::fs::read_to_string(&present).unwrap(), "newer\n");
    }

    fn uri(path: &str) -> lsp_types::Uri {
        path.parse().unwrap()
    }

    fn doc_edit(path: &str, edit: TextEdit) -> lsp_types::TextDocumentEdit {
        lsp_types::TextDocumentEdit {
            text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                uri: uri(path),
                version: None,
            },
            edits: vec![lsp_types::OneOf::Left(edit)],
        }
    }

    #[test]
    fn document_change_operations_yield_their_text_edits() {
        use lsp_types::{DocumentChangeOperation, DocumentChanges, WorkspaceEdit};
        let edit = WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(vec![
                DocumentChangeOperation::Edit(doc_edit("file:///a.rs", replace(0, 0, 1, "x"))),
                DocumentChangeOperation::Edit(doc_edit("file:///b.rs", replace(1, 0, 0, "y"))),
            ])),
            ..Default::default()
        };
        let edits = collect_text_edits(edit).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[1].0.as_str(), "file:///b.rs");
        assert_eq!(edits[1].1[0].new_text, "y");
    }

    #[test]
    fn file_operations_refuse_the_whole_edit() {
        use lsp_types::{DocumentChangeOperation, DocumentChanges, ResourceOp, WorkspaceEdit};
        let edit = WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(vec![
                DocumentChangeOperation::Op(ResourceOp::Rename(lsp_types::RenameFile {
                    old_uri: uri("file:///a.rs"),
                    new_uri: uri("file:///b.rs"),
                    options: None,
                    annotation_id: None,
                })),
                DocumentChangeOperation::Edit(doc_edit("file:///b.rs", replace(0, 0, 0, "x"))),
            ])),
            ..Default::default()
        };
        let error = collect_text_edits(edit).unwrap_err().to_string();
        assert!(
            error.contains("rename file:///a.rs to file:///b.rs"),
            "{error}"
        );
    }

    fn insert(line: u32, character: u32, new_text: &str) -> TextEdit {
        let pos = Position { line, character };
        TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text: new_text.to_string(),
        }
    }

    // phpactor's "Import class" delivers two zero-width inserts at the SAME
    // position. The line-based predecessor mis-ordered/duplicated them; the
    // offset splice must reproduce the server's intent and leave the usage line
    // (and every other line) untouched. Payloads below are captured verbatim
    // from phpactor 2025.12.21.1.
    #[test]
    fn phpactor_import_with_namespace() {
        let content = "<?php\n\nnamespace App;\n\nclass Controller\n{\n    public function index()\n    {\n        return Order::where('id', 1)->get();\n    }\n}\n";
        // Both inserts land at end of `namespace App;` (line 2, char 14).
        let edits = vec![
            insert(2, 14, "\n"),
            insert(2, 14, "\nuse App\\Models\\Order\\Order;"),
        ];
        let result = apply_text_edits(content, &edits);
        assert!(
            result.contains("namespace App;\n\nuse App\\Models\\Order\\Order;\n\nclass Controller"),
            "import not placed correctly:\n{result}"
        );
        // The class usage must be preserved exactly.
        assert!(result.contains("        return Order::where('id', 1)->get();\n"));
    }

    #[test]
    fn phpactor_import_without_namespace() {
        let content = "<?php\n\nclass Top\n{\n    public function run()\n    {\n        return Order::where('id', 1)->get();\n    }\n}\n";
        // Both inserts land at the start of line 1; note the array order here is
        // the reverse of the namespaced case.
        let edits = vec![
            insert(1, 0, "\nuse App\\Models\\Order\\Order;"),
            insert(1, 0, "\n"),
        ];
        let result = apply_text_edits(content, &edits);
        assert!(
            result.starts_with("<?php\n\nuse App\\Models\\Order\\Order;\n\nclass Top"),
            "import not placed correctly:\n{result}"
        );
        assert!(result.contains("        return Order::where('id', 1)->get();\n"));
    }

    #[test]
    fn multiline_replacement_spans_and_collapses_lines() {
        let content = "alpha\nbeta\ngamma\ndelta\n";
        // Replace from line 1 col 0 through line 2 col 0 with "BETA\n".
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 0,
                },
            },
            new_text: "BETA\n".to_string(),
        };
        assert_eq!(
            apply_text_edits(content, &[edit]),
            "alpha\nBETA\ngamma\ndelta\n"
        );
    }

    #[test]
    fn single_line_replacement_keeps_surrounding_text() {
        let content = "let foo = oldName(x);\n";
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: 0,
                    character: 10,
                },
                end: Position {
                    line: 0,
                    character: 17,
                },
            },
            new_text: "newName".to_string(),
        };
        assert_eq!(
            apply_text_edits(content, &[edit]),
            "let foo = newName(x);\n"
        );
    }
}
