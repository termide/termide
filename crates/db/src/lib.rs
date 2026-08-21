//! Database access core for termide.
//!
//! Engine-agnostic, read-only browsing of SQLite / PostgreSQL / MySQL: connect
//! from a URL, list tables, and fetch paginated rows. Queries run on a
//! background tokio runtime; callers interact through a synchronous handle that
//! polls results over a channel, mirroring the rest of termide's async-to-TUI
//! bridges (VFS, LSP).

mod engine;
mod error;
mod value;

use std::sync::mpsc;
use std::thread::{self, JoinHandle};

pub use error::DbError;
pub use value::DbValue;

/// One decoded result row.
pub type ColumnValueRow = Vec<DbValue>;

/// Which engine a URL/connection targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbBackend {
    Sqlite,
    Postgres,
    MySql,
}

impl DbBackend {
    /// Classify a connection URL by its scheme.
    pub fn from_url(url: &str) -> Result<Self, DbError> {
        let scheme = url.split(':').next().unwrap_or("");
        match scheme {
            "sqlite" => Ok(DbBackend::Sqlite),
            "postgres" | "postgresql" => Ok(DbBackend::Postgres),
            "mysql" | "mariadb" => Ok(DbBackend::MySql),
            other => Err(DbError::UnsupportedScheme(other.to_string())),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DbBackend::Sqlite => "SQLite",
            DbBackend::Postgres => "PostgreSQL",
            DbBackend::MySql => "MySQL",
        }
    }
}

/// Broad type category of a column, used to pick relevant filter operators and
/// to coerce filter input. Exotic types (json/array/uuid/…) fall to `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCategory {
    Number,
    Text,
    Bool,
    Date,
    Bytes,
    Other,
}

/// One column's name and inferred category (from the engine catalog).
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub category: TypeCategory,
    /// Whether the column accepts NULL. Drives the "NULL" checkbox in the row
    /// editor: a column that rejects NULL must not offer it.
    pub nullable: bool,
}

/// Sort direction for an `ORDER BY` term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// A filter comparison operator. `IsNull`/`IsNotNull` take no value; the LIKE
/// family (`Contains`/`StartsWith`/`EndsWith`) applies to text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Contains,
    StartsWith,
    EndsWith,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    IsNull,
    IsNotNull,
}

/// One `WHERE` condition: `column op value`. `value` is `None` for the null
/// operators. Values are always bound as parameters, never interpolated.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub column: String,
    pub op: FilterOp,
    pub value: Option<DbValue>,
}

/// A request for one page of a table's rows, with optional server-side filtering
/// and sorting. Multiple conditions are combined with `AND`.
#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub table: String,
    pub filters: Vec<Condition>,
    pub order_by: Vec<(String, SortDir)>,
    pub limit: u64,
    pub offset: u64,
}

/// A page of rows plus the column names of the result set.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub columns: Vec<String>,
    pub rows: Vec<ColumnValueRow>,
    /// The offset this page started at (echoes the request).
    pub offset: u64,
    /// Whether at least one more row exists past this page.
    pub has_more: bool,
}

/// What the caller asked the worker to do; each carries a reply channel the
/// caller polls non-blockingly.
enum Request {
    ListDatabases(mpsc::Sender<Result<Vec<String>, DbError>>),
    ListTables(mpsc::Sender<Result<Vec<String>, DbError>>),
    Columns {
        table: String,
        reply: mpsc::Sender<Result<Vec<ColumnInfo>, DbError>>,
    },
    Count {
        table: String,
        filters: Vec<Condition>,
        reply: mpsc::Sender<Result<i64, DbError>>,
    },
    Page {
        req: PageRequest,
        reply: mpsc::Sender<Result<Page, DbError>>,
    },
    PrimaryKey {
        table: String,
        reply: mpsc::Sender<Result<Vec<String>, DbError>>,
    },
    UpdateCell {
        table: String,
        /// Primary-key columns and their values for the row being edited.
        key: Vec<(String, DbValue)>,
        column: String,
        value: DbValue,
        reply: mpsc::Sender<Result<u64, DbError>>,
    },
}

