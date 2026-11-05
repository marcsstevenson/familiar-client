// SPDX-License-Identifier: MIT OR Apache-2.0
//! Store errors.

use std::path::PathBuf;

/// A store result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong in the store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A file operation failed.
    #[error("{op} {path}: {source}")]
    Io {
        /// What was being done.
        op: &'static str,
        /// The file or directory.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },

    /// Another process holds the writer lock. Only `famd` may write.
    #[error("another process is already writing to the store at {0}")]
    WriterLocked(PathBuf),

    /// The store was written by a newer build.
    #[error("store schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew {
        /// Version in the database.
        found: u32,
        /// Newest version this build knows.
        supported: u32,
    },

    /// A reader found a store that the writer has not migrated yet.
    #[error(
        "store schema version {found} is older than this build ({supported}); start famd to migrate it"
    )]
    SchemaTooOld {
        /// Version in the database.
        found: u32,
        /// Version this build needs.
        supported: u32,
    },

    /// A reader was opened on a directory with no store in it.
    #[error("no store at {0}; start famd to create it")]
    NoStore(PathBuf),

    /// A body's bytes don't match its hash, or won't decompress.
    #[error("body {hash} is corrupt: {reason}")]
    CorruptBody {
        /// Hex hash of the body.
        hash: String,
        /// What was wrong.
        reason: String,
    },

    /// A row the caller named doesn't exist.
    #[error("{0} not found")]
    NotFound(String),

    /// The caller rolled its transaction back on purpose.
    #[error("transaction rolled back")]
    Aborted,

    /// A JSON column could not be encoded or decoded.
    #[error("JSON column: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            op,
            path: path.into(),
            source,
        }
    }
}
