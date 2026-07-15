//! Git integration for editor.
//!
//! This module provides git diff tracking and visualization for the editor,
//! including line status markers, deletion markers, and diff cache management.

use ratatui::style::Color;
use std::sync::mpsc;

use termide_buffer::TextBuffer;
use termide_git::{load_original_async, GitDiffAsyncResult, GitDiffCache, LineStatus};
use termide_theme::Theme;

/// LSP diagnostic marker information for gutter
pub struct LspMarkerInfo {
    pub marker: char,
    pub color: Color,
}

/// Virtual line representation for rendering.
///
/// Allows inserting visual-only lines (like deletion markers, diagnostics) between real buffer lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualLine {
    /// Real line from the buffer at given index (0-based)
    Real(usize),
    /// Visual deletion indicator after the given buffer line index.
    ///
    /// Parameters: (after_line_idx, deletion_count)
    /// This is a visual-only line showing where content was deleted and how many lines.
    DeletionMarker(usize, usize),
    /// Diagnostic virtual line showing error/warning under the affected token.
    ///
    /// Row 0: `~~~  [code] message_start`
    /// Row 1+: `     message_continuation`
    Diagnostic {
        /// Buffer line index this diagnostic refers to
        line: usize,
        /// Start column of the underline (in the line above)
        start_col: usize,
        /// Length of the underline
        underline_len: usize,
        /// Diagnostic severity (ERROR, WARNING, etc.)
        severity: lsp_types::DiagnosticSeverity,
        /// Diagnostic code (e.g., "E0425")
        code: Option<String>,
        /// Diagnostic message
        message: String,
        /// Row index within this diagnostic (0 = first row with underline)
        row_index: usize,
        /// Total number of rows for this diagnostic
        total_rows: usize,
    },
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
                if let Err(e) = cache.update_from_buffer(&content) {
                    log::warn!("git diff cache update_from_buffer failed: {e}");
                }
            }

            return (true, None); // Update performed, clear pending
        }
    }

    (false, pending_time) // No update, keep pending time
}

/// Approximate relative luminance (ITU-R BT.601) for a ratatui Color.
fn color_luminance(color: Color) -> f32 {
    match color {
        Color::Rgb(r, g, b) => 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32,
        _ => 128.0, // fallback for named colors
    }
}

/// Pick fg that contrasts better with the given bg from two candidates.
fn pick_contrasting_fg(bg: Color, a: Color, b: Color) -> Color {
    let bg_lum = color_luminance(bg);
    let diff_a = (color_luminance(a) - bg_lum).abs();
    let diff_b = (color_luminance(b) - bg_lum).abs();
    if diff_a >= diff_b {
        a
    } else {
        b
    }
}

/// Get line number style based on git status.
///
/// Returns (fg_color, bg_color) for the line number.
/// - Green bg for added lines
/// - Yellow bg for modified lines
/// - No bg for unchanged lines
///
/// fg is chosen from theme.fg / theme.bg for best contrast against git bg.
pub fn get_line_number_git_style(
    line_idx: usize,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    theme: &Theme,
) -> (Color, Option<Color>) {
    if !show_git_diff {
        return (theme.disabled, None);
    }

    git_diff_cache
        .as_ref()
        .map(|cache| {
            let status = cache.get_line_status(line_idx);
            match status {
                LineStatus::Added => {
                    let fg = pick_contrasting_fg(theme.success, theme.fg, theme.bg);
                    (fg, Some(theme.success))
                }
                LineStatus::Modified => {
                    let fg = pick_contrasting_fg(theme.warning, theme.fg, theme.bg);
                    (fg, Some(theme.warning))
                }
                LineStatus::Unchanged => (theme.disabled, None),
                LineStatus::DeletedAfter => (theme.disabled, None),
            }
        })
        .unwrap_or((theme.disabled, None))
}

