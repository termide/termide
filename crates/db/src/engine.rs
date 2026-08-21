//! Engine layer: per-backend sqlx pools and the async query primitives.
//!
//! Reads are `SELECT`/metadata queries; the only write is a single-row
//! `UPDATE` addressed by primary key ([`update_cell`]). Table and column
//! identifiers are quoted (never parameterised — they can't be), while every
//! user-supplied *value* (filter operands, updated values, key values) is
//! bound as a parameter.

use std::str::FromStr;

use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Column, Row};

use crate::{
    ColumnInfo, ColumnValueRow, Condition, DbBackend, DbError, DbValue, FilterOp, Page,
    PageRequest, SortDir, TypeCategory,
};

/// Bind a slice of [`DbValue`]s onto a concrete sqlx query, in order. Works for
/// any engine's query type because all three support these scalar binds.
macro_rules! bind_values {
    ($q:expr, $binds:expr) => {{
        let mut q = $q;
        for v in $binds.iter() {
            q = match v {
                DbValue::Int(i) => q.bind(*i),
                DbValue::Float(f) => q.bind(*f),
                DbValue::Bool(b) => q.bind(*b),
                DbValue::Text(s) => q.bind(s.clone()),
                DbValue::Bytes(b) => q.bind(b.clone()),
                DbValue::Null => q.bind(Option::<String>::None),
            };
        }
        q
    }};
}

/// A live pool, one variant per supported engine.
pub(crate) enum Pool {
    Sqlite(sqlx::SqlitePool),
    Postgres(sqlx::PgPool),
    MySql(sqlx::MySqlPool),
}

/// Connect to `url`, returning a single-connection pool. SQLite opens
/// read-only so browsing can never mutate the file and databases on read-only
/// media still open; the pool is reopened writable on the first edit (see
/// [`connect_writable`]). Postgres/MySQL have no such mode — they simply are
/// not sent writes until the user edits a cell.
pub(crate) async fn connect(url: &str) -> Result<Pool, DbError> {
    connect_with_writes(url, false).await
}

/// Connect with writes enabled — used when an edit is applied, so a viewer
/// that only browses keeps its read-only guarantee.
pub(crate) async fn connect_writable(url: &str) -> Result<Pool, DbError> {
    connect_with_writes(url, true).await
}

async fn connect_with_writes(url: &str, writable: bool) -> Result<Pool, DbError> {
    match DbBackend::from_url(url)? {
        DbBackend::Sqlite => {
            let opts = SqliteConnectOptions::from_str(url)?.read_only(!writable);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await?;
            Ok(Pool::Sqlite(pool))
        }
        DbBackend::Postgres => {
            let mut opts = PgConnectOptions::from_str(url)?;
            // PostgreSQL requires a database to connect to. When the URL omits
            // one, bootstrap on the standard `postgres` maintenance DB so we can
            // still enumerate databases and let the user pick.
            if opts.get_database().is_none_or(str::is_empty) {
                opts = opts.database("postgres");
            }
            let pool = PgPoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await?;
            Ok(Pool::Postgres(pool))
        }
        DbBackend::MySql => {
            let opts = MySqlConnectOptions::from_str(url)?;
            let pool = MySqlPoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await?;
            Ok(Pool::MySql(pool))
        }
    }
}

pub(crate) async fn close(pool: Pool) {
    match pool {
        Pool::Sqlite(p) => p.close().await,
        Pool::Postgres(p) => p.close().await,
        Pool::MySql(p) => p.close().await,
    }
}

/// List user tables (excludes engine internals / system schemas).
pub(crate) async fn list_tables(pool: &Pool) -> Result<Vec<String>, DbError> {
    let sql = match pool {
        Pool::Sqlite(_) => {
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        }
        Pool::Postgres(_) => {
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' \
             ORDER BY table_name"
        }
        Pool::MySql(_) => {
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' \
             ORDER BY table_name"
        }
    };
    let names = match pool {
        Pool::Sqlite(p) => collect_first_column(sqlx::query(sql).fetch_all(p).await?),
        Pool::Postgres(p) => collect_first_column(sqlx::query(sql).fetch_all(p).await?),
        Pool::MySql(p) => collect_first_column(sqlx::query(sql).fetch_all(p).await?),
    };
    Ok(names)
}

