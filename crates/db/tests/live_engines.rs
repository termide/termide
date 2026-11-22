//! Live PostgreSQL / MySQL integration tests.
//!
//! These exercise the engine-specific SQL that the hermetic SQLite tests can't
//! reach: `information_schema` catalog queries, `ILIKE`/`LIKE`, `$1` vs `?`
//! placeholders, and per-engine type-category mapping.
//!
//! They are **local-only and opt-in**: each test reads a connection URL from an
//! environment variable and skips (no-op) when it isn't set, so CI — which
//! never sets them — simply passes. To run them locally against a throwaway
//! database:
//!
//! ```sh
//! TERMIDE_TEST_POSTGRES_URL=postgres://user:pass@localhost/testdb \
//! TERMIDE_TEST_MYSQL_URL=mysql://user:pass@localhost/testdb \
//!     cargo test -p termide-db --test live_engines -- --nocapture
//! ```
//!
//! Each test creates and drops its own table (`termide_it_users`,
//! `termide_it_catalog`), so they can run in parallel against one database.

use termide_db::{Condition, DbConnection, DbValue, FilterOp, PageRequest, SortDir, TypeCategory};

fn env_url(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Shared assertions once `termide_it_users` exists with the canonical rows:
/// (alice, 1.5, true), (bob, 2.25, false), (carol, NULL, true).
fn assert_browse(conn: &DbConnection, active_is_bool: bool) {
    let tables = conn.list_tables().recv().unwrap().unwrap();
    assert!(
        tables.iter().any(|t| t == "termide_it_users"),
        "table should be listed, got {tables:?}"
    );

    let cols = conn.columns("termide_it_users").recv().unwrap().unwrap();
    let cat = |n: &str| cols.iter().find(|c| c.name == n).unwrap().category;
    assert_eq!(cat("id"), TypeCategory::Number);
    assert_eq!(cat("name"), TypeCategory::Text);
    assert_eq!(cat("score"), TypeCategory::Number);
    assert_eq!(
        cat("active"),
        if active_is_bool {
            TypeCategory::Bool
        } else {
            TypeCategory::Number
        }
    );

    let total = conn
        .count("termide_it_users", vec![])
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(total, 3);

    // Case-insensitive "contains a" → alice, carol.
    let filters = vec![Condition {
        column: "name".into(),
        op: FilterOp::Contains,
        value: Some(DbValue::Text("A".into())),
    }];
    let filtered = conn
        .count("termide_it_users", filters.clone())
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(filtered, 2, "case-insensitive contains should match 2 rows");

    // LIKE wildcards in the operand match literally: no name contains `_`.
    let literal = vec![Condition {
        column: "name".into(),
        op: FilterOp::Contains,
        value: Some(DbValue::Text("_".into())),
    }];
    let none = conn
        .count("termide_it_users", literal)
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(none, 0, "`_` should not act as a wildcard");

    let page = conn
        .page(PageRequest {
            table: "termide_it_users".into(),
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

    // NULL preserved, numeric comparison works.
    let ge = conn
        .page(PageRequest {
            table: "termide_it_users".into(),
            filters: vec![Condition {
                column: "score".into(),
                op: FilterOp::Ge,
                value: Some(DbValue::Float(2.0)),
            }],
            order_by: vec![],
            limit: 50,
            offset: 0,
        })
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(ge.rows.len(), 1);
    assert_eq!(ge.rows[0][1], DbValue::Text("bob".into()));
}

#[test]
fn postgres_browse_filter_sort() {
    let Some(url) = env_url("TERMIDE_TEST_POSTGRES_URL") else {
        eprintln!("skip: set TERMIDE_TEST_POSTGRES_URL to run the PostgreSQL test");
        return;
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        sqlx::query("DROP TABLE IF EXISTS termide_it_users")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE termide_it_users (\
             id serial PRIMARY KEY, name text, score double precision, active boolean)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO termide_it_users (name, score, active) VALUES \
             ('alice', 1.5, true), ('bob', 2.25, false), ('carol', NULL, true)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    });

    let conn = DbConnection::connect(&url).unwrap();
    assert_browse(&conn, true);
    drop(conn);

    rt.block_on(async {
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        sqlx::query("DROP TABLE termide_it_users")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    });
}

/// Filters and edits on columns Postgres does not compare with text: what the
/// user types is bound as text, and must be cast to the column's type (or the
/// column read as text for a pattern). Unsorted pages follow the key.
#[test]
fn postgres_filters_and_edits_typed_columns() {
    let Some(url) = env_url("TERMIDE_TEST_POSTGRES_URL") else {
        eprintln!("skip: set TERMIDE_TEST_POSTGRES_URL to run the PostgreSQL test");
        return;
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let exec = |sql: &'static str| {
        rt.block_on(async {
            let pool = sqlx::PgPool::connect(&url).await.unwrap();
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
            pool.close().await;
        })
    };
    exec(
        "DROP TABLE IF EXISTS termide_it_typed; DROP TYPE IF EXISTS termide_it_mood; \
         CREATE TYPE termide_it_mood AS ENUM ('ok', 'sad'); \
         CREATE TABLE termide_it_typed (id int PRIMARY KEY, at timestamp, d date, \
           m termide_it_mood, u uuid, ip inet, j jsonb); \
         INSERT INTO termide_it_typed \
           SELECT i, '2024-01-01'::timestamp + i * interval '1 day', '2024-01-01'::date + i, \
                  CASE WHEN i % 2 = 0 THEN 'ok' ELSE 'sad' END::termide_it_mood, \
                  ('a0eebc99-9c0b-4ef8-bb6d-6bb9bd38000' || i)::uuid, \
                  ('10.0.0.' || i)::inet, jsonb_build_object('i', i) \
           FROM generate_series(1, 6) i; \
         UPDATE termide_it_typed SET d = d WHERE id = 1;",
    );

    let conn = DbConnection::connect(&url).unwrap();
    let count = |column: &str, op: FilterOp, value: &str| {
        let filters = vec![Condition {
            column: column.into(),
            op,
            value: Some(DbValue::Text(value.into())),
        }];
        conn.count("termide_it_typed", filters)
            .recv()
            .unwrap()
            .unwrap()
    };
    assert_eq!(count("at", FilterOp::Gt, "2024-01-04"), 3);
    assert_eq!(count("d", FilterOp::Le, "2024-01-02"), 1);
    assert_eq!(count("m", FilterOp::Eq, "ok"), 3);
    assert_eq!(
        count("u", FilterOp::Eq, "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380002"),
        1
    );
    assert_eq!(count("ip", FilterOp::Ne, "10.0.0.1"), 5);
    assert_eq!(count("u", FilterOp::EndsWith, "3"), 1);
    assert_eq!(count("m", FilterOp::StartsWith, "S"), 3);
    assert_eq!(count("j", FilterOp::Contains, "\"i\": 5"), 1);

    let edit = |column: &str, value: DbValue| {
        conn.update_cell(
            "termide_it_typed",
            vec![("id".into(), DbValue::Int(2))],
            column,
            value,
        )
        .recv()
        .unwrap()
        .unwrap()
    };
    assert_eq!(edit("d", DbValue::Text("2030-05-05".into())), 1);
    assert_eq!(edit("m", DbValue::Text("sad".into())), 1);
    assert_eq!(edit("at", DbValue::Null), 1);
    assert_eq!(count("d", FilterOp::Eq, "2030-05-05"), 1);

    // Row 1 was rewritten last, so the heap holds it after the others.
    let page = conn
        .page(PageRequest {
            table: "termide_it_typed".into(),
            filters: vec![],
            order_by: vec![],
            limit: 3,
            offset: 0,
        })
        .recv()
        .unwrap()
        .unwrap();
    let ids: Vec<DbValue> = page.rows.iter().map(|r| r[0].clone()).collect();
    assert_eq!(ids, [1, 2, 3].map(DbValue::Int));
    drop(conn);

    exec("DROP TABLE termide_it_typed; DROP TYPE termide_it_mood;");
}

#[test]
fn mysql_browse_filter_sort() {
    let Some(url) = env_url("TERMIDE_TEST_MYSQL_URL") else {
        eprintln!("skip: set TERMIDE_TEST_MYSQL_URL to run the MySQL test");
        return;
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        sqlx::query("DROP TABLE IF EXISTS termide_it_users")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE termide_it_users (\
             id INT AUTO_INCREMENT PRIMARY KEY, name TEXT, score DOUBLE, active TINYINT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO termide_it_users (name, score, active) VALUES \
             ('alice', 1.5, 1), ('bob', 2.25, 0), ('carol', NULL, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    });

    let conn = DbConnection::connect(&url).unwrap();
    // MySQL has no real boolean — TINYINT maps to the Number category.
    assert_browse(&conn, false);
    drop(conn);

    rt.block_on(async {
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        sqlx::query("DROP TABLE termide_it_users")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    });
}

/// MySQL 8 reports several `information_schema` columns as binary strings
/// (`VARBINARY`, `LONGBLOB`), which sqlx refuses to decode as `String`. Every
/// catalog query must still come back populated rather than silently empty.
#[test]
fn mysql_catalog_decodes_information_schema() {
    let Some(url) = env_url("TERMIDE_TEST_MYSQL_URL") else {
        eprintln!("skip: set TERMIDE_TEST_MYSQL_URL to run the MySQL test");
        return;
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        sqlx::query("DROP TABLE IF EXISTS termide_it_catalog")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE termide_it_catalog (\
             id INT NOT NULL, part INT NOT NULL, label VARCHAR(50), \
             PRIMARY KEY (id, part))",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    });

    let conn = DbConnection::connect(&url).unwrap();

    let databases = conn.list_databases().recv().unwrap().unwrap();
    assert!(
        databases.iter().any(|d| d == "information_schema"),
        "databases should be listed, got {databases:?}"
    );

    let tables = conn.list_tables().recv().unwrap().unwrap();
    assert!(
        tables.iter().any(|t| t == "termide_it_catalog"),
        "table should be listed, got {tables:?}"
    );

    let cols = conn.columns("termide_it_catalog").recv().unwrap().unwrap();
    let described: Vec<(&str, TypeCategory, bool)> = cols
        .iter()
        .map(|c| (c.name.as_str(), c.category, c.nullable))
        .collect();
    assert_eq!(
        described,
        vec![
            ("id", TypeCategory::Number, false),
            ("part", TypeCategory::Number, false),
            ("label", TypeCategory::Text, true),
        ]
    );

    let key = conn
        .primary_key("termide_it_catalog")
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(key, vec!["id", "part"]);
    drop(conn);

    rt.block_on(async {
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        sqlx::query("DROP TABLE termide_it_catalog")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    });
}