/// Get LSP diagnostic marker for gutter.
///
/// Returns the marker character and color for diagnostic severity.
/// - ▶/► (red) for ERROR
/// - ▶/► (yellow) for WARNING
/// - ' ' (space) for no diagnostic or INFO/HINT
///
/// On Windows U+25B6 ▶ is outside WGL4; U+25BA ► is used instead.
pub fn get_lsp_marker(
    diagnostic_severity: Option<lsp_types::DiagnosticSeverity>,
    theme: &Theme,
) -> LspMarkerInfo {
    const LSP_MARKER: char = if cfg!(windows) { '►' } else { '▶' };
    if let Some(severity) = diagnostic_severity {
        use lsp_types::DiagnosticSeverity;
        match severity {
            DiagnosticSeverity::ERROR => LspMarkerInfo {
                marker: LSP_MARKER,
                color: theme.error,
            },
            DiagnosticSeverity::WARNING => LspMarkerInfo {
                marker: LSP_MARKER,
                color: theme.warning,
            },
            _ => LspMarkerInfo {
                marker: ' ',
                color: theme.disabled,
            },
        }
    } else {
        LspMarkerInfo {
            marker: ' ',
            color: theme.disabled,
        }
    }
}

/// Build virtual lines visible in the viewport.
///
/// Only processes buffer lines starting from `viewport_top_line` and collects
/// at most `max_lines` virtual lines, avoiding O(N) iteration over the entire buffer.
///
/// # Parameters
/// - `viewport_top_line`: First buffer line visible in the viewport
/// - `max_lines`: Maximum number of virtual lines to collect (typically content_height)
/// - `content_width`: Available width for content (used for diagnostic wrapping)
pub fn build_virtual_lines_for_viewport(
    buffer: &TextBuffer,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    diagnostics_by_line: &std::collections::HashMap<usize, Vec<DiagnosticInfo>>,
    viewport_top_line: usize,
    max_lines: usize,
    content_width: usize,
) -> Vec<VirtualLine> {
    let mut virtual_lines = Vec::with_capacity(max_lines);
    let buffer_line_count = buffer.line_count();

    // Start from viewport_top_line instead of 0
    for line_idx in viewport_top_line..buffer_line_count {
        if virtual_lines.len() >= max_lines {
            break;
        }

        // Add real line first
        virtual_lines.push(VirtualLine::Real(line_idx));

        // Add deletion markers (git)
        if show_git_diff {
            if let Some(git_diff) = git_diff_cache.as_ref() {
                if git_diff.has_deletion_marker(line_idx) {
                    let deletion_count = git_diff.get_deletion_count(line_idx);
                    virtual_lines.push(VirtualLine::DeletionMarker(line_idx, deletion_count));
                }
            }
        }

        // Add diagnostic virtual lines for this line
        if let Some(line_diagnostics) = diagnostics_by_line.get(&line_idx) {
            for diag_info in line_diagnostics {
                // Calculate how many rows this diagnostic needs
                let total_rows = calculate_diagnostic_rows(
                    diag_info.start_col,
                    diag_info.underline_len,
                    diag_info.code.as_deref(),
                    &diag_info.message,
                    content_width,
                );

                // Create virtual line for each row of the diagnostic
                for row_index in 0..total_rows {
                    virtual_lines.push(VirtualLine::Diagnostic {
                        line: line_idx,
                        start_col: diag_info.start_col,
                        underline_len: diag_info.underline_len,
                        severity: diag_info.severity,
                        code: diag_info.code.clone(),
                        message: diag_info.message.clone(),
                        row_index,
                        total_rows,
                    });
                }
            }
        }
    }

    virtual_lines
}

/// Calculate how many visual rows a diagnostic message needs.
///
/// Row 0: `~~~  [code] message_start`
/// Row 1+: `          message_continuation`
pub fn calculate_diagnostic_rows(
    start_col: usize,
    underline_len: usize,
    code: Option<&str>,
    message: &str,
    content_width: usize,
) -> usize {
    use unicode_width::UnicodeWidthStr;

    if content_width == 0 {
        return 1;
    }

    // First row format: spaces + ~~~ + [code] + space + message
    let code_part_len = code.map(|c| c.len() + 3).unwrap_or(0); // " [code]"
    let first_row_prefix_len = start_col + underline_len + code_part_len + 1; // +1 for space

    if first_row_prefix_len >= content_width {
        // Not enough space even for prefix, just show 1 row
        return 1;
    }

    let msg_width = message.width();
    let first_row_msg_space = content_width.saturating_sub(first_row_prefix_len);

    if msg_width <= first_row_msg_space {
        // Message fits on first row
        return 1;
    }

    // Calculate continuation rows
    // Continuation format: indent + message_part
    let continuation_indent = start_col + 2; // Align with message start (after "~~")
    let continuation_space = content_width.saturating_sub(continuation_indent);

    if continuation_space == 0 {
        return 1;
    }

    let remaining_msg_width = msg_width.saturating_sub(first_row_msg_space);
    let continuation_rows = remaining_msg_width.div_ceil(continuation_space);

    1 + continuation_rows
}

