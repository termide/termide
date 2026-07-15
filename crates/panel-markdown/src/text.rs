//! Pure text/geometry helpers for the Markdown preview (columns, slicing, search).

use unicode_width::UnicodeWidthChar;

/// The `#fragment` part of a URL, if present and non-empty.
pub(crate) fn url_fragment(url: &str) -> Option<String> {
    url.split_once('#')
        .map(|(_, f)| f.to_string())
        .filter(|f| !f.is_empty())
}

/// Whether `path` (or URL) ends in a known raster-image extension.
pub(crate) fn is_image_path(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif"
    )
}

/// Substring of `s` between character indices `[start, end)`.
pub(crate) fn slice_chars(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Display column at character index `col` (sum of preceding char widths).
pub(crate) fn char_col_to_display(s: &str, col: usize) -> usize {
    s.chars().take(col).map(|c| c.width().unwrap_or(0)).sum()
}

/// Character index at (or just past) display column `disp`.
pub(crate) fn display_to_char_col(s: &str, disp: u16) -> usize {
    let target = disp as usize;
    let mut acc = 0usize;
    for (i, c) in s.chars().enumerate() {
        if acc >= target {
            return i;
        }
        acc += c.width().unwrap_or(0);
    }
    s.chars().count()
}

/// Character indices where `needle` occurs in `line` (case-insensitive when `ci`).
pub(crate) fn find_in_line(line: &str, needle: &str, ci: bool) -> Vec<usize> {
    let hay: Vec<char> = line.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if pat.is_empty() || pat.len() > hay.len() {
        return out;
    }
    let eq = |a: char, b: char| {
        if ci {
            a.eq_ignore_ascii_case(&b) || a.to_lowercase().eq(b.to_lowercase())
        } else {
            a == b
        }
    };
    for i in 0..=hay.len() - pat.len() {
        if (0..pat.len()).all(|j| eq(hay[i + j], pat[j])) {
            out.push(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_in_line_case_insensitive() {
        assert_eq!(find_in_line("Foo foo FOO", "foo", true), vec![0, 4, 8]);
        assert_eq!(find_in_line("Foo foo FOO", "foo", false), vec![4]);
    }
}
