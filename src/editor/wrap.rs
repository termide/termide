//! Word wrapping utilities for smart line breaking at word boundaries
//!
//! This module provides functions for intelligent line wrapping that respects
//! word boundaries when possible, falling back to hard breaks for words wider
//! than the viewport.

/// Calculate the optimal wrap point for a line segment
///
/// This function tries to find a word boundary (non-alphanumeric character)
/// to break the line at, but will force a break at max_width if:
/// - No word boundary is found (single long word)
/// - The word would be wider than the viewport
///
/// # Arguments
/// * `chars` - The line characters to wrap
/// * `start` - Starting position in the char array
/// * `max_width` - Maximum width before wrapping (content width)
/// * `line_len` - Total length of the line
///
/// # Returns
/// The index where the line should be wrapped
pub fn calculate_wrap_point(
    chars: &[char],
    start: usize,
    max_width: usize,
    line_len: usize,
) -> usize {
    let ideal_end = (start + max_width).min(line_len);

    // If we're at or past the end of the line, no wrapping needed
    if ideal_end >= line_len {
        return line_len;
    }

    // If the next character after ideal_end is a word boundary, we can break there
    if ideal_end < line_len && !chars[ideal_end].is_alphanumeric() {
        return ideal_end + 1;
    }

    // Search backwards from ideal_end for a word boundary
    for i in (start..ideal_end).rev() {
        if !chars[i].is_alphanumeric() {
            // Found a boundary - wrap after this character
            // But avoid wrapping right after start (would create empty visual line)
            if i > start {
                return i + 1;
            }
        }
    }

    // No word boundary found - this means we have a single long word
    // Force break at max_width to prevent horizontal overflow
    ideal_end
}

/// Calculate all wrap points for a line
///
/// Returns a vector of indices where the line should be wrapped.
/// Each index represents the start of a new visual line.
///
/// # Arguments
/// * `line_text` - The text of the line
/// * `max_width` - Maximum width before wrapping (content width)
///
/// # Returns
/// Vector of wrap points (character indices). Empty if line doesn't need wrapping.
#[allow(dead_code)]
pub fn calculate_wrap_points_for_line(line_text: &str, max_width: usize) -> Vec<usize> {
    let chars: Vec<char> = line_text.chars().collect();
    let line_len = chars.len();

    if line_len <= max_width {
        return Vec::new(); // No wrapping needed
    }

    let mut wrap_points = Vec::new();
    let mut char_offset = 0;

    while char_offset < line_len {
        let chunk_end = calculate_wrap_point(&chars, char_offset, max_width, line_len);

        // Only add wrap point if we're not at the start
        if char_offset > 0 {
            wrap_points.push(char_offset);
        }

        char_offset = chunk_end;

        // Safety: prevent infinite loop
        if chunk_end == char_offset && char_offset < line_len {
            char_offset += 1;
        }
    }

    wrap_points
}

/// Check if a character is a word boundary
///
/// Word boundaries are non-alphanumeric characters (spaces, punctuation, etc.)
/// This is used internally by the wrapping algorithm.
#[allow(dead_code)]
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
        let text = "hello world test";
        let chars: Vec<char> = text.chars().collect();

        // Should wrap after "hello "
        let wrap_point = calculate_wrap_point(&chars, 0, 10, chars.len());
        assert_eq!(wrap_point, 6); // After space
    }

    #[test]
    fn test_calculate_wrap_point_long_word() {
        let text = "verylongword";
        let chars: Vec<char> = text.chars().collect();

        // Should force break at max_width
        let wrap_point = calculate_wrap_point(&chars, 0, 5, chars.len());
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
