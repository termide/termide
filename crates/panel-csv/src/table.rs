//! How a delimited text file is laid out — which delimiter separates the
//! fields, whether the first record names the columns — and how its fields
//! show and sort.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::Path;

use csv::{ReaderBuilder, StringRecord};

/// Delimiters the viewer recognises, in the order the switch cycles them.
pub const DELIMITERS: [u8; 4] = [b',', b';', b'\t', b'|'];

/// Records read from the sample when guessing the delimiter.
const SNIFF_RECORDS: usize = 50;
/// Bytes read from the start of the file when guessing the delimiter.
pub const SNIFF_BYTES: usize = 64 * 1024;

/// A reader of `source` split by `delimiter`, every record taken as data.
pub fn reader<R: std::io::Read>(delimiter: u8, source: R) -> csv::Reader<R> {
    ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        // Rows of different lengths are common in hand-made files; show them
        // rather than stop at the first one.
        .flexible(true)
        .from_reader(source)
}

/// The delimiter of a file: tab for `.tsv`/`.tab`; otherwise the candidate
/// that splits the first records into the same number of fields, more than
/// one, most often — and among equals, into the most fields. `sample` is the
/// start of the file, `whole` whether it is all of it.
pub fn guess_delimiter(path: &Path, sample: &[u8], whole: bool) -> u8 {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("tsv" | "tab")) {
        return b'\t';
    }
    let sample = &sample[..sample.len().min(SNIFF_BYTES)];
    // Cut at the last line break so a half-read record does not count.
    let sample = match sample.iter().rposition(|&b| b == b'\n') {
        Some(end) if !whole => &sample[..end],
        _ => sample,
    };
    let mut best = (b',', 0usize, 0usize);
    for &delimiter in &DELIMITERS {
        let counts: Vec<usize> = reader(delimiter, sample)
            .records()
            .take(SNIFF_RECORDS)
            .filter_map(Result::ok)
            .map(|r| r.len())
            .collect();
        let Some((fields, agreeing)) = mode(&counts) else {
            continue;
        };
        if fields > 1 && (agreeing, fields) > (best.1, best.2) {
            best = (delimiter, agreeing, fields);
        }
    }
    best.0
}

/// The most frequent value and how often it occurs; the larger on a tie.
fn mode(values: &[usize]) -> Option<(usize, usize)> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mut best: Option<(usize, usize)> = None;
    for chunk in sorted.chunk_by(|a, b| a == b) {
        let candidate = (chunk[0], chunk.len());
        if best.is_none_or(|(_, n)| candidate.1 >= n) {
            best = Some(candidate);
        }
    }
    best
}

/// Column names are text: a first record with an empty or numeric field is
/// taken for data.
pub fn guess_header(first: &StringRecord) -> bool {
    !first.is_empty()
        && first
            .iter()
            .all(|f| !f.trim().is_empty() && f.trim().parse::<f64>().is_err())
}

/// Spreadsheet-style name of column `index`: A … Z, AA, AB, ….
pub fn column_letter(mut index: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

/// Names for `width` columns: the header's fields where there is one and it
/// is not blank, the column's letter elsewhere.
pub fn column_names(header: Option<&StringRecord>, width: usize) -> Vec<String> {
    (0..width)
        .map(|i| match header.and_then(|h| h.get(i)) {
            Some(name) if !name.trim().is_empty() => one_line(name).into_owned(),
            _ => column_letter(i),
        })
        .collect()
}

/// A field as one line of text: line breaks become `↵`, other control
/// characters a space, so a cell never spills out of its row.
pub fn one_line(field: &str) -> Cow<'_, str> {
    if !field.chars().any(char::is_control) {
        return Cow::Borrowed(field);
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' => out.push('↵'),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// A field as it sorts: numbers by value before text, text by its
/// characters, and empty fields last. Worked out once per row, so a sort of
/// millions does not parse each field at every comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum SortKey {
    Number(f64),
    Text(Box<str>),
    Empty,
}

impl SortKey {
    pub fn of(field: &str) -> Self {
        let field = field.trim();
        if field.is_empty() {
            Self::Empty
        } else if let Ok(n) = field.parse::<f64>() {
            Self::Number(n)
        } else {
            Self::Text(field.into())
        }
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.total_cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::Number(_) => 0,
            Self::Text(_) => 1,
            Self::Empty => 2,
        }
    }
}

