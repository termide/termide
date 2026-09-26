//! Matching for every list that filters as the user types, on nucleo.
//!
//! A [`Query`] scores a candidate, so lists can rank by how well it matched,
//! and reports which graphemes matched, so a row can highlight them. Fuzzy
//! queries take fzf's syntax: space-separated words that must all match,
//! `'exact`, `^prefix`, `suffix$` and `!negated`, lower case matching either
//! case.

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

enum Needle {
    Pattern(Pattern),
    Atom(Atom),
}

pub struct Query {
    needle: Needle,
    empty: bool,
    matcher: Matcher,
    buf: Vec<char>,
}

impl Query {
    /// A fuzzy query in fzf syntax, for labels and names.
    #[must_use]
    pub fn fuzzy(query: &str) -> Self {
        Self::pattern(query, Config::DEFAULT)
    }

    /// A fuzzy query for paths: matches at the start of a path segment score
    /// higher, so `lib` ranks `src/lib.rs` above `src/calibrate.rs`.
    #[must_use]
    pub fn fuzzy_path(query: &str) -> Self {
        Self::pattern(query, Config::DEFAULT.match_paths())
    }

    /// The whole query as one contiguous needle, spaces included, and no
    /// accent folding: a plain "contains".
    #[must_use]
    pub fn substring(query: &str, case_sensitive: bool) -> Self {
        let case = if case_sensitive {
            CaseMatching::Respect
        } else {
            CaseMatching::Ignore
        };
        Self {
            needle: Needle::Atom(Atom::new(
                query,
                case,
                Normalization::Never,
                AtomKind::Substring,
                false,
            )),
            empty: query.is_empty(),
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }

    fn pattern(query: &str, config: Config) -> Self {
        Self {
            needle: Needle::Pattern(Pattern::parse(
                query,
                CaseMatching::Smart,
                Normalization::Smart,
            )),
            empty: query.trim().is_empty(),
            matcher: Matcher::new(config),
            buf: Vec::new(),
        }
    }

    /// Whether the query matches everything (nothing typed yet).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.empty
    }

    /// The score of `text`, higher is better; `None` when it does not match.
    /// An empty query matches everything with score 0.
    pub fn score(&mut self, text: &str) -> Option<u32> {
        if self.empty {
            return Some(0);
        }
        let haystack = Utf32Str::new(text, &mut self.buf);
        match &self.needle {
            Needle::Pattern(pattern) => pattern.score(haystack, &mut self.matcher),
            Needle::Atom(atom) => atom.score(haystack, &mut self.matcher).map(u32::from),
        }
    }

    /// The best score among `texts`, for a candidate matched on several
    /// fields.
    pub fn score_any(&mut self, texts: &[&str]) -> Option<u32> {
        texts.iter().filter_map(|text| self.score(text)).max()
    }

    /// The matched grapheme indices of `text`, sorted; `None` when it does
    /// not match, empty for an empty query.
    pub fn positions(&mut self, text: &str) -> Option<Vec<usize>> {
        if self.empty {
            return Some(Vec::new());
        }
        let haystack = Utf32Str::new(text, &mut self.buf);
        let mut indices = Vec::new();
        match &self.needle {
            Needle::Pattern(pattern) => pattern
                .indices(haystack, &mut self.matcher, &mut indices)
                .map(|_| ()),
            Needle::Atom(atom) => atom
                .indices(haystack, &mut self.matcher, &mut indices)
                .map(|_| ()),
        }?;
        indices.sort_unstable();
        indices.dedup();
        Some(indices.into_iter().map(|i| i as usize).collect())
    }
}

/// The indices of the candidates that matched, best score first; equal
/// scores keep their original order, so an empty query keeps the list as is.
pub fn rank(scores: impl IntoIterator<Item = Option<u32>>) -> Vec<usize> {
    let mut hits: Vec<(usize, u32)> = scores
        .into_iter()
        .enumerate()
        .filter_map(|(index, score)| score.map(|score| (index, score)))
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    hits.into_iter().map(|(index, _)| index).collect()
}

/// `text` as spans, the graphemes at `positions` (from [`Query::positions`])
/// in `hit`, the rest in `base`.
#[must_use]
pub fn highlight(text: &str, positions: &[usize], base: Style, hit: Style) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_is_hit = false;
    let mut next = positions.iter().peekable();
    for (index, grapheme) in text.graphemes(true).enumerate() {
        let is_hit = next.next_if(|&&p| p == index).is_some();
        if is_hit != run_is_hit && !run.is_empty() {
            let style = if run_is_hit { hit } else { base };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_is_hit = is_hit;
        run.push_str(grapheme);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_is_hit { hit } else { base }));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn fuzzy_matches_a_subsequence_and_ranks_the_tighter_match_first() {
        let names = ["HomepageProvider.php", "hp.txt", "Homepage.php", "other.rs"];
        let mut query = Query::fuzzy("hpprov");
        let ranked = rank(names.iter().map(|n| query.score(n)));
        assert_eq!(ranked, vec![0]);

        let mut query = Query::fuzzy("home");
        let ranked = rank(names.iter().map(|n| query.score(n)));
        assert_eq!(ranked.len(), 2);
        assert!(!ranked.contains(&3));
    }

    #[test]
    fn empty_query_keeps_every_candidate_in_order() {
        let mut query = Query::fuzzy("  ");
        assert!(query.is_empty());
        assert_eq!(
            rank(["b", "a", "c"].iter().map(|n| query.score(n))),
            [0, 1, 2]
        );
        assert_eq!(query.positions("abc"), Some(vec![]));
    }

    #[test]
    fn fzf_syntax_and_smart_case() {
        let mut query = Query::fuzzy("^src !test");
        assert!(query.score("src/lib.rs").is_some());
        assert!(query.score("src/test.rs").is_none());
        assert!(query.score("lib/src.rs").is_none());

        assert!(Query::fuzzy("readme").score("README.md").is_some());
        assert!(Query::fuzzy("README").score("readme.md").is_none());
    }

    #[test]
    fn substring_is_contiguous_and_honours_case() {
        let mut query = Query::substring("a b", false);
        assert!(query.score("x A B y").is_some());
        assert!(query.score("a_b").is_none());
        assert!(Query::substring("Main", true).score("main.rs").is_none());
        assert!(Query::substring("main", false).score("MAIN.rs").is_some());
        assert!(Query::substring("", false).score("anything").is_some());
    }

    #[test]
    fn path_matching_prefers_segment_starts() {
        let paths = ["src/calibrate.rs", "src/lib.rs"];
        let mut query = Query::fuzzy_path("lib");
        let ranked = rank(paths.iter().map(|p| query.score(p)));
        assert_eq!(ranked, [1, 0]);
    }

    #[test]
    fn positions_count_graphemes_and_highlight_splits_runs() {
        let mut query = Query::fuzzy("ab");
        assert_eq!(query.positions("яab"), Some(vec![1, 2]));
        assert_eq!(query.positions("äb"), Some(vec![0, 1]), "a matches ä");
        assert_eq!(query.positions("xyz"), None);

        let base = Style::default();
        let hit = Style::default().add_modifier(Modifier::BOLD);
        let spans = highlight("äabc", &[1, 2], base, hit);
        let parts: Vec<(&str, bool)> = spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style == hit))
            .collect();
        assert_eq!(parts, [("ä", false), ("ab", true), ("c", false)]);
        assert_eq!(highlight("abc", &[], base, hit).len(), 1);
    }
}
