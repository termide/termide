use std::time::SystemTime;
use chrono::{DateTime, Local};

/// File rename pattern
#[derive(Debug, Clone)]
pub struct RenamePattern {
    template: String,
}

impl RenamePattern {
    /// Create new rename pattern
    pub fn new(template: String) -> Self {
        Self { template }
    }

    /// Apply pattern to filename
    pub fn apply(
        &self,
        original_name: &str,
        counter: usize,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> String {
        let parts = Self::split_filename(original_name);
        let mut result = self.template.clone();

        // Replace $0 (full name)
        result = result.replace("$0", original_name);

        // Replace $1-9 (parts from left)
        for i in 1..=9 {
            let placeholder = format!("${}", i);
            let value = parts.get(i - 1).map(|s| s.as_str()).unwrap_or("");
            result = result.replace(&placeholder, value);
        }

        // Replace $-1 to $-9 (parts from right)
        for i in 1..=9 {
            let placeholder = format!("$-{}", i);
            let idx = parts.len().saturating_sub(i);
            let value = parts.get(idx).map(|s| s.as_str()).unwrap_or("");
            result = result.replace(&placeholder, value);
        }

        // Replace $I (counter)
        result = result.replace("$I", &counter.to_string());

        // Replace $C (creation time)
        if let Some(time) = created {
            result = result.replace("$C", &Self::format_time(time));
        } else {
            result = result.replace("$C", "");
        }

        // Replace $M (modification time)
        if let Some(time) = modified {
            result = result.replace("$M", &Self::format_time(time));
        } else {
            result = result.replace("$M", "");
        }

        result
    }

    /// Split filename into parts by dots
    fn split_filename(filename: &str) -> Vec<String> {
        filename.split('.').map(|s| s.to_string()).collect()
    }

    /// Format time to YYYYMMDD_HHMMSS string
    fn format_time(time: SystemTime) -> String {
        let datetime: DateTime<Local> = time.into();
        datetime.format("%Y%m%d_%H%M%S").to_string()
    }

    /// Get preview result for example
    pub fn preview(&self, example_name: &str) -> String {
        self.apply(example_name, 1, None, None)
    }

    /// Check if result contains forbidden characters
    pub fn is_valid_result(&self, result: &str) -> bool {
        // Forbidden characters in filenames
        let forbidden = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
        !result.is_empty() && !result.chars().any(|c| forbidden.contains(&c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_replacement() {
        let pattern = RenamePattern::new("$0".to_string());
        assert_eq!(pattern.preview("file.txt"), "file.txt");
    }

    #[test]
    fn test_parts_from_left() {
        let pattern = RenamePattern::new("$1_copy.$2".to_string());
        assert_eq!(pattern.preview("document.txt"), "document_copy.txt");
    }

    #[test]
    fn test_parts_from_right() {
        let pattern = RenamePattern::new("$1_backup.$-1".to_string());
        assert_eq!(pattern.preview("archive.tar.gz"), "archive_backup.gz");
    }

    #[test]
    fn test_counter() {
        let pattern = RenamePattern::new("$1_$I.$-1".to_string());
        assert_eq!(pattern.apply("file.txt", 5, None, None), "file_5.txt");
    }

    #[test]
    fn test_complex_pattern() {
        let pattern = RenamePattern::new("$1_$I.$2.$3".to_string());
        assert_eq!(pattern.preview("document.tar.gz"), "document_1.tar.gz");
    }

    #[test]
    fn test_missing_parts() {
        let pattern = RenamePattern::new("$1.$5".to_string());
        assert_eq!(pattern.preview("file.txt"), "file.");
    }

    #[test]
    fn test_validation() {
        let pattern = RenamePattern::new("$1_copy.$-1".to_string());
        assert!(pattern.is_valid_result("file_copy.txt"));
        assert!(!pattern.is_valid_result("file/copy.txt"));
        assert!(!pattern.is_valid_result("file:copy.txt"));
        assert!(!pattern.is_valid_result(""));
    }
}