/// A synchronous handle to a database, backed by a dedicated thread running a
/// current-thread tokio runtime. Each query method returns immediately with a
/// [`mpsc::Receiver`] the caller polls with `try_recv()` from the TUI loop.
///
/// Queries are serialised (one connection, one in flight) — fine for a viewer
/// and keeps the bridge trivial. Dropping the handle closes the channel, which
/// ends the worker loop and the pool.
pub struct DbConnection {
    backend: DbBackend,
    tx: Option<mpsc::Sender<Request>>,
    worker: Option<JoinHandle<()>>,
}

impl DbConnection {
    /// Connect to `url`. Blocks until the connection is established (or fails);
    /// callers that must not block the UI should run this on a short-lived
    /// background thread and poll for the resulting handle.
    pub fn connect(url: &str) -> Result<Self, DbError> {
        let backend = DbBackend::from_url(url)?;
        let url = url.to_string();
        let (init_tx, init_rx) = mpsc::channel::<Result<(), DbError>>();
        let (tx, rx) = mpsc::channel::<Request>();

        let worker = thread::Builder::new()
            .name("termide-db".into())
            .spawn(move || run_worker(url, init_tx, rx))
            .map_err(|e| DbError::Runtime(e.to_string()))?;

        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                backend,
                tx: Some(tx),
                worker: Some(worker),
            }),
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(e)
            }
            Err(_) => {
                let _ = worker.join();
                Err(DbError::Closed)
            }
        }
    }

    pub fn backend(&self) -> DbBackend {
        self.backend
    }

    /// List databases/schemas the connection can switch to. Poll the receiver.
    pub fn list_databases(&self) -> mpsc::Receiver<Result<Vec<String>, DbError>> {
        self.request(Request::ListDatabases)
    }

    /// List tables. Poll the returned receiver for the result.
    pub fn list_tables(&self) -> mpsc::Receiver<Result<Vec<String>, DbError>> {
        self.request(Request::ListTables)
    }

    /// Describe a table's columns (name + inferred type category).
    pub fn columns(
        &self,
        table: impl Into<String>,
    ) -> mpsc::Receiver<Result<Vec<ColumnInfo>, DbError>> {
        let table = table.into();
        self.request(|reply| Request::Columns { table, reply })
    }

    /// Count rows in `table`, honouring `filters` (the filtered total shown in
    /// the status bar).
    pub fn count(
        &self,
        table: impl Into<String>,
        filters: Vec<Condition>,
    ) -> mpsc::Receiver<Result<i64, DbError>> {
        let table = table.into();
        self.request(|reply| Request::Count {
            table,
            filters,
            reply,
        })
    }

    /// Fetch one page of rows.
    pub fn page(&self, req: PageRequest) -> mpsc::Receiver<Result<Page, DbError>> {
        self.request(|reply| Request::Page { req, reply })
    }

    /// Primary-key columns of `table`, in key order. An empty list means the
    /// table has no primary key, so its rows cannot be addressed for editing.
    pub fn primary_key(
        &self,
        table: impl Into<String>,
    ) -> mpsc::Receiver<Result<Vec<String>, DbError>> {
        let table = table.into();
        self.request(|reply| Request::PrimaryKey { table, reply })
    }

    /// Set one column of one row, addressed by its primary key.
    ///
    /// Resolves to the number of rows the server changed; the caller treats
    /// anything but 1 as a failed edit. For SQLite this is the point where the
    /// pool is reopened writable — browsing never holds a writable handle.
    pub fn update_cell(
        &self,
        table: impl Into<String>,
        key: Vec<(String, DbValue)>,
        column: impl Into<String>,
        value: DbValue,
    ) -> mpsc::Receiver<Result<u64, DbError>> {
        let table = table.into();
        let column = column.into();
        self.request(|reply| Request::UpdateCell {
            table,
            key,
            column,
            value,
            reply,
        })
    }

    /// Send a request built from a fresh reply channel and return its receiver.
    /// If the worker is gone, the receiver yields `Closed` immediately.
    fn request<T: Send + 'static>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, DbError>>) -> Request,
    ) -> mpsc::Receiver<Result<T, DbError>> {
        let (reply, rx) = mpsc::channel();
        let fail = reply.clone();
        let req = build(reply);
        match &self.tx {
            Some(tx) if tx.send(req).is_ok() => {}
            _ => {
                let _ = fail.send(Err(DbError::Closed));
            }
        }
        rx
    }
}

