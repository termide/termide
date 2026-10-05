//! Turning text into search terms, the same way for documents and queries:
//! words split at `_` and camelCase humps, lowercased and stemmed (Snowball,
//! English or Russian by script), so `split_command_line`, `splitCommandLine`
//! and "split the command line" meet, and so do "сессия" and "сессии".

use std::sync::OnceLock;

use rust_stemmers::{Algorithm, Stemmer};

/// Longer runs (hashes, base64, minified code) are not words anyone searches.
const MAX_WORD_CHARS: usize = 64;

/// Words too common to narrow a substring scan of git or code; ranking needs
/// no list, as their weight there is low anyway.
const STOP_WORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "from",
    "into",
    "what",
    "why",
    "how",
    "when",
    "was",
    "were",
    "are",
    "not",
    "but",
    "did",
    "does",
    "have",
    "has",
    "had",
    "use",
    "used",
    "about",
    "which",
    "where",
    "there",
    "their",
    "then",
    "than",
    "can",
    "will",
    "its",
    "как",
    "что",
    "это",
    "для",
    "или",
    "при",
    "так",
    "там",
    "тут",
    "его",
    "она",
    "они",
    "оно",
    "был",
    "была",
    "были",
    "было",
    "почему",
    "зачем",
    "когда",
    "где",
    "чем",
    "который",
    "которая",
    "которые",
    "если",
    "уже",
    "ещё",
    "еще",
    "мы",
    "вы",
    "нас",
    "вас",
    "над",
    "под",
    "без",
    "про",
    "через",
];

struct Stemmers {
    english: Stemmer,
    russian: Stemmer,
}

fn stemmers() -> &'static Stemmers {
    static STEMMERS: OnceLock<Stemmers> = OnceLock::new();
    STEMMERS.get_or_init(|| Stemmers {
        english: Stemmer::create(Algorithm::English),
        russian: Stemmer::create(Algorithm::Russian),
    })
}

/// A lowercase word reduced to its stem: Russian for a word with Cyrillic
/// letters, English for an ASCII one, anything else as it is.
fn stem(word: &str) -> String {
    if word.chars().any(|c| matches!(c, 'а'..='я' | 'ё')) {
        stemmers().russian.stem(word).into_owned()
    } else if word.chars().all(|c| c.is_ascii_alphabetic()) {
        stemmers().english.stem(word).into_owned()
    } else {
        word.to_string()
    }
}

/// Russian case and number endings, longest first, for [`light_stem`].
const RUSSIAN_ENDINGS: &[&str] = &[
    "иями", "ями", "ами", "иях", "ией", "ием", "ого", "его", "ому", "ему", "ыми", "ими", "ой",
    "ей", "ом", "ем", "ам", "ям", "ах", "ях", "ов", "ев", "ую", "юю", "ая", "яя", "ое", "ее", "ые",
    "ие", "ый", "ий", "а", "я", "у", "ю", "е", "и", "ы", "о", "ь", "й",
];

/// A Russian word without its case ending, nothing else: Snowball also
/// strips verb endings, which cuts a name it does not know differently by
/// case — "сомбала" (taken for a verb, like "сделала") becomes "сомба",
/// "сомбалу" becomes "сомбал". This stem agrees for both.
fn light_stem(word: &str) -> Option<String> {
    let ending = RUSSIAN_ENDINGS
        .iter()
        .find(|ending| word.ends_with(*ending))?;
    let stem = &word[..word.len() - ending.len()];
    (stem.chars().count() >= 3).then(|| stem.to_string())
}

/// The forms a lowercase word is indexed and matched under: its Snowball
/// stem, and for a Russian word also its light stem when that differs.
fn variants(word: &str) -> Vec<String> {
    let stemmed = stem(word);
    let mut out = vec![stemmed];
    if word.chars().any(|c| matches!(c, 'а'..='я' | 'ё')) {
        if let Some(light) = light_stem(word) {
            if light != out[0] {
                out.push(light);
            }
        }
    }
    out
}

/// The words of `text`: runs of letters, digits and `_`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty() && word.chars().count() <= MAX_WORD_CHARS)
}

/// `word` cut at `_` and at camelCase humps (`HTTPServer` → `HTTP`, `Server`).
fn parts(word: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    for piece in word.split('_').filter(|piece| !piece.is_empty()) {
        let chars: Vec<(usize, char)> = piece.char_indices().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let (at, c) = chars[i];
            let prev = chars[i - 1].1;
            let next_lower = chars.get(i + 1).is_some_and(|(_, n)| n.is_lowercase());
            let hump = (prev.is_lowercase() && c.is_uppercase())
                || (prev.is_uppercase() && c.is_uppercase() && next_lower);
            if hump {
                parts.push(&piece[start..at]);
                start = at;
            }
        }
        parts.push(&piece[start..]);
    }
    parts
}

