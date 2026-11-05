//! Reading a delimited text file into records, and guessing how it is laid
//! out: which delimiter separates the fields, and whether the first record
//! names the columns.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::Path;

use csv::{ReaderBuilder, StringRecord};

/// Delimiters the viewer recognises, in the order the switch cycles them.
pub const DELIMITERS: [u8; 4] = [b',', b';', b'\t', b'|'];

/// Records read from the sample when guessing the delimiter.
const SNIFF_RECORDS: usize = 50;
/// Bytes read from the start of the file when guessing the delimiter.
const SNIFF_BYTES: usize = 64 * 1024;

/// A file read into memory, every record as it stands — the first one too,
/// whether or not it names the columns.
#[derive(Debug, Default)]
pub struct Table {
    pub records: Vec<StringRecord>,
    pub delimiter: u8,
    /// Whether the first record looks like column names.
    pub header_guess: bool,
    /// Fields in the widest record.
    pub width: usize,
}

/// Read and parse `path`. `delimiter` overrides the guess.
pub fn load(path: &Path, delimiter: Option<u8>) -> Result<Table, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    parse(
        &bytes,
        delimiter.unwrap_or_else(|| guess_delimiter(path, &bytes)),
    )
}

/// Parse `bytes` with `delimiter`. Text that is not UTF-8 is read lossily
/// rather than refused: a stray byte should not hide the whole table.
pub fn parse(bytes: &[u8], delimiter: u8) -> Result<Table, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    let mut reader = reader(delimiter, text.as_bytes());
    let mut records = Vec::new();
    for record in reader.records() {
        records.push(record.map_err(|e| e.to_string())?);
    }
    let width = records.iter().map(StringRecord::len).max().unwrap_or(0);
    let header_guess = guess_header(&records);
    Ok(Table {
        records,
        delimiter,
        header_guess,
        width,
    })
}

fn reader(delimiter: u8, bytes: &[u8]) -> csv::Reader<&[u8]> {
    ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        // Rows of different lengths are common in hand-made files; show them
        // rather than stop at the first one.
        .flexible(true)
        .from_reader(bytes)
}

/// The delimiter of a file: tab for `.tsv`/`.tab`; otherwise the candidate
/// that splits the first records into the same number of fields, more than
/// one, most often — and among equals, into the most fields.
pub fn guess_delimiter(path: &Path, bytes: &[u8]) -> u8 {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("tsv" | "tab")) {
        return b'\t';
    }
    let sample = &bytes[..bytes.len().min(SNIFF_BYTES)];
    // Cut at the last line break so a half-read record does not count.
    let sample = match sample.iter().rposition(|&b| b == b'\n') {
        Some(end) if sample.len() < bytes.len() => &sample[..end],
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
fn guess_header(records: &[StringRecord]) -> bool {
    let Some(first) = records.first() else {
        return false;
    };
    first
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

/// Order two fields for sorting: numbers by value before text, text by its
/// characters, and empty fields last.
pub fn compare_fields(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.total_cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        _ => a.cmp(b),
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
        guess_delimiter(Path::new(name), text.as_bytes())
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
    fn quoted_fields_keep_their_delimiters_and_line_breaks() {
        let table = parse(b"a,b\n\"x, y\",\"two\nlines\"\n", b',').unwrap();
        assert_eq!(table.records.len(), 2);
        assert_eq!(&table.records[1][0], "x, y");
        assert_eq!(&table.records[1][1], "two\nlines");
        assert_eq!(one_line(&table.records[1][1]), "two↵lines");
    }

    #[test]
    fn ragged_rows_and_a_bom_are_read() {
        let table = parse(b"\xEF\xBB\xBFa,b\n1\n1,2,3\n", b',').unwrap();
        assert_eq!(&table.records[0][0], "a");
        assert_eq!(table.width, 3);
        assert_eq!(table.records[1].len(), 1);
    }

    #[test]
    fn invalid_utf8_is_read_lossily() {
        let table = parse(b"a,b\n\xFF,2\n", b',').unwrap();
        assert_eq!(&table.records[1][0], "\u{FFFD}");
    }

    #[test]
    fn a_first_record_of_names_is_a_header() {
        assert!(parse(b"id,name\n1,x\n", b',').unwrap().header_guess);
        assert!(!parse(b"1,x\n2,y\n", b',').unwrap().header_guess);
        assert!(!parse(b"id,\n1,x\n", b',').unwrap().header_guess);
        assert!(!parse(b"", b',').unwrap().header_guess);
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
        let mut v = vec!["10", "", "b", "9", "a", "-1.5"];
        v.sort_by(|a, b| compare_fields(a, b));
        assert_eq!(v, vec!["-1.5", "9", "10", "a", "b", ""]);
    }

    #[test]
    fn the_delimiter_switch_cycles() {
        assert_eq!(next_delimiter(b','), b';');
        assert_eq!(next_delimiter(b'|'), b',');
        assert_eq!(delimiter_label(b'\t'), "Tab");
    }
}
