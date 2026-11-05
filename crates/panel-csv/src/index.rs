//! Where the records of a file start, found by one background pass, so the
//! viewer reads from disk only the rows it shows: a file of any size opens
//! at once and costs a few bytes of memory per record kept.
//!
//! The index keeps the byte offset of every [`STRIDE`]-th record. A window
//! is read by seeking to the checkpoint before it and skipping the records
//! up to its first row. A sorted view keeps the offset of every data row in
//! its order instead, built by a pass of its own.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use csv::{ByteRecord, Position, StringRecord};

use crate::table::{self, SortKey, SNIFF_BYTES};

/// Records between two checkpoints of the index.
pub const STRIDE: usize = 128;
/// How often the indexer reports what it has found so far.
const REPORT_EVERY: Duration = Duration::from_millis(100);
/// Records between two checks of the clock or of a cancel flag.
const CHECK_EVERY: usize = 4096;

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// What the indexer found so far of one file.
#[derive(Debug, Default)]
pub struct Index {
    pub delimiter: u8,
    /// Byte offset of records 0, STRIDE, 2·STRIDE, ….
    checkpoints: Vec<u64>,
    /// Records indexed.
    pub records: usize,
    /// Fields in the widest record indexed.
    pub width: usize,
    /// The first record: the column names, if it is a header.
    pub first: Option<StringRecord>,
}

/// A report of the indexer.
#[derive(Debug)]
pub enum Update {
    /// The delimiter the file is read with.
    Start {
        delimiter: u8,
    },
    /// Records found since the last report.
    Found {
        checkpoints: Vec<u64>,
        records: usize,
        width: usize,
        first: Option<StringRecord>,
    },
    /// The whole file is indexed.
    Done,
    Failed(String),
}

impl Index {
    /// Take a report in; false for `Done` and `Failed`, which the caller
    /// handles.
    pub fn apply(&mut self, update: Update) -> bool {
        match update {
            Update::Start { delimiter } => self.delimiter = delimiter,
            Update::Found {
                checkpoints,
                records,
                width,
                first,
            } => {
                self.checkpoints.extend(checkpoints);
                self.records = records;
                self.width = self.width.max(width);
                if self.first.is_none() {
                    self.first = first;
                }
            }
            Update::Done | Update::Failed(_) => return false,
        }
        true
    }

    /// Records `from..from + count` in file order, fewer past the end.
    pub fn read(
        &self,
        path: &Path,
        from: usize,
        count: usize,
    ) -> Result<Vec<StringRecord>, String> {
        let count = count.min(self.records.saturating_sub(from));
        let Some(&offset) = self.checkpoints.get(from / STRIDE) else {
            return Ok(Vec::new());
        };
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut reader = open_at(path, self.delimiter, offset)?;
        let mut record = ByteRecord::new();
        for _ in 0..from % STRIDE {
            if !reader.read_byte_record(&mut record).map_err(err)? {
                return Ok(Vec::new());
            }
        }
        let mut out = Vec::with_capacity(count);
        while out.len() < count && reader.read_byte_record(&mut record).map_err(err)? {
            out.push(lossy(&record));
        }
        Ok(out)
    }
}

/// The records starting at `offsets`, one seek each: the rows of a window
/// of a sorted view.
pub fn read_at(path: &Path, delimiter: u8, offsets: &[u64]) -> Result<Vec<StringRecord>, String> {
    let Some(&first) = offsets.first() else {
        return Ok(Vec::new());
    };
    let mut reader = open_at(path, delimiter, first)?;
    let mut record = ByteRecord::new();
    let mut out = Vec::with_capacity(offsets.len());
    for &offset in offsets {
        reader.seek(position(offset)).map_err(err)?;
        if !reader.read_byte_record(&mut record).map_err(err)? {
            break;
        }
        out.push(lossy(&record));
    }
    Ok(out)
}

/// Index `path` on a background thread; `delimiter` overrides the guess.
/// Dropping the receiver stops it at its next report.
pub fn spawn_index(path: PathBuf, delimiter: Option<u8>) -> std::io::Result<Receiver<Update>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("termide-csv-index".into())
        .spawn(move || {
            if let Err(e) = index_file(&path, delimiter, &tx) {
                let _ = tx.send(Update::Failed(e));
            }
        })?;
    Ok(rx)
}

