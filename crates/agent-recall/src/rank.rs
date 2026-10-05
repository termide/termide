//! Ranking: BM25 within a source, a recency weight for what has a date, and
//! reciprocal rank fusion across sources, whose scores are not comparable.

use std::collections::HashMap;

/// BM25's term-frequency saturation and length normalisation, the usual
/// values.
const K1: f64 = 1.2;
const B: f64 = 0.75;
/// A document must cover at least one in this many parts of a phrasing's
/// weight.
const MIN_COVERAGE_DIVISOR: usize = 3;
/// Reciprocal rank fusion's damping constant, as in the original paper.
const RRF_K: f64 = 60.0;
/// Days over which the recency weight halves its way down to the floor.
const RECENCY_DAYS: f64 = 180.0;

/// Interned terms: documents keep ids, not strings.
#[derive(Debug, Default)]
pub struct Vocabulary(HashMap<String, u32>);

impl Vocabulary {
    fn intern(&mut self, term: String) -> u32 {
        let next = u32::try_from(self.0.len()).unwrap_or(u32::MAX);
        *self.0.entry(term).or_insert(next)
    }

    /// One query phrasing, given as its words with the forms each may match
    /// in: the ids of the forms this vocabulary knows, word by word, and how
    /// many words have no known form at all.
    #[must_use]
    pub fn phrase(&self, words: &[Vec<String>]) -> Phrase {
        let mut groups: Vec<Vec<u32>> = Vec::new();
        let mut unknown = 0;
        for forms in words {
            let mut ids: Vec<u32> = forms
                .iter()
                .filter_map(|f| self.0.get(f).copied())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            if ids.is_empty() {
                unknown += 1;
            } else if !groups.contains(&ids) {
                groups.push(ids);
            }
        }
        Phrase { groups, unknown }
    }

    /// Every phrasing of a query, see [`Vocabulary::phrase`].
    #[must_use]
    pub fn phrases(&self, phrasings: &[Vec<Vec<String>>]) -> Vec<Phrase> {
        phrasings
            .iter()
            .map(|words| self.phrase(words))
            .filter(|phrase| !phrase.groups.is_empty())
            .collect()
    }
}

/// One phrasing of a query, as term ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phrase {
    /// One group per word: the ids of the forms it may match in.
    pub groups: Vec<Vec<u32>>,
    /// Words of the phrasing no document has: the rarest there are, so a
    /// document lacking them covers the phrasing less.
    pub unknown: usize,
}

/// One document's terms: `(id, count)` sorted by id, and its length.
#[derive(Debug, Clone, Default)]
pub struct TermBag {
    terms: Vec<(u32, u32)>,
    len: u32,
}

impl TermBag {
    /// The bag of `terms`, interned into `vocabulary`.
    #[must_use]
    pub fn new(vocabulary: &mut Vocabulary, terms: Vec<String>) -> Self {
        let len = u32::try_from(terms.len()).unwrap_or(u32::MAX);
        let mut ids: Vec<u32> = terms.into_iter().map(|t| vocabulary.intern(t)).collect();
        ids.sort_unstable();
        let mut bag: Vec<(u32, u32)> = Vec::new();
        for id in ids {
            match bag.last_mut() {
                Some((last, count)) if *last == id => *count += 1,
                _ => bag.push((id, 1)),
            }
        }
        Self { terms: bag, len }
    }

    fn count(&self, id: u32) -> u32 {
        self.terms
            .binary_search_by_key(&id, |(term, _)| *term)
            .map_or(0, |i| self.terms[i].1)
    }
}

/// The BM25 score of every document for a query given as several
/// phrasings, in order: each phrasing is scored on its own and a document
/// keeps its best, since phrasings are alternatives, not one long query.
///
/// A phrasing's score is scaled by how much of it the document covers,
/// weighted by IDF (Lucene's coordination factor, by weight rather than by
/// count): a document with the one rare word of a phrasing covers most of
/// it, one with only its common words covers little. Under a third scores
/// nothing — that document matched incidental words. A term no document has
/// counts as the rarest possible, so it still weighs in the share.
#[must_use]
pub fn bm25(docs: &[&TermBag], phrases: &[Phrase]) -> Vec<f64> {
    if docs.is_empty() || phrases.is_empty() {
        return vec![0.0; docs.len()];
    }
    let n = docs.len() as f64;
    let average = docs.iter().map(|d| f64::from(d.len)).sum::<f64>() / n;
    let average = average.max(1.0);
    let idf_of = |df: f64| (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
    // A word's count in a document is its likeliest form's: the forms of
    // one occurrence are not several occurrences.
    let count =
        |doc: &TermBag, group: &[u32]| group.iter().map(|id| doc.count(*id)).max().unwrap_or(0);
    let mut idf: HashMap<&[u32], f64> = HashMap::new();
    for phrase in phrases {
        for group in &phrase.groups {
            idf.entry(group.as_slice()).or_insert_with(|| {
                idf_of(docs.iter().filter(|d| count(d, group) > 0).count() as f64)
            });
        }
    }
    let unknown_idf = idf_of(0.0);
    docs.iter()
        .map(|doc| {
            let norm = K1 * (1.0 - B + B * f64::from(doc.len) / average);
            phrases
                .iter()
                .map(|phrase| {
                    let mut score = 0.0;
                    let mut covered = 0.0;
                    let mut total = unknown_idf * phrase.unknown as f64;
                    for group in &phrase.groups {
                        let weight = idf[group.as_slice()];
                        total += weight;
                        let tf = f64::from(count(doc, group));
                        if tf > 0.0 {
                            covered += weight;
                            score += weight * tf * (K1 + 1.0) / (tf + norm);
                        }
                    }
                    let share = if total > 0.0 { covered / total } else { 0.0 };
                    if share * (MIN_COVERAGE_DIVISOR as f64) < 1.0 {
                        0.0
                    } else {
                        score * share
                    }
                })
                .fold(0.0, f64::max)
        })
        .collect()
}

/// A weight in `(0.5, 1]` that favours what was written recently: a hit
/// from today keeps its score, one from long ago keeps half of it.
#[must_use]
pub fn recency(timestamp_ms: u64, now_ms: u64) -> f64 {
    let age_days = now_ms.saturating_sub(timestamp_ms) as f64 / 86_400_000.0;
    0.5 + 0.5 * (-age_days / RECENCY_DAYS).exp()
}

/// Where a hit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Session,
    Git,
    File,
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub source: Source,
    /// What `open` takes: `session:<id>#<entry>`, `commit:<repo>@<sha>`,
    /// `file:<path>:<line>`.
    pub reference: String,
    pub timestamp: Option<u64>,
    /// A short label: the kind of session entry, a commit subject, a path.
    pub label: String,
    pub snippet: String,
    /// A related reference, such as the commit a session's call made.
    pub related: Option<String>,
    pub score: f64,
}