/// The search terms of `text`, in order: every part of every word,
/// lowercased, in each of its [`variants`], parts shorter than two
/// characters left out. A word made of several parts also yields itself
/// whole and lowercased, so a match on the exact identifier counts for more
/// than one on its parts.
#[must_use]
pub fn terms(text: &str) -> Vec<String> {
    query_words(text).into_iter().flatten().collect()
}

/// The words of a query, each as the forms it may match in (see
/// [`variants`]): a document has the word when it has any of them.
#[must_use]
pub fn query_words(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for word in words(text) {
        let pieces = parts(word);
        for piece in &pieces {
            let lower = piece.to_lowercase();
            if lower.chars().count() >= 2 {
                out.push(variants(&lower));
            }
        }
        if pieces.len() > 1 {
            out.push(vec![word.to_lowercase()]);
        }
    }
    out
}

/// What a substring scan (git's `--grep`, the code search) looks for: the
/// stems of the queries' words, at least three characters and no stop
/// words, deduplicated. A stem is a prefix of its inflected forms, so
/// "сесс" finds "сессии" and "compact" finds "compaction".
#[must_use]
pub fn scan_patterns(queries: &[String]) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    for query in queries {
        for word in words(query) {
            for piece in parts(word) {
                let lower = piece.to_lowercase();
                if lower.chars().count() < 3 || STOP_WORDS.contains(&lower.as_str()) {
                    continue;
                }
                // The shortest form finds the most inflections as a substring.
                let pattern = variants(&lower)
                    .into_iter()
                    .filter(|form| form.chars().count() >= 3)
                    .min_by_key(|form| form.chars().count())
                    .unwrap_or(lower);
                if !patterns.contains(&pattern) {
                    patterns.push(pattern);
                }
            }
        }
    }
    patterns
}

/// The queries' two-character acronyms, as typed in capitals ("CI", "UI",
/// "ИИ"), lowercased: too short for a substring scan ("ci" is inside
/// "decision"), they are looked for as whole words instead.
#[must_use]
pub fn acronyms(queries: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for query in queries {
        for word in words(query) {
            for piece in parts(word) {
                let short = piece.chars().count() == 2
                    && piece.chars().all(char::is_alphanumeric)
                    && piece.chars().any(char::is_uppercase)
                    && !piece.chars().any(char::is_lowercase);
                let lower = piece.to_lowercase();
                if short && !found.contains(&lower) {
                    found.push(lower);
                }
            }
        }
    }
    found
}

/// The queries' identifiers: words with `_`, `::` neighbours or camelCase
/// humps, as typed, for git's pickaxe (`-S`), which matches them literally.
#[must_use]
pub fn identifiers(queries: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for query in queries {
        for word in words(query) {
            if parts(word).len() > 1 && !found.iter().any(|f| f == word) {
                found.push(word.to_string());
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acronyms_are_two_capitals_as_typed() {
        let queries = vec!["why CI fails in the UI, ИИ too, we go".to_string()];
        assert_eq!(acronyms(&queries), ["ci", "ui", "ии"]);
        assert!(acronyms(&["Go on".to_string()]).is_empty());
    }

    #[test]
    fn identifiers_split_and_also_count_whole() {
        assert_eq!(
            terms("split_command_line"),
            ["split", "command", "line", "split_command_line"]
        );
        assert_eq!(terms("HTTPServer"), ["http", "server", "httpserver"]);
        assert_eq!(terms("readBranch"), ["read", "branch", "readbranch"]);
        assert_eq!(terms("a b"), Vec::<String>::new());
    }

    #[test]
    fn inflected_forms_share_a_stem_in_both_languages() {
        assert_eq!(terms("сессия"), terms("сессии"));
        assert_eq!(terms("Сессий"), terms("сессиями"));
        assert_eq!(terms("compaction"), terms("compactions"));
        assert_eq!(terms("searching"), terms("searched"));
    }

    #[test]
    fn a_name_the_stemmer_does_not_know_matches_in_every_case() {
        let forms = ["сомбала", "сомбалу", "сомбале", "сомбалы", "сомбалой"];
        for a in forms {
            for b in forms {
                let doc = terms(a);
                assert!(
                    query_words(b)[0].iter().any(|form| doc.contains(form)),
                    "{b} should find {a}: {doc:?} vs {:?}",
                    query_words(b)
                );
            }
        }
        let patterns = scan_patterns(&["сомбала".to_string()]);
        assert!("сомбалу".contains(patterns[0].as_str()), "{patterns:?}");
    }

    #[test]
    fn scan_patterns_drop_short_and_common_words() {
        let patterns = scan_patterns(&["почему отказались от tokio".to_string()]);
        assert!(patterns.contains(&"tokio".to_string()));
        assert!(!patterns.iter().any(|p| p == "от" || p == "почему"));
        let patterns = scan_patterns(&["why the compaction".to_string()]);
        assert_eq!(patterns, ["compact"]);
    }

    #[test]
    fn identifiers_are_kept_as_typed() {
        assert_eq!(
            identifiers(&["where is read_branch and AgentSpec used".to_string()]),
            ["read_branch", "AgentSpec"]
        );
    }
}