/// Info about a single diagnostic for virtual line rendering.
#[derive(Debug, Clone)]
pub struct DiagnosticInfo {
    /// Start column for underline positioning
    pub start_col: usize,
    /// Length of the underline
    pub underline_len: usize,
    /// Diagnostic severity (ERROR, WARNING, etc.)
    pub severity: lsp_types::DiagnosticSeverity,
    /// Diagnostic code (e.g., "E0425")
    pub code: Option<String>,
    /// Diagnostic message
    pub message: String,
}

/// Group diagnostics by line and compute underline ranges.
///
/// Returns a map from line index to list of diagnostics on that line.
/// Each diagnostic has word-boundary-expanded underline positions.
/// Deduplicates diagnostics with the same message on the same line.
pub fn group_diagnostics_by_line(
    diagnostics: &[lsp_types::Diagnostic],
    buffer: &TextBuffer,
) -> std::collections::HashMap<usize, Vec<DiagnosticInfo>> {
    use std::collections::{HashMap, HashSet};

    let mut result: HashMap<usize, Vec<DiagnosticInfo>> = HashMap::new();
    // Track seen (line, message) pairs to deduplicate
    let mut seen: HashSet<(usize, String)> = HashSet::new();

    for diag in diagnostics {
        let line = diag.range.start.line as usize;

        // Skip duplicate messages on the same line
        if !seen.insert((line, diag.message.clone())) {
            continue;
        }

        let start_col = diag.range.start.character as usize;
        let end_col = diag.range.end.character as usize;

        // Expand to word boundaries
        let (word_start, word_end) = if let Some(line_text) = buffer.line(line) {
            (
                find_word_start(&line_text, start_col),
                find_word_end(&line_text, start_col).max(end_col),
            )
        } else {
            (start_col, end_col.max(start_col + 1))
        };

        let underline_len = word_end.saturating_sub(word_start).max(1);
        let severity = diag
            .severity
            .unwrap_or(lsp_types::DiagnosticSeverity::ERROR);

        // Extract diagnostic code
        let code = diag.code.as_ref().map(|c| match c {
            lsp_types::NumberOrString::Number(n) => n.to_string(),
            lsp_types::NumberOrString::String(s) => s.clone(),
        });

        result.entry(line).or_default().push(DiagnosticInfo {
            start_col: word_start,
            underline_len,
            severity,
            code,
            message: diag.message.clone(),
        });
    }

    result
}

/// Find the start of the word containing the given column.
fn find_word_start(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() {
        return col;
    }

    let mut start = col;
    while start > 0 {
        let ch = chars[start - 1];
        if !ch.is_alphanumeric() && ch != '_' {
            break;
        }
        start -= 1;
    }
    start
}

/// Find the end of the word containing the given column.
fn find_word_end(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() {
        return chars.len();
    }

    let mut end = col;
    while end < chars.len() {
        let ch = chars[end];
        if !ch.is_alphanumeric() && ch != '_' {
            break;
        }
        end += 1;
    }
    end
}

/// Get the virtual line at a given visual row position.
///
/// Returns the virtual line at the specified visual row offset from viewport.top_line.
/// Returns None if the row is out of bounds.
pub fn get_virtual_line_at_row(
    buffer: &TextBuffer,
    git_diff_cache: &Option<GitDiffCache>,
    show_git_diff: bool,
    diagnostics: &[lsp_types::Diagnostic],
    viewport_top_line: usize,
    visual_row: usize,
    content_width: usize,
) -> Option<VirtualLine> {
    // Rare path (mouse click), so building the HashMap here is fine.
    let diagnostics_by_line = group_diagnostics_by_line(diagnostics, buffer);
    let virtual_lines = build_virtual_lines_for_viewport(
        buffer,
        git_diff_cache,
        show_git_diff,
        &diagnostics_by_line,
        viewport_top_line,
        visual_row + 1,
        content_width,
    );

    virtual_lines.into_iter().nth(visual_row)
}
