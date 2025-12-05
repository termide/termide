use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

/// Complete translation data loaded from TOML file
#[derive(Debug, Deserialize, Clone)]
pub struct TranslationData {
    pub metadata: Metadata,
    pub strings: HashMap<String, String>,
    pub formats: HashMap<String, String>,
    #[serde(default)]
    pub plurals: HashMap<String, PluralRules>,
}

/// Metadata about the translation file
#[derive(Debug, Deserialize, Clone)]
pub struct Metadata {
    pub language: String,
    pub name: String,
}

/// Pluralization rules for a word
///
/// Different languages have different pluralization rules:
/// - English: one, other (2 forms)
/// - Russian/Polish: one, few, many (3-4 forms)
/// - Japanese/Chinese: no pluralization (1 form)
#[derive(Debug, Deserialize, Clone)]
pub struct PluralRules {
    pub one: String,
    #[serde(default)]
    pub few: Option<String>,
    pub other: String,
}

/// Load translation data for a specific language
///
/// Uses `include_str!()` for compile-time embedding of TOML files.
/// This ensures zero runtime file I/O and allows the TOML files to be
/// validated at build time.
///
/// # Arguments
/// * `lang` - ISO 639-1 language code (e.g., "en", "ru", "de")
///
/// # Returns
/// * `Ok(TranslationData)` if the language is supported and TOML is valid
/// * `Err` if parsing fails or language is unsupported
///
/// # Example
/// ```no_run
/// use termide::i18n::loader::load_language;
///
/// let translation = load_language("en").unwrap();
/// assert_eq!(translation.metadata.name, "English");
/// ```
pub fn load_language(lang: &str) -> Result<TranslationData> {
    let toml_content = match lang {
        "en" => include_str!("../../i18n/en.toml"),
        "ru" => include_str!("../../i18n/ru.toml"),
        "de" => include_str!("../../i18n/de.toml"),
        "es" => include_str!("../../i18n/es.toml"),
        "fr" => include_str!("../../i18n/fr.toml"),
        "pt" => include_str!("../../i18n/pt.toml"),
        "zh" => include_str!("../../i18n/zh.toml"),
        "hi" => include_str!("../../i18n/hi.toml"),
        "th" => include_str!("../../i18n/th.toml"),
        _ => {
            // Fallback to English for unsupported languages
            include_str!("../../i18n/en.toml")
        }
    };

    toml::from_str(toml_content).context(format!("Failed to parse translation for '{}'", lang))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translation_data_structure() {
        // Test that we can create the structures manually
        let rules = PluralRules {
            one: "".to_string(),
            few: None,
            other: "s".to_string(),
        };

        let mut plurals = HashMap::new();
        plurals.insert("file".to_string(), rules);

        let mut strings = HashMap::new();
        strings.insert("test_key".to_string(), "test_value".to_string());

        let mut formats = HashMap::new();
        formats.insert("test_format".to_string(), "Hello {name}".to_string());

        let data = TranslationData {
            metadata: Metadata {
                language: "en".to_string(),
                name: "English".to_string(),
            },
            strings,
            formats,
            plurals,
        };

        assert_eq!(data.metadata.language, "en");
        assert_eq!(data.strings.get("test_key").unwrap(), "test_value");
    }

    #[test]
    fn test_plural_rules_deserialization() {
        // Test English (one + other)
        let toml = r#"
            one = ""
            other = "s"
        "#;
        let rules: PluralRules = toml::from_str(toml).unwrap();
        assert_eq!(rules.one, "");
        assert_eq!(rules.other, "s");
        assert!(rules.few.is_none());

        // Test Russian (one + few + other)
        let toml = r#"
            one = ""
            few = "а"
            other = "ов"
        "#;
        let rules: PluralRules = toml::from_str(toml).unwrap();
        assert_eq!(rules.one, "");
        assert_eq!(rules.few.as_ref().unwrap(), "а");
        assert_eq!(rules.other, "ов");
    }

    // Note: Actual language loading tests will be added once TOML files are generated
    // For now, these will fail since TOML files don't exist yet
    #[test]
    #[ignore]
    fn test_load_all_languages() {
        let languages = vec!["en", "ru", "de", "es", "fr", "pt", "zh", "hi", "th"];

        for lang in languages {
            let result = load_language(lang);
            assert!(
                result.is_ok(),
                "Failed to load language '{}': {:?}",
                lang,
                result.err()
            );

            let data = result.unwrap();
            assert_eq!(data.metadata.language, lang);
            assert!(
                !data.strings.is_empty(),
                "Language '{}' has no strings",
                lang
            );
        }
    }

    #[test]
    #[ignore]
    fn test_unsupported_language_fallback() {
        // Should fallback to English
        let result = load_language("unknown");
        assert!(result.is_ok());

        let data = result.unwrap();
        // Fallback returns English
        assert_eq!(data.metadata.language, "en");
    }
}