/// List databases/schemas the connection can switch to. SQLite has none.
pub(crate) async fn list_databases(pool: &Pool) -> Result<Vec<String>, DbError> {
    let names = match pool {
        Pool::Sqlite(_) => Vec::new(),
        Pool::Postgres(p) => {
            // Cast to text: `datname` is the `name` type, which doesn't decode
            // straight to String.
            let sql = "SELECT datname::text FROM pg_catalog.pg_database \
                       WHERE datistemplate = false ORDER BY 1";
            collect_first_column(sqlx::query(sql).fetch_all(p).await?)
        }
        Pool::MySql(p) => {
            let sql = "SELECT schema_name FROM information_schema.schemata \
                       ORDER BY schema_name";
            collect_first_column(sqlx::query(sql).fetch_all(p).await?)
        }
    };
    Ok(names)
}

/// Describe a table's columns: name + inferred [`TypeCategory`]. Comes from the
/// engine catalog, so it works for empty tables too (unlike deriving columns
/// from a result set).
pub(crate) async fn columns(pool: &Pool, table: &str) -> Result<Vec<ColumnInfo>, DbError> {
    match pool {
        Pool::Sqlite(p) => {
            let sql = format!(
                "PRAGMA table_info({})",
                quote_ident(DbBackend::Sqlite, table)
            );
            let rows = sqlx::query(&sql).fetch_all(p).await?;
            Ok(rows
                .iter()
                .map(|r| {
                    let name: String = r.try_get("name").unwrap_or_default();
                    let ty: String = r.try_get("type").unwrap_or_default();
                    let notnull: i64 = r.try_get("notnull").unwrap_or(0);
                    ColumnInfo {
                        name,
                        category: sqlite_category(&ty),
                        nullable: notnull == 0,
                    }
                })
                .collect())
        }
        Pool::Postgres(p) => {
            let sql = "SELECT column_name::text AS column_name, data_type::text AS data_type, \
                              is_nullable::text AS is_nullable \
                       FROM information_schema.columns \
                       WHERE table_schema = current_schema() AND table_name = $1 \
                       ORDER BY ordinal_position";
            let rows = sqlx::query(sql).bind(table).fetch_all(p).await?;
            Ok(rows
                .iter()
                .map(|r| {
                    let name: String = r.try_get("column_name").unwrap_or_default();
                    let ty: String = r.try_get("data_type").unwrap_or_default();
                    let nullable: String = r.try_get("is_nullable").unwrap_or_default();
                    ColumnInfo {
                        name,
                        category: pg_category(&ty),
                        nullable: nullable.eq_ignore_ascii_case("yes"),
                    }
                })
                .collect())
        }
        Pool::MySql(p) => {
            let sql = "SELECT column_name, data_type, is_nullable \
                       FROM information_schema.columns \
                       WHERE table_schema = DATABASE() AND table_name = ? \
                       ORDER BY ordinal_position";
            let rows = sqlx::query(sql).bind(table).fetch_all(p).await?;
            Ok(rows
                .iter()
                .map(|r| {
                    let name: String = r.try_get("column_name").unwrap_or_default();
                    let ty: String = r.try_get("data_type").unwrap_or_default();
                    let nullable: String = r.try_get("is_nullable").unwrap_or_default();
                    ColumnInfo {
                        name,
                        category: mysql_category(&ty),
                        nullable: nullable.eq_ignore_ascii_case("yes"),
                    }
                })
                .collect())
        }
    }
}

/// Primary-key column names for `table`, in key order. Empty when the table
/// has no primary key — cell editing is refused in that case, because without
/// a key there is no way to name exactly one row.
pub(crate) async fn primary_key(pool: &Pool, table: &str) -> Result<Vec<String>, DbError> {
    match pool {
        Pool::Sqlite(p) => {
            let sql = format!(
                "PRAGMA table_info({})",
                quote_ident(DbBackend::Sqlite, table)
            );
            let rows = sqlx::query(&sql).fetch_all(p).await?;
            // `pk` is the 1-based position within the key, 0 for other columns.
            let mut keyed: Vec<(i64, String)> = rows
                .iter()
                .filter_map(|r| {
                    let pk: i64 = r.try_get("pk").unwrap_or(0);
                    let name: String = r.try_get("name").unwrap_or_default();
                    (pk > 0).then_some((pk, name))
                })
                .collect();
            keyed.sort_by_key(|(pos, _)| *pos);
            Ok(keyed.into_iter().map(|(_, name)| name).collect())
        }
        Pool::Postgres(p) => {
            // Cast to text: `name`/`sql_identifier` don't decode to String.
            let sql = "SELECT kcu.column_name::text AS column_name \
                       FROM information_schema.table_constraints tc \
                       JOIN information_schema.key_column_usage kcu \
                         ON kcu.constraint_name = tc.constraint_name \
                        AND kcu.table_schema = tc.table_schema \
                       WHERE tc.constraint_type = 'PRIMARY KEY' \
                         AND tc.table_schema = current_schema() \
                         AND tc.table_name = $1 \
                       ORDER BY kcu.ordinal_position";
            Ok(collect_first_column(
                sqlx::query(sql).bind(table).fetch_all(p).await?,
            ))
        }
        Pool::MySql(p) => {
            let sql = "SELECT column_name FROM information_schema.key_column_usage \
                       WHERE table_schema = DATABASE() AND table_name = ? \
                         AND constraint_name = 'PRIMARY' \
                       ORDER BY ordinal_position";
            Ok(collect_first_column(
                sqlx::query(sql).bind(table).fetch_all(p).await?,
            ))
        }
    }
}