impl Drop for DbConnection {
    fn drop(&mut self) {
        // Closing the request channel ends the worker's recv loop, after which
        // it closes the pool and exits.
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Worker thread body: build a runtime, connect, then serve requests until the
/// channel closes.
fn run_worker(
    url: String,
    init_tx: mpsc::Sender<Result<(), DbError>>,
    rx: mpsc::Receiver<Request>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = init_tx.send(Err(DbError::Runtime(e.to_string())));
            return;
        }
    };

    let mut pool = match rt.block_on(engine::connect(&url)) {
        Ok(pool) => {
            let _ = init_tx.send(Ok(()));
            pool
        }
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };

    // SQLite starts read-only and is upgraded on the first edit; the server
    // engines have no such mode, so they count as writable from the start.
    let mut writable = !matches!(pool, engine::Pool::Sqlite(_));

    while let Ok(req) = rx.recv() {
        match req {
            Request::ListDatabases(reply) => {
                let _ = reply.send(rt.block_on(engine::list_databases(&pool)));
            }
            Request::ListTables(reply) => {
                let _ = reply.send(rt.block_on(engine::list_tables(&pool)));
            }
            Request::Columns { table, reply } => {
                let _ = reply.send(rt.block_on(engine::columns(&pool, &table)));
            }
            Request::Count {
                table,
                filters,
                reply,
            } => {
                let _ = reply.send(rt.block_on(engine::count_rows(&pool, &table, &filters)));
            }
            Request::Page { req, reply } => {
                let _ = reply.send(rt.block_on(engine::fetch_page(&pool, &req)));
            }
            Request::PrimaryKey { table, reply } => {
                let _ = reply.send(rt.block_on(engine::primary_key(&pool, &table)));
            }
            Request::UpdateCell {
                table,
                key,
                column,
                value,
                reply,
            } => {
                // SQLite browses through a read-only handle, so the first edit
                // reopens the pool writable. A database on read-only media
                // fails here with the driver's error, which is what the user
                // needs to see.
                let upgraded = match &pool {
                    engine::Pool::Sqlite(_) if !writable => {
                        match rt.block_on(engine::connect_writable(&url)) {
                            Ok(new_pool) => {
                                let old = std::mem::replace(&mut pool, new_pool);
                                rt.block_on(engine::close(old));
                                writable = true;
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    _ => Ok(()),
                };
                let result = match upgraded {
                    Ok(()) => {
                        rt.block_on(engine::update_cell(&pool, &table, &key, &column, &value))
                    }
                    Err(e) => Err(e),
                };
                let _ = reply.send(result);
            }
        }
    }

    rt.block_on(engine::close(pool));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sqlite_db() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let setup_url = format!("sqlite://{}?mode=rwc", path.display());

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pool = sqlx::SqlitePool::connect(&setup_url).await.unwrap();
            sqlx::query(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER)",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO users (name, score, active) VALUES \
                 ('alice', 1.5, 1), ('bob', 2.25, 0), ('carol', NULL, 1)",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("CREATE TABLE empty_t (x INTEGER)")
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
        });

        // Read-only URL (no mode=rwc) for the viewer under test.
        (dir, format!("sqlite://{}", path.display()))
    }

    #[test]
    fn backend_from_url() {
        assert_eq!(
            DbBackend::from_url("sqlite:///x.db").unwrap(),
            DbBackend::Sqlite
        );
        assert_eq!(
            DbBackend::from_url("postgres://localhost/db").unwrap(),
            DbBackend::Postgres
        );
        assert_eq!(
            DbBackend::from_url("mysql://h/db").unwrap(),
            DbBackend::MySql
        );
        assert!(DbBackend::from_url("redis://x").is_err());
    }

