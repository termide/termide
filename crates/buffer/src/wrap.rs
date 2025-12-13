//! Word wrapping utilities for smart line breaking at word boundaries
//!
//! This module provides functions for intelligent line wrapping that respects
//! word boundaries when possible, falling back to hard breaks for words wider
//! than the viewport.

use unicode_width::UnicodeWidthStr;

/// Calculate the optimal wrap point for a line segment using graphemes
///
/// This function tries to find a word boundary (non-alphanumeric character)
/// to break the line at, but will force a break at max_width if:
/// - No word boundary is found (single long word)
/// - The word would be wider than the viewport
///
/// Uses display width and grapheme clusters for proper Unicode handling
/// (CJK characters, combining characters like Hindi vowel signs, etc.)
///
/// # Arguments
/// * `graphemes` - The line grapheme clusters to wrap
/// * `start` - Starting position in the grapheme array
/// * `max_width` - Maximum display width before wrapping (content width)
/// * `line_len` - Total length of the line (grapheme count)
///
/// # Returns
/// The grapheme index where the line should be wrapped
pub fn calculate_wrap_point(
    graphemes: &[&str],
    start: usize,
    max_width: usize,
    line_len: usize,
) -> usize {
    if start >= line_len {
        return line_len;
    }

    // Find the grapheme index where display width exceeds max_width
    let mut display_width = 0;
    let mut ideal_end = start;

    for (i, grapheme) in graphemes
        .iter()
        .enumerate()
        .skip(start)
        .take(line_len - start)
    {
        let grapheme_width = grapheme.width();

        if display_width + grapheme_width > max_width {
            ideal_end = i;
            break;
        }

        display_width += grapheme_width;
        ideal_end = i + 1;
    }

    // If we reached end of line, no wrapping needed
    if ideal_end >= line_len {
        return line_len;
    }

    // Check if grapheme is a word boundary (first char is non-alphanumeric)
    let is_boundary = |g: &str| g.chars().next().is_none_or(|c| !c.is_alphanumeric());

    // If the grapheme at ideal_end is a word boundary, we can break there
    if ideal_end < line_len && is_boundary(graphemes[ideal_end]) {
        return ideal_end + 1;
    }

    // Search backwards from ideal_end for a word boundary
    for i in (start..ideal_end).rev() {
        if is_boundary(graphemes[i]) {
            // Found a boundary - wrap after this grapheme
            // But avoid wrapping right after start (would create empty visual line)
            if i > start {
                return i + 1;
            }
        }
    }

    // No word boundary found - this means we have a single long word
    // Force break at ideal_end to prevent horizontal overflow
    ideal_end.max(start + 1) // Ensure at least one grapheme is included
}

/// Calculate all wrap points for a line
///
/// Returns a vector of indices where the line should be wrapped.
/// Each index represents the start of a new visual line.
///
/// Uses display width and grapheme clusters for proper Unicode handling.
///
/// # Arguments
/// * `line_text` - The text of the line
/// * `max_width` - Maximum display width before wrapping (content width)
///
/// # Returns
/// Vector of wrap points (grapheme indices). Empty if line doesn't need wrapping.
#[allow(dead_code)]
pub fn calculate_wrap_points_for_line(line_text: &str, max_width: usize) -> Vec<usize> {
    use unicode_segmentation::UnicodeSegmentation;

    let graphemes: Vec<&str> = line_text.graphemes(true).collect();
    let line_len = graphemes.len();

    // Check display width, not grapheme count
    if line_text.width() <= max_width {
        return Vec::new(); // No wrapping needed
    }

    let mut wrap_points = Vec::new();
    let mut grapheme_offset = 0;

    while grapheme_offset < line_len {
        let chunk_end = calculate_wrap_point(&graphemes, grapheme_offset, max_width, line_len);

        // Only add wrap point if we're not at the start
        if grapheme_offset > 0 {
            wrap_points.push(grapheme_offset);
        }

        grapheme_offset = chunk_end;

        // Safety: prevent infinite loop
        if chunk_end == grapheme_offset && grapheme_offset < line_len {
            grapheme_offset += 1;
        }
    }

    wrap_points
}

/// Check if a character is a word boundary
///
/// Word boundaries are non-alphanumeric characters (spaces, punctuation, etc.)
/// This is used by the wrapping algorithm and word selection.
pub fn is_word_boundary(c: char) -> bool {
    !c.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_wrap_needed() {
        let text = "Short line";
        let wrap_points = calculate_wrap_points_for_line(text, 80);
        assert_eq!(wrap_points.len(), 0);
    }

    #[test]
    fn test_wrap_at_space() {
        let text = "This is a long line that needs to be wrapped at spaces";
        let wrap_points = calculate_wrap_points_for_line(text, 20);
        assert!(!wrap_points.is_empty());

        // Check that wrap points are reasonable
        for &point in &wrap_points {
            assert!(point > 0 && point < text.len());
        }
    }

    #[test]
    fn test_long_word_force_break() {
        // Single word wider than viewport
        let text = "verylongwordthatcannotbebrokenatanyboundary";
        let wrap_points = calculate_wrap_points_for_line(text, 10);
        assert!(!wrap_points.is_empty());

        // Should force breaks every 10 characters
        assert!(wrap_points.len() >= 3);
    }

    #[test]
    fn test_unicode() {
        let text = "Привет мир как дела это тест юникода";
        let wrap_points = calculate_wrap_points_for_line(text, 15);
        // Should wrap on spaces between Cyrillic words
        assert!(!wrap_points.is_empty());
    }

    #[test]
    fn test_mixed_alphanumeric() {
        let text = "function_name123 another_function456 test789";
        let wrap_points = calculate_wrap_points_for_line(text, 20);
        // Underscores are not alphanumeric, so they're word boundaries
        assert!(!wrap_points.is_empty());
    }

    #[test]
    fn test_calculate_wrap_point_basic() {
        use unicode_segmentation::UnicodeSegmentation;

        let text = "hello world test";
        let graphemes: Vec<&str> = text.graphemes(true).collect();

        // Should wrap after "hello "
        let wrap_point = calculate_wrap_point(&graphemes, 0, 10, graphemes.len());
        assert_eq!(wrap_point, 6); // After space
    }

    #[test]
    fn test_calculate_wrap_point_long_word() {
        use unicode_segmentation::UnicodeSegmentation;

        let text = "verylongword";
        let graphemes: Vec<&str> = text.graphemes(true).collect();

        // Should force break at max_width
        let wrap_point = calculate_wrap_point(&graphemes, 0, 5, graphemes.len());
        assert_eq!(wrap_point, 5);
    }

    #[test]
    fn test_is_word_boundary() {
        assert!(is_word_boundary(' '));
        assert!(is_word_boundary('.'));
        assert!(is_word_boundary(','));
        assert!(is_word_boundary('!'));
        assert!(is_word_boundary('_'));

        assert!(!is_word_boundary('a'));
        assert!(!is_word_boundary('Z'));
        assert!(!is_word_boundary('5'));
        assert!(!is_word_boundary('ж')); // Cyrillic
        assert!(!is_word_boundary('中')); // Chinese
    }
}