/// Set one column of one row, addressed by its primary key.
///
/// Returns the number of rows the server reports as changed, which the caller
/// checks: anything but 1 means the row the grid displayed is not the row on
/// disk any more (deleted, or the key changed underneath), and the edit is
/// reported as failed rather than silently accepted.
pub(crate) async fn update_cell(
    pool: &Pool,
    table: &str,
    key: &[(String, DbValue)],
    column: &str,
    value: &DbValue,
) -> Result<u64, DbError> {
    if key.is_empty() {
        return Err(DbError::Rejected(
            "table has no primary key, cannot address a row".to_string(),
        ));
    }
    let backend = match pool {
        Pool::Sqlite(_) => DbBackend::Sqlite,
        Pool::Postgres(_) => DbBackend::Postgres,
        Pool::MySql(_) => DbBackend::MySql,
    };

    // Binds go in statement order: the new value first, then the key values.
    let mut binds: Vec<DbValue> = Vec::with_capacity(key.len() + 1);
    binds.push(value.clone());
    let mut conditions = Vec::with_capacity(key.len());
    for (name, key_value) in key {
        binds.push(key_value.clone());
        conditions.push(format!(
            "{} = {}",
            quote_ident(backend, name),
            placeholder(backend, binds.len())
        ));
    }
    let sql = format!(
        "UPDATE {} SET {} = {} WHERE {}",
        quote_ident(backend, table),
        quote_ident(backend, column),
        placeholder(backend, 1),
        conditions.join(" AND ")
    );

    let affected = match pool {
        Pool::Sqlite(p) => bind_values!(sqlx::query(&sql), binds)
            .execute(p)
            .await?
            .rows_affected(),
        Pool::Postgres(p) => bind_values!(sqlx::query(&sql), binds)
            .execute(p)
            .await?
            .rows_affected(),
        Pool::MySql(p) => bind_values!(sqlx::query(&sql), binds)
            .execute(p)
            .await?
            .rows_affected(),
    };
    Ok(affected)
}

/// Count rows matching `filters` (`SELECT COUNT(*)` + the same `WHERE`).
pub(crate) async fn count_rows(
    pool: &Pool,
    table: &str,
    filters: &[Condition],
) -> Result<i64, DbError> {
    let b = backend(pool);
    let (where_sql, binds) = build_where(b, filters);
    let sql = format!(
        "SELECT COUNT(*) FROM {}{}",
        quote_ident(b, table),
        where_sql
    );
    let n = match pool {
        Pool::Sqlite(p) => bind_values!(sqlx::query(&sql), binds)
            .fetch_one(p)
            .await?
            .try_get::<i64, _>(0)?,
        Pool::Postgres(p) => bind_values!(sqlx::query(&sql), binds)
            .fetch_one(p)
            .await?
            .try_get::<i64, _>(0)?,
        Pool::MySql(p) => bind_values!(sqlx::query(&sql), binds)
            .fetch_one(p)
            .await?
            .try_get::<i64, _>(0)?,
    };
    Ok(n)
}

/// Fetch one page of a table with optional filtering/sorting. Reads `limit + 1`
/// rows to cheaply learn whether more pages follow, then truncates to `limit`.
pub(crate) async fn fetch_page(pool: &Pool, req: &PageRequest) -> Result<Page, DbError> {
    let b = backend(pool);
    let limit = req.limit.max(1);
    let probe = limit.saturating_add(1);
    let table = quote_ident(b, &req.table);
    let (where_sql, binds) = build_where(b, &req.filters);
    let order_sql = build_order(b, &req.order_by);
    let sql = format!(
        "SELECT * FROM {table}{where_sql}{order_sql} LIMIT {probe} OFFSET {}",
        req.offset
    );

    let mut page = match pool {
        Pool::Sqlite(p) => page_from(bind_values!(sqlx::query(&sql), binds).fetch_all(p).await?),
        Pool::Postgres(p) => page_from(bind_values!(sqlx::query(&sql), binds).fetch_all(p).await?),
        Pool::MySql(p) => page_from(bind_values!(sqlx::query(&sql), binds).fetch_all(p).await?),
    };
    page.offset = req.offset;
    page.has_more = page.rows.len() as u64 > limit;
    page.rows.truncate(limit as usize);
    Ok(page)
}