    /// The row editor offers a NULL checkbox only where the column accepts it,
    /// so nullability has to come from the catalog.
    #[test]
    fn sqlite_columns_report_nullability() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let columns = conn.columns("users").recv().unwrap().unwrap();
        let by_name = |name: &str| {
            columns
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("no column {name}"))
        };
        // INTEGER PRIMARY KEY is the rowid alias — SQLite reports it as
        // nullable, but the other columns are plainly nullable too.
        assert!(by_name("name").nullable);
        assert!(by_name("score").nullable);
    }

    #[test]
    fn sqlite_primary_key_is_reported_in_key_order() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let key = conn.primary_key("users").recv().unwrap().unwrap();
        assert_eq!(key, vec!["id".to_string()]);

        // A table without a primary key reports none — editing is refused for
        // it rather than guessing which row to write.
        let none = conn.primary_key("empty_t").recv().unwrap().unwrap();
        assert!(none.is_empty(), "{none:?}");
    }

    /// The viewer browses SQLite through a read-only handle; the first edit has
    /// to reopen it writable, and the new value must be visible afterwards.
    #[test]
    fn sqlite_update_cell_writes_through_a_read_only_handle() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let affected = conn
            .update_cell(
                "users",
                vec![("id".to_string(), DbValue::Int(1))],
                "name",
                DbValue::Text("alice-edited".to_string()),
            )
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(affected, 1);

        let page = conn
            .page(PageRequest {
                table: "users".to_string(),
                filters: vec![],
                order_by: vec![],
                limit: 10,
                offset: 0,
            })
            .recv()
            .unwrap()
            .unwrap();
        let names: Vec<String> = page
            .rows
            .iter()
            .map(|row| match &row[1] {
                DbValue::Text(s) => s.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert!(
            names.contains(&"alice-edited".to_string()),
            "edit not visible: {names:?}"
        );
    }

    /// NULL is a value like any other: writing it must clear the cell, not
    /// store the text "NULL".
    #[test]
    fn sqlite_update_cell_can_write_null() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let affected = conn
            .update_cell(
                "users",
                vec![("id".to_string(), DbValue::Int(1))],
                "name",
                DbValue::Null,
            )
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(affected, 1);

        let page = conn
            .page(PageRequest {
                table: "users".to_string(),
                filters: vec![],
                order_by: vec![],
                limit: 10,
                offset: 0,
            })
            .recv()
            .unwrap()
            .unwrap();
        let first = page.rows.iter().find(|row| row[0] == DbValue::Int(1));
        assert_eq!(first.map(|row| &row[1]), Some(&DbValue::Null));
    }

    /// A key that matches nothing reports zero rows changed, so the panel can
    /// tell the user the row is gone instead of pretending the edit landed.
    #[test]
    fn sqlite_update_cell_reports_no_rows_for_a_stale_key() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let affected = conn
            .update_cell(
                "users",
                vec![("id".to_string(), DbValue::Int(9_999))],
                "name",
                DbValue::Text("ghost".to_string()),
            )
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(affected, 0);
    }

    /// Editing a table with no primary key is refused before any SQL runs.
    #[test]
    fn sqlite_update_cell_refuses_a_table_without_a_key() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let err = conn
            .update_cell("empty_t", vec![], "x", DbValue::Int(1))
            .recv()
            .unwrap()
            .expect_err("must refuse");
        assert!(matches!(err, DbError::Rejected(_)), "{err:?}");
    }

    #[test]
    fn sqlite_list_tables() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();
        let tables = conn.list_tables().recv().unwrap().unwrap();
        assert_eq!(tables, vec!["empty_t".to_string(), "users".to_string()]);
    }

    #[test]
    fn sqlite_count_and_page() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let total = conn.count("users", vec![]).recv().unwrap().unwrap();
        assert_eq!(total, 3);

        let page = conn
            .page(PageRequest {
                table: "users".into(),
                limit: 2,
                offset: 0,
                ..Default::default()
            })
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(page.columns, vec!["id", "name", "score", "active"]);
        assert_eq!(page.rows.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.rows[0][0], DbValue::Int(1));
        assert_eq!(page.rows[0][1], DbValue::Text("alice".into()));
        assert_eq!(page.rows[0][2], DbValue::Float(1.5));

        // Second page: the remaining row, NULL preserved, no more pages.
        let page2 = conn
            .page(PageRequest {
                table: "users".into(),
                limit: 2,
                offset: 2,
                ..Default::default()
            })
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(page2.rows.len(), 1);
        assert!(!page2.has_more);
        assert_eq!(page2.rows[0][1], DbValue::Text("carol".into()));
        assert_eq!(page2.rows[0][2], DbValue::Null);
    }

    #[test]
    fn sqlite_columns_with_categories() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        // Works for an empty table too (catalog-derived, not row-derived).
        let cols = conn.columns("empty_t").recv().unwrap().unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, "x");
        assert_eq!(cols[0].category, TypeCategory::Number);

        let cols = conn.columns("users").recv().unwrap().unwrap();
        let by_name = |n: &str| cols.iter().find(|c| c.name == n).unwrap().category;
        assert_eq!(by_name("id"), TypeCategory::Number);
        assert_eq!(by_name("name"), TypeCategory::Text);
        assert_eq!(by_name("score"), TypeCategory::Number); // REAL
        assert_eq!(by_name("active"), TypeCategory::Number); // INTEGER
    }

    #[test]
    fn sqlite_filter_contains_and_compare() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        // contains "a" → alice, carol (case-insensitive ASCII).
        let filters = vec![Condition {
            column: "name".into(),
            op: FilterOp::Contains,
            value: Some(DbValue::Text("a".into())),
        }];
        let total = conn
            .count("users", filters.clone())
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(total, 2);

        let page = conn
            .page(PageRequest {
                table: "users".into(),
                filters,
                order_by: vec![("name".into(), SortDir::Asc)],
                limit: 50,
                offset: 0,
            })
            .recv()
            .unwrap()
            .unwrap();
        let names: Vec<String> = page.rows.iter().map(|r| r[1].display()).collect();
        assert_eq!(names, vec!["alice", "carol"]);

        // score >= 2.0 → only bob.
        let page = conn
            .page(PageRequest {
                table: "users".into(),
                filters: vec![Condition {
                    column: "score".into(),
                    op: FilterOp::Ge,
                    value: Some(DbValue::Float(2.0)),
                }],
                ..Default::default()
            })
            .recv()
            .unwrap()
            .unwrap();
        // limit defaults to 0 → coerced to 1 in engine; bob is the only match.
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0][1], DbValue::Text("bob".into()));
    }

    #[test]
    fn sqlite_filter_is_null_and_sort_desc() {
        let (_dir, url) = make_sqlite_db();
        let conn = DbConnection::connect(&url).unwrap();

        let page = conn
            .page(PageRequest {
                table: "users".into(),
                filters: vec![Condition {
                    column: "score".into(),
                    op: FilterOp::IsNull,
                    value: None,
                }],
                limit: 50,
                ..Default::default()
            })
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0][1], DbValue::Text("carol".into()));

        // Sort by name DESC → carol, bob, alice.
        let page = conn
            .page(PageRequest {
                table: "users".into(),
                order_by: vec![("name".into(), SortDir::Desc)],
                limit: 50,
                ..Default::default()
            })
            .recv()
            .unwrap()
            .unwrap();
        let names: Vec<String> = page.rows.iter().map(|r| r[1].display()).collect();
        assert_eq!(names, vec!["carol", "bob", "alice"]);
    }

    #[test]
    fn connect_rejects_bad_scheme() {
        match DbConnection::connect("redis://localhost") {
            Err(DbError::UnsupportedScheme(_)) => {}
            other => panic!("expected UnsupportedScheme, got {:?}", other.map(|_| ())),
        }
    }
}
