//! Symlink creation for batch file operations.

use anyhow::Result;
use std::path::{Path, PathBuf};

use super::super::App;
use termide_ui::path_utils;

fn relative_path_from(base_dir: &Path, target: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let base_components: Vec<Component<'_>> = base_dir.components().collect();
    let target_components: Vec<Component<'_>> = target.components().collect();

    let mut common_len = 0usize;
    while common_len < base_components.len()
        && common_len < target_components.len()
        && base_components[common_len] == target_components[common_len]
    {
        common_len += 1;
    }

    if common_len == 0 && base_dir.is_absolute() != target.is_absolute() {
        return None;
    }

    let mut result = PathBuf::new();
    for component in &base_components[common_len..] {
        if matches!(component, Component::Normal(_)) {
            result.push("..");
        }
    }
    for component in &target_components[common_len..] {
        result.push(component.as_os_str());
    }

    if result.as_os_str().is_empty() {
        Some(PathBuf::from("."))
    } else {
        Some(result)
    }
}

fn symlink_target_path(
    source: &Path,
    link_path: &Path,
    use_relative: bool,
) -> std::io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(source)?;
    if !use_relative {
        return Ok(canonical);
    }

    let Some(parent) = link_path.parent() else {
        return Ok(canonical);
    };
    let canonical_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    Ok(relative_path_from(&canonical_parent, &canonical).unwrap_or(canonical))
}

impl App {
    /// Handle symlink creation instead of copy
    pub(in crate::app::modal) fn handle_create_symlinks(
        &mut self,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
        create_relative_symlink: bool,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        // Extract destination string
        let dest_str = if let Some(s) = value.downcast_ref::<String>() {
            s.clone()
        } else {
            return Ok(());
        };

        let Some((absolute_destination, destination_is_directory)) =
            self.resolve_local_destination_input(target_directory.as_ref(), &dest_str)
        else {
            return Ok(());
        };

        #[allow(unused_mut)]
        let mut success_count = 0usize;
        let mut error_count = 0usize;
        let is_single_source = sources.len() == 1;

        for source in &sources {
            let link_path = path_utils::resolve_batch_destination_path(
                source,
                &absolute_destination,
                is_single_source,
                destination_is_directory,
            );

            if let Some(parent) = link_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    log::error!(
                        "Failed to create parent directory {}: {}",
                        parent.display(),
                        e
                    );
                    error_count += 1;
                    continue;
                }
            }

            #[cfg(unix)]
            let target_path = match symlink_target_path(source, &link_path, create_relative_symlink)
            {
                Ok(p) => p,
                Err(e) => {
                    log::error!(
                        "Failed to resolve symlink target for {}: {}",
                        source.display(),
                        e
                    );
                    error_count += 1;
                    continue;
                }
            };

            #[cfg(unix)]
            match std::os::unix::fs::symlink(&target_path, &link_path) {
                Ok(()) => success_count += 1,
                Err(e) => {
                    log::error!(
                        "Failed to create symlink {} -> {}: {}",
                        link_path.display(),
                        target_path.display(),
                        e
                    );
                    error_count += 1;
                }
            }

            #[cfg(not(unix))]
            {
                let _ = create_relative_symlink;
                log::error!("Symlink creation is only supported on Unix");
                error_count += 1;
            }
        }

        if error_count == 0 {
            let t = termide_i18n::t();
            self.state.set_info(format!(
                "{} {} symlink{}",
                t.batch_result_copied(),
                success_count,
                if success_count == 1 { "" } else { "s" }
            ));
        } else {
            self.show_error_modal(format!(
                "Symlinks: {} created, {} errors",
                success_count, error_count
            ));
        }

        self.state.needs_redraw = true;
        Ok(())
    }
}