fn backend(pool: &Pool) -> DbBackend {
    match pool {
        Pool::Sqlite(_) => DbBackend::Sqlite,
        Pool::Postgres(_) => DbBackend::Postgres,
        Pool::MySql(_) => DbBackend::MySql,
    }
}

/// Quote an identifier for safe interpolation. Values are never interpolated —
/// only table/column names, which come from the engine's own catalog.
fn quote_ident(backend: DbBackend, name: &str) -> String {
    match backend {
        DbBackend::MySql => format!("`{}`", name.replace('`', "``")),
        DbBackend::Sqlite | DbBackend::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Positional bind placeholder: `$1, $2, …` for Postgres, `?` elsewhere.
fn placeholder(backend: DbBackend, idx: usize) -> String {
    match backend {
        DbBackend::Postgres => format!("${idx}"),
        DbBackend::Sqlite | DbBackend::MySql => "?".to_string(),
    }
}

/// Escape LIKE wildcards so user input matches literally (with `ESCAPE '\'`).
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Build the ` WHERE …` clause (with leading space) plus the ordered bind
/// values. Identifiers are quoted; operands are returned for binding, never
/// inlined. Conditions are joined with `AND`.
fn build_where(backend: DbBackend, conds: &[Condition]) -> (String, Vec<DbValue>) {
    if conds.is_empty() {
        return (String::new(), Vec::new());
    }
    let mut parts = Vec::with_capacity(conds.len());
    let mut binds = Vec::new();
    let mut idx = 1usize;
    for c in conds {
        let col = quote_ident(backend, &c.column);
        match c.op {
            FilterOp::IsNull => parts.push(format!("{col} IS NULL")),
            FilterOp::IsNotNull => parts.push(format!("{col} IS NOT NULL")),
            FilterOp::Contains | FilterOp::StartsWith | FilterOp::EndsWith => {
                // Case-insensitive contains: PG needs ILIKE; SQLite LIKE is
                // ASCII-insensitive and MySQL LIKE is collation-insensitive.
                let like = if backend == DbBackend::Postgres {
                    "ILIKE"
                } else {
                    "LIKE"
                };
                let ph = placeholder(backend, idx);
                idx += 1;
                parts.push(format!("{col} {like} {ph} ESCAPE '\\'"));
                let raw = match &c.value {
                    Some(DbValue::Text(s)) => s.clone(),
                    Some(v) => v.display(),
                    None => String::new(),
                };
                let esc = escape_like(&raw);
                let pat = match c.op {
                    FilterOp::Contains => format!("%{esc}%"),
                    FilterOp::StartsWith => format!("{esc}%"),
                    FilterOp::EndsWith => format!("%{esc}"),
                    _ => unreachable!(),
                };
                binds.push(DbValue::Text(pat));
            }
            op => {
                let sym = match op {
                    FilterOp::Eq => "=",
                    FilterOp::Ne => "<>",
                    FilterOp::Gt => ">",
                    FilterOp::Ge => ">=",
                    FilterOp::Lt => "<",
                    FilterOp::Le => "<=",
                    _ => unreachable!(),
                };
                let ph = placeholder(backend, idx);
                idx += 1;
                parts.push(format!("{col} {sym} {ph}"));
                binds.push(c.value.clone().unwrap_or(DbValue::Null));
            }
        }
    }
    (format!(" WHERE {}", parts.join(" AND ")), binds)
}

/// Build the ` ORDER BY …` clause (with leading space), or empty.
fn build_order(backend: DbBackend, order: &[(String, SortDir)]) -> String {
    if order.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = order
        .iter()
        .map(|(col, dir)| {
            let d = match dir {
                SortDir::Asc => "ASC",
                SortDir::Desc => "DESC",
            };
            format!("{} {}", quote_ident(backend, col), d)
        })
        .collect();
    format!(" ORDER BY {}", parts.join(", "))
}

fn sqlite_category(decl: &str) -> TypeCategory {
    // SQLite type affinity is substring-based; bool/date are conventions only.
    let t = decl.to_uppercase();
    if t.contains("BOOL") {
        TypeCategory::Bool
    } else if t.contains("INT") {
        TypeCategory::Number
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        TypeCategory::Text
    } else if t.contains("DATE") || t.contains("TIME") {
        TypeCategory::Date
    } else if t.contains("REAL")
        || t.contains("FLOA")
        || t.contains("DOUB")
        || t.contains("NUM")
        || t.contains("DEC")
    {
        TypeCategory::Number
    } else if t.is_empty() || t.contains("BLOB") {
        TypeCategory::Bytes
    } else {
        TypeCategory::Other
    }
}

fn pg_category(data_type: &str) -> TypeCategory {
    let t = data_type.to_lowercase();
    if t == "boolean" {
        TypeCategory::Bool
    } else if t.contains("int")
        || t == "numeric"
        || t == "decimal"
        || t == "real"
        || t.contains("double")
        || t.contains("serial")
        || t == "money"
    {
        TypeCategory::Number
    } else if t.contains("char") || t == "text" || t == "citext" || t == "name" {
        TypeCategory::Text
    } else if t.contains("timestamp") || t == "date" || t.contains("time") {
        TypeCategory::Date
    } else if t == "bytea" {
        TypeCategory::Bytes
    } else {
        TypeCategory::Other
    }
}

fn mysql_category(data_type: &str) -> TypeCategory {
    // MySQL has no real boolean (it's tinyint(1)); treat integer-family as Number.
    let t = data_type.to_lowercase();
    if t.contains("int")
        || t == "decimal"
        || t == "numeric"
        || t == "float"
        || t == "double"
        || t == "bit"
    {
        TypeCategory::Number
    } else if t.contains("char") || t.contains("text") || t == "enum" || t == "set" {
        TypeCategory::Text
    } else if t == "date" || t == "datetime" || t == "timestamp" || t == "time" || t == "year" {
        TypeCategory::Date
    } else if t.contains("blob") || t.contains("binary") {
        TypeCategory::Bytes
    } else {
        TypeCategory::Other
    }
}

fn collect_first_column<R: Row>(rows: Vec<R>) -> Vec<String>
where
    for<'a> String: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    usize: sqlx::ColumnIndex<R>,
{
    rows.iter()
        .filter_map(|r| r.try_get::<String, _>(0).ok())
        .collect()
}

/// Build a [`Page`] from a result set: column names from the row metadata, and
/// each cell decoded into a [`DbValue`] via [`decode_cell`].
fn page_from<R: Row>(rows: Vec<R>) -> Page
where
    for<'a> i64: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> i32: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> i16: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> i8: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> f64: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> f32: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> bool: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> String: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Vec<u8>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    usize: sqlx::ColumnIndex<R>,
{
    let columns: Vec<String> = rows
        .first()
        .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
        .unwrap_or_default();

    let rows: Vec<ColumnValueRow> = rows
        .iter()
        .map(|r| (0..r.len()).map(|i| decode_cell(r, i)).collect())
        .collect();

    Page {
        columns,
        rows,
        offset: 0,
        has_more: false,
    }
}

/// Decode a single cell to [`DbValue`] by trying the common scalar types in
/// width order. Engine-specific exotics (numeric, json, arrays, timestamps,
/// unsigned ints, uuid) currently fall through to `Null` — see the roadmap's
/// "type zoo" item; refined in a later phase.
fn decode_cell<R: Row>(row: &R, i: usize) -> DbValue
where
    for<'a> i64: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> i32: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> i16: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> i8: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> f64: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> f32: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> bool: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> String: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> Vec<u8>: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    usize: sqlx::ColumnIndex<R>,
{
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return v.map_or(DbValue::Null, DbValue::Int);
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(i) {
        return v.map_or(DbValue::Null, |n| DbValue::Int(n as i64));
    }
    if let Ok(v) = row.try_get::<Option<i16>, _>(i) {
        return v.map_or(DbValue::Null, |n| DbValue::Int(n as i64));
    }
    if let Ok(v) = row.try_get::<Option<i8>, _>(i) {
        return v.map_or(DbValue::Null, |n| DbValue::Int(n as i64));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return v.map_or(DbValue::Null, DbValue::Float);
    }
    if let Ok(v) = row.try_get::<Option<f32>, _>(i) {
        return v.map_or(DbValue::Null, |f| DbValue::Float(f as f64));
    }
    if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
        return v.map_or(DbValue::Null, DbValue::Bool);
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return v.map_or(DbValue::Null, DbValue::Text);
    }
    if let Ok(v) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return v.map_or(DbValue::Null, DbValue::Bytes);
    }
    DbValue::Null
}