/// A sort in the background, with the means to give it up.
pub struct SortJob {
    pub result: Receiver<Result<Vec<u64>, String>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for SortJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a sort orders: the data rows of a file by one column.
pub struct SortSpec {
    pub delimiter: u8,
    /// Leave the first record out: it is the header.
    pub skip_first: bool,
    pub column: usize,
    pub descending: bool,
}

/// Sort the data rows of `path` on a background thread into the offsets of
/// their records. Dropping the job stops it.
pub fn spawn_sort(path: PathBuf, spec: SortSpec) -> std::io::Result<SortJob> {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::Builder::new()
        .name("termide-csv-sort".into())
        .spawn(move || {
            if let Some(result) = sort_file(&path, &spec, &flag) {
                let _ = tx.send(result);
            }
        })?;
    Ok(SortJob { result: rx, cancel })
}

fn index_file(path: &Path, delimiter: Option<u8>, tx: &Sender<Update>) -> Result<(), String> {
    let (start, delimiter) = layout(path, delimiter)?;
    if tx.send(Update::Start { delimiter }).is_err() {
        return Ok(());
    }
    let mut reader = open_at(path, delimiter, start)?;
    let mut record = ByteRecord::new();
    let mut found = Vec::new();
    let mut records = 0usize;
    let mut width = 0usize;
    let mut first = None;
    let mut last_report = Instant::now();
    while reader.read_byte_record(&mut record).map_err(err)? {
        if records.is_multiple_of(STRIDE) {
            found.push(record.position().map_or(start, Position::byte));
        }
        if records == 0 {
            first = Some(lossy(&record));
        }
        width = width.max(record.len());
        records += 1;
        if records.is_multiple_of(CHECK_EVERY) && last_report.elapsed() >= REPORT_EVERY {
            let update = Update::Found {
                checkpoints: std::mem::take(&mut found),
                records,
                width,
                first: first.take(),
            };
            if tx.send(update).is_err() {
                return Ok(());
            }
            last_report = Instant::now();
        }
    }
    let _ = tx.send(Update::Found {
        checkpoints: found,
        records,
        width,
        first,
    });
    let _ = tx.send(Update::Done);
    Ok(())
}

/// `None` when cancelled.
fn sort_file(
    path: &Path,
    spec: &SortSpec,
    cancel: &AtomicBool,
) -> Option<Result<Vec<u64>, String>> {
    let run = || -> Result<Option<Vec<u64>>, String> {
        let (start, _) = layout(path, Some(spec.delimiter))?;
        let mut reader = open_at(path, spec.delimiter, start)?;
        let mut record = ByteRecord::new();
        let mut keyed: Vec<(SortKey, u64)> = Vec::new();
        let mut seen = 0usize;
        while reader.read_byte_record(&mut record).map_err(err)? {
            seen += 1;
            if seen.is_multiple_of(CHECK_EVERY) && cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if seen == 1 && spec.skip_first {
                continue;
            }
            let field = record.get(spec.column).unwrap_or(b"");
            let key = SortKey::of(&String::from_utf8_lossy(field));
            keyed.push((key, record.position().map_or(start, Position::byte)));
        }
        // Stable both ways, so equal values keep their file order.
        if spec.descending {
            keyed.sort_by(|a, b| b.0.compare(&a.0));
        } else {
            keyed.sort_by(|a, b| a.0.compare(&b.0));
        }
        Ok(Some(keyed.into_iter().map(|(_, offset)| offset).collect()))
    };
    run().transpose()
}

/// Where the records of `path` start (past a byte-order mark) and the
/// delimiter: `delimiter`, or the one guessed from the start of the file.
fn layout(path: &Path, delimiter: Option<u8>) -> Result<(u64, u8), String> {
    let mut head = Vec::new();
    File::open(path)
        .map_err(err)?
        .take(SNIFF_BYTES as u64 + 1)
        .read_to_end(&mut head)
        .map_err(err)?;
    let start = if head.starts_with(BOM) { BOM.len() } else { 0 };
    let whole = head.len() <= SNIFF_BYTES;
    let delimiter =
        delimiter.unwrap_or_else(|| table::guess_delimiter(path, &head[start..], whole));
    Ok((start as u64, delimiter))
}

fn open_at(path: &Path, delimiter: u8, offset: u64) -> Result<csv::Reader<File>, String> {
    let mut reader = table::reader(delimiter, File::open(path).map_err(err)?);
    if offset > 0 {
        reader.seek(position(offset)).map_err(err)?;
    }
    Ok(reader)
}

fn position(byte: u64) -> Position {
    let mut pos = Position::new();
    pos.set_byte(byte);
    pos
}

/// Text that is not UTF-8 is read lossily rather than refused: a stray byte
/// should not hide the whole table.
fn lossy(record: &ByteRecord) -> StringRecord {
    StringRecord::from_byte_record_lossy(record.clone())
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The whole file indexed on the calling thread.
#[cfg(test)]
pub fn index_now(path: &Path, delimiter: Option<u8>) -> Index {
    let (tx, rx) = mpsc::channel();
    index_file(path, delimiter, &tx).unwrap();
    drop(tx);
    let mut index = Index::default();
    for update in rx {
        index.apply(update);
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(content: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.csv");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn fields(records: &[StringRecord], col: usize) -> Vec<String> {
        records.iter().map(|r| r[col].to_string()).collect()
    }

    #[test]
    fn any_window_reads_from_its_checkpoint() {
        let text: String = (0..1000).map(|i| format!("{i},\"row\n{i}\"\n")).collect();
        let (_dir, path) = file(text.as_bytes());
        let index = index_now(&path, None);
        assert_eq!(index.records, 1000);
        assert_eq!(index.checkpoints.len(), 1000usize.div_ceil(STRIDE));
        let rows = index.read(&path, 2 * STRIDE - 1, 3).unwrap();
        assert_eq!(fields(&rows, 0), vec!["255", "256", "257"]);
        assert_eq!(&rows[1][1], "row\n256");
        // Past the end: what there is.
        assert_eq!(index.read(&path, 998, 10).unwrap().len(), 2);
        assert!(index.read(&path, 1000, 10).unwrap().is_empty());
    }

    #[test]
    fn quoted_fields_keep_their_delimiters_and_line_breaks() {
        let (_dir, path) = file(b"a,b\n\"x, y\",\"two\nlines\"\n");
        let index = index_now(&path, None);
        let rows = index.read(&path, 0, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(&rows[1][0], "x, y");
        assert_eq!(&rows[1][1], "two\nlines");
    }

    #[test]
    fn ragged_rows_a_bom_and_stray_bytes_are_read() {
        let (_dir, path) = file(b"\xEF\xBB\xBFa,b\n\n1\n\xFF,2,3\n");
        let index = index_now(&path, None);
        assert_eq!(index.records, 3, "a blank line is no record");
        assert_eq!(index.width, 3);
        assert_eq!(&index.first.as_ref().unwrap()[0], "a");
        let rows = index.read(&path, 0, 3).unwrap();
        assert_eq!(rows[1].len(), 1);
        assert_eq!(&rows[2][0], "\u{FFFD}");
    }

    #[test]
    fn a_sort_orders_the_data_rows_stably_both_ways() {
        let (_dir, path) = file(b"k,n\nb,1\na,2\nb,3\n,4\na,5\n");
        let sort = |descending| {
            let spec = SortSpec {
                delimiter: b',',
                skip_first: true,
                column: 0,
                descending,
            };
            let offsets = sort_file(&path, &spec, &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            fields(&read_at(&path, b',', &offsets).unwrap(), 1)
        };
        assert_eq!(sort(false), vec!["2", "5", "1", "3", "4"]);
        assert_eq!(sort(true), vec!["4", "1", "3", "2", "5"]);
    }

    #[test]
    fn a_cancelled_sort_returns_nothing() {
        let text: String = (0..CHECK_EVERY + 1).map(|i| format!("{i}\n")).collect();
        let (_dir, path) = file(text.as_bytes());
        let spec = SortSpec {
            delimiter: b',',
            skip_first: false,
            column: 0,
            descending: false,
        };
        assert!(sort_file(&path, &spec, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn a_missing_file_fails_the_index() {
        let rx = spawn_index(PathBuf::from("/nonexistent/x.csv"), None).unwrap();
        assert!(matches!(rx.recv().unwrap(), Update::Failed(_)));
    }
}
