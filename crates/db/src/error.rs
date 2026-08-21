//! Error type for database access.

/// Errors surfaced by [`crate::DbConnection`] and the engine layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// URL scheme isn't one of the supported engines.
    #[error("unsupported database URL scheme: {0:?}")]
    UnsupportedScheme(String),

    /// The background runtime could not be created.
    #[error("failed to start database runtime: {0}")]
    Runtime(String),

    /// The worker thread/connection is gone.
    #[error("database connection closed")]
    Closed,

    /// The request cannot be fulfilled against this table — editing a row in a
    /// table with no primary key, for instance. Carries a message meant for
    /// the user, not a driver failure.
    #[error("{0}")]
    Rejected(String),

    /// Anything coming from the underlying driver (connect, query, decode).
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl DbError {
    /// Whether this error is an authentication failure — the signal for the
    /// lazy password prompt. Matches by SQLSTATE: `28P01` (invalid_password)
    /// and `28000` (invalid_authorization_specification), used by both
    /// PostgreSQL and MySQL for access-denied / bad-password. SQLite never
    /// authenticates, so it never produces these.
    pub fn is_auth(&self) -> bool {
        match self {
            DbError::Sqlx(sqlx::Error::Database(db)) => {
                matches!(db.code().as_deref(), Some("28P01") | Some("28000"))
            }
            _ => false,
        }
    }
}
