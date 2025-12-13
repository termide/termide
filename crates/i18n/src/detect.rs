//! Language detection from environment.

/// Detect language from environment variables.
///
/// Checks in order: TERMIDE_LANG, LANG, LC_ALL.
/// Falls back to "en" if none found.
pub fn detect_language() -> String {
    // Check TERMIDE_LANG first (app-specific)
    if let Ok(lang) = std::env::var("TERMIDE_LANG") {
        return normalize_lang(&lang);
    }

    // Check LANG
    if let Ok(lang) = std::env::var("LANG") {
        return normalize_lang(&lang);
    }

    // Check LC_ALL
    if let Ok(lang) = std::env::var("LC_ALL") {
        return normalize_lang(&lang);
    }

    // Default to English
    "en".to_string()
}

/// Normalize language string (e.g., "ru_RU.UTF-8" -> "ru").
pub fn normalize_lang(lang: &str) -> String {
    let normalized = lang
        .split('_')
        .next()
        .unwrap_or("en")
        .split('.')
        .next()
        .unwrap_or("en")
        .to_lowercase();

    if normalized.is_empty() {
        "en".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_lang() {
        assert_eq!(normalize_lang("ru_RU.UTF-8"), "ru");
        assert_eq!(normalize_lang("en_US"), "en");
        assert_eq!(normalize_lang("de"), "de");
        assert_eq!(normalize_lang(""), "en");
    }
}