/// The printable name of a delimiter, for the status bar.
pub fn delimiter_label(delimiter: u8) -> &'static str {
    match delimiter {
        b',' => ",",
        b';' => ";",
        b'\t' => "Tab",
        b'|' => "|",
        _ => "?",
    }
}

/// The delimiter after `delimiter` in [`DELIMITERS`], wrapping around.
pub fn next_delimiter(delimiter: u8) -> u8 {
    let at = DELIMITERS.iter().position(|&d| d == delimiter);
    DELIMITERS[at.map_or(0, |i| (i + 1) % DELIMITERS.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guess(name: &str, text: &str) -> u8 {
        guess_delimiter(Path::new(name), text.as_bytes(), true)
    }

    #[test]
    fn the_delimiter_is_the_one_that_splits_records_evenly() {
        assert_eq!(guess("a.csv", "a,b,c\n1,2,3\n4,5,6\n"), b',');
        // Excel in many locales: semicolons, with decimal commas inside.
        assert_eq!(guess("a.csv", "name;price\nx;1,5\ny;2,75\n"), b';');
        assert_eq!(guess("a.txt", "a\tb\n1\t2\n"), b'\t');
        assert_eq!(guess("a.csv", "a|b|c\n1|2|3\n"), b'|');
        // A comma inside quotes does not split the field.
        assert_eq!(guess("a.csv", "a;b\n\"x, y\";1\n\"z, w\";2\n"), b';');
        // By extension, whatever the content.
        assert_eq!(guess("a.tsv", "a,b\n1,2\n"), b'\t');
        // One column: nothing to choose, comma.
        assert_eq!(guess("a.csv", "one\ntwo\n"), b',');
    }

    #[test]
    fn a_cut_sample_drops_its_half_read_record() {
        // Cut inside the third record: whole, `,` would split it unevenly.
        let sample = b"a;b\n1;2\nx,y,z,";
        assert_eq!(guess_delimiter(Path::new("a.csv"), sample, false), b';');
    }

    #[test]
    fn a_first_record_of_names_is_a_header() {
        let header = |fields: Vec<&str>| guess_header(&StringRecord::from(fields));
        assert!(header(vec!["id", "name"]));
        assert!(!header(vec!["1", "x"]));
        assert!(!header(vec!["id", ""]));
        assert!(!header(vec![]));
    }

    #[test]
    fn columns_are_named_by_the_header_or_by_letter() {
        assert_eq!(column_letter(0), "A");
        assert_eq!(column_letter(25), "Z");
        assert_eq!(column_letter(26), "AA");
        assert_eq!(column_letter(27), "AB");
        assert_eq!(column_letter(702), "AAA");
        let header = StringRecord::from(vec!["id", " "]);
        assert_eq!(column_names(Some(&header), 3), vec!["id", "B", "C"]);
        assert_eq!(column_names(None, 2), vec!["A", "B"]);
    }

    #[test]
    fn numbers_sort_by_value_before_text_and_blanks_last() {
        let mut v = vec!["10", "", "b", "9", " a", "-1.5"];
        v.sort_by(|a, b| SortKey::of(a).compare(&SortKey::of(b)));
        assert_eq!(v, vec!["-1.5", "9", "10", " a", "b", ""]);
    }

    #[test]
    fn the_delimiter_switch_cycles() {
        assert_eq!(next_delimiter(b','), b';');
        assert_eq!(next_delimiter(b'|'), b',');
        assert_eq!(delimiter_label(b'\t'), "Tab");
    }
}
