//! Pack prompt result: start packing the selection into a new archive.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::super::App;
use crate::state::ActiveModal;
use crate::state::OperationType;
use termide_file_ops::OperationRequest;
use termide_ui::path_utils;
use termide_vfs::archive::pack::CompressionLevel;
use termide_vfs::archive::ArchiveFormat;

impl App {
    /// Copy the pack modal's compression row into `PendingAction::PackPaths`
    /// before the modal closes.
    pub(in crate::app) fn sync_pack_level(&mut self) {
        let Some(ActiveModal::EditableSelect(modal)) = &self.state.active_modal else {
            return;
        };
        let Some(index) = modal.choice_index() else {
            return;
        };
        if let Some(termide_state::PendingAction::PackPaths { level, .. }) =
            &mut self.state.pending_action
        {
            if let Some(chosen) = CompressionLevel::ALL.get(index) {
                *level = *chosen;
            }
        }
    }

    /// Start a tracked pack operation into the archive the prompt named.
    /// A relative path is taken from the selection's directory; a
    /// directory gets an archive named after the selection inside it.
    pub(in crate::app) fn handle_pack_paths(
        &mut self,
        sources: Vec<PathBuf>,
        level: CompressionLevel,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(input) = value.downcast_ref::<String>() else {
            return Ok(());
        };
        let Some(first) = sources.first() else {
            return Ok(());
        };
        let base = first.parent().unwrap_or(Path::new("/")).to_path_buf();
        let (mut archive, is_dir) =
            path_utils::resolve_local_destination_input(&base, input.trim());
        if is_dir || archive.is_dir() {
            let stem = match &sources[..] {
                [single] => single
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                _ => "archive".to_string(),
            };
            archive = archive.join(format!("{stem}.zip"));
        }

        let t = termide_i18n::t();
        let name = archive
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !ArchiveFormat::from_file_name(&name).is_some_and(ArchiveFormat::can_pack) {
            self.show_error_modal(t.fm_pack_unknown_format(&name));
            return Ok(());
        }
        if archive.symlink_metadata().is_ok() {
            self.show_error_modal(t.fm_pack_exists(&name));
            return Ok(());
        }

        let source_display = match &sources[..] {
            [single] => single.display().to_string(),
            many => t.status_item_count(many.len()),
        };
        let request = OperationRequest::pack(sources, archive.clone()).with_compression(level);
        if let Err(e) = self.start_tracked_operation(
            request,
            Arc::new(termide_vfs::VfsManager::new()),
            OperationType::Pack,
            source_display,
            archive.display().to_string(),
            0,
            0,
        ) {
            log::error!("Failed to start pack operation: {e}");
            self.show_error_modal(e.to_string());
        }
        Ok(())
    }
}