/// Merge per-source lists, each best first, by reciprocal rank fusion and
/// keep the best `limit`; a reference listed twice counts once, at its best.
#[must_use]
pub fn fuse(lists: Vec<Vec<Hit>>, limit: usize) -> Vec<Hit> {
    let mut fused: Vec<Hit> = Vec::new();
    for list in lists {
        for (rank, mut hit) in list.into_iter().enumerate() {
            hit.score = 1.0 / (RRF_K + rank as f64 + 1.0);
            match fused.iter_mut().find(|h| h.reference == hit.reference) {
                Some(existing) => existing.score = existing.score.max(hit.score),
                None => fused.push(hit),
            }
        }
    }
    fused.sort_by(|a, b| b.score.total_cmp(&a.score));
    fused.truncate(limit);
    fused
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{query_words, terms};

    #[test]
    fn bm25_prefers_the_document_with_the_rarer_term() {
        let mut vocabulary = Vocabulary::default();
        let docs: Vec<TermBag> = [
            "the compaction keeps the tail verbatim",
            "the panel draws the transcript",
            "the panel and the compaction prompt",
        ]
        .iter()
        .map(|text| TermBag::new(&mut vocabulary, terms(text)))
        .collect();
        let refs: Vec<&TermBag> = docs.iter().collect();
        let query = vocabulary.phrases(&[query_words("compaction tail")]);
        let scores = bm25(&refs, &query);
        assert!(scores[0] > 0.0);
        // The third has only the commoner word, a small share of the query's
        // weight, and the second has neither.
        assert_eq!(scores[2], 0.0);
        assert_eq!(scores[1], 0.0);
        assert!(vocabulary.phrases(&[query_words("nowhere")]).is_empty());
    }

    #[test]
    fn a_rare_word_outweighs_common_ones_and_phrasings_are_alternatives() {
        let mut vocabulary = Vocabulary::default();
        let mut texts = vec!["the sombala app idea for altered states"];
        texts.extend(std::iter::repeat_n(
            "there is no sense to develop the project",
            8,
        ));
        texts.push("no sense in the project plan");
        let docs: Vec<TermBag> = texts
            .iter()
            .map(|text| TermBag::new(&mut vocabulary, terms(text)))
            .collect();
        let refs: Vec<&TermBag> = docs.iter().collect();
        // One phrasing: the rare word carries the query.
        let one = vocabulary.phrases(&[query_words("sombala develop no sense")]);
        let scores = bm25(&refs, &one);
        assert!(
            scores[0] > 0.0,
            "the rare word alone covers enough: {scores:?}"
        );
        assert!(scores[0] > scores[9]);
        // Several phrasings do not dilute each other.
        let many = vocabulary.phrases(&[
            query_words("sombala develop no sense"),
            query_words("sombala"),
            query_words("sombala project"),
        ]);
        let scores = bm25(&refs, &many);
        assert!(scores[0] > scores[1] && scores[0] > scores[9]);
        // A word no document has keeps common-word matches from passing.
        let missing = vocabulary.phrases(&[query_words("zanzibar develop sense")]);
        assert_eq!(missing[0].unknown, 1);
        assert!(bm25(&refs, &missing).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn recency_halves_towards_the_floor() {
        let day = 86_400_000;
        assert!((recency(10 * day, 10 * day) - 1.0).abs() < 1e-9);
        assert!(recency(0, 3650 * day) < 0.51);
        assert!(recency(0, 30 * day) > recency(0, 300 * day));
    }

    #[test]
    fn fusion_interleaves_sources_and_dedups_references() {
        let hit = |reference: &str, source| Hit {
            source,
            reference: reference.to_string(),
            timestamp: None,
            label: String::new(),
            snippet: String::new(),
            related: None,
            score: 0.0,
        };
        let fused = fuse(
            vec![
                vec![hit("s1", Source::Session), hit("s2", Source::Session)],
                vec![hit("g1", Source::Git), hit("s1", Source::Git)],
            ],
            10,
        );
        let order: Vec<&str> = fused.iter().map(|h| h.reference.as_str()).collect();
        assert_eq!(order.len(), 3);
        assert_eq!(&order[..2], ["s1", "g1"]);
        assert_eq!(fuse(vec![vec![hit("a", Source::File)]], 0).len(), 0);
    }
}
