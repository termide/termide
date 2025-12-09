//! Git integration for editor.
//!
//! This module provides git diff tracking and visualization for the editor,
//! including line status markers, deletion markers, and diff cache management.

use ratatui::style::Color;
use std::sync::mpsc;

use crate::editor::TextBuffer;
use crate::git::{load_original_async, GitDiffAsyncResult, GitDiffCache, LineStatus};

/// Git line status information for rendering
pub struct GitLineInfo {
    pub status_color: Color,
    pub status_marker: char,
}

/// Virtual line representation for rendering.
///
/// Allows inserting visual-only lines (like deletion markers) between real buffer lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualLine {
    /// Real line from the buffer at given index (0-based)
    Real(usize),
    /// Visual deletion indicator after the given buffer line index.
    ///
    /// Parameters: (after_line_idx, deletion_count)
    /// This is a visual-only line showing where content was deleted and how many lines.
    DeletionMarker(usize, usize),
}

/// Start async git diff update by spawning background thread.
///
/// Creates a new cache if needed and returns a receiver for the async result.
/// The caller should store this receiver and poll it on each tick.
pub fn update_git_diff_async(
    git_diff_cache: &mut Option<GitDiffCache>,
    file_path: Option<&std::path::Path>,
) -> Option<mpsc::Receiver<GitDiffAsyncResult>> {
    let file_path = file_path?;

    // Ensure cache exists
    if git_diff_cache.is_none() {
        *git_diff_cache = Some(GitDiffCache::new(file_path.to_path_buf()));
    }

    // Spawn async load in background thread
    Some(load_original_async(file_path.to_path_buf()))
}

/// Check async git diff receiver and apply result if ready.
///
/// Returns true if result was applied, false otherwise.
pub fn check_git_diff_receiver(
    receiver: &mut Option<mpsc::Receiver<GitDiffAsyncResult>>,
    git_diff_cache: &mut Option<GitDiffCache>,
) -> bool {
    let rx = match receiver {
        Some(rx) => rx,
        None => return false,
    };

    // Try to receive result without blocking
    match rx.try_recv() {
        Ok(result) => {
            // Apply result to cache
            if let Some(cache) = git_diff_cache {
                cache.apply_async_result(result);
            }
            // Clear receiver
            *receiver = None;
            true
        }
        Err(mpsc::TryRecvError::Empty) => {
            // Not ready yet
            false
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            // Thread finished without sending (shouldn't happen)
            *receiver = None;
            false
        }
    }
}

/// Schedule git diff update with debounce.
///
/// Returns Some(Instant) if update was scheduled, None otherwise.
pub fn schedule_git_diff_update(
    git_diff_cache: &Option<GitDiffCache>,
) -> Option<std::time::Instant> {
    // Only schedule if we have a git diff cache
    if git_diff_cache.is_some() {
        Some(std::time::Instant::now())
    } else {
        None
    }
}

/// Check and apply pending git diff update if debounce time has passed.
///
/// Returns true if update was performed, false otherwise.
pub fn check_pending_git_diff_update(
    pending_time: Option<std::time::Instant>,
    git_diff_cache: &mut Option<GitDiffCache>,
    buffer: &TextBuffer,
) -> (bool, Option<std::time::Instant>) {
    const DEBOUNCE_MS: u64 = 300;

    if let Some(pending_time) = pending_time {
        if pending_time.elapsed().as_millis() >= DEBOUNCE_MS as u128 {
            // Time has passed, perform update
            let content = buffer.to_string();

            // Update diff cache with current buffer
            if let Some(ref mut cache) = git_diff_cache {
                let _ = cache.update_from_buffer(&content);
            }

            return (true, None); // Update performed, clear pending
        }
    }

    (false, pending_time) // No update, keep pending time
}

/// Get git line information for rendering.
pub fn get_git_line_info(
    line_idx: usize,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    theme: &crate::theme::Theme,
) -> GitLineInfo {
    if !show_git_diff {
        return GitLineInfo {
            status_color: theme.disabled,
            status_marker: ' ',
        };
    }

    git_diff_cache
        .as_ref()
        .map(|cache| {
            let status = cache.get_line_status(line_idx);

            // Status marker and color
            let (status_color, status_marker) = match status {
                LineStatus::Added => (theme.success, ' '),
                LineStatus::Modified => (theme.warning, ' '),
                LineStatus::Unchanged => (theme.disabled, ' '),
                LineStatus::DeletedAfter => (theme.disabled, ' '),
            };

            GitLineInfo {
                status_color,
                status_marker,
            }
        })
        .unwrap_or(GitLineInfo {
            status_color: theme.disabled,
            status_marker: ' ',
        })
}

/// Build list of virtual lines (real buffer lines + deletion marker lines).
///
/// Returns a Vec mapping visual row index to VirtualLine.
pub fn build_virtual_lines(
    buffer: &TextBuffer,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
) -> Vec<VirtualLine> {
    let mut virtual_lines = Vec::new();
    let buffer_line_count = buffer.line_count();

    // If git diff is disabled or not available, just return real lines
    if !show_git_diff || git_diff_cache.is_none() {
        for line_idx in 0..buffer_line_count {
            virtual_lines.push(VirtualLine::Real(line_idx));
        }
        return virtual_lines;
    }

    let git_diff = git_diff_cache.as_ref().unwrap();

    // Interleave real lines with deletion markers
    for line_idx in 0..buffer_line_count {
        virtual_lines.push(VirtualLine::Real(line_idx));

        // Check if there's a deletion marker after this line
        if git_diff.has_deletion_marker(line_idx) {
            let deletion_count = git_diff.get_deletion_count(line_idx);
            virtual_lines.push(VirtualLine::DeletionMarker(line_idx, deletion_count));
        }
    }

    virtual_lines
}
