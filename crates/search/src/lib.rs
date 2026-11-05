// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail search (WP 1.9): a Tantivy index beside the store, the query
//! language, and the pieces for merging in server results.
//!
//! - [`query`]: parses `from:club subject:"AGM" has:attachment after:2026-01-01`.
//! - [`Indexer`]: `famd` keeps the index in step with the store by draining
//!   the store's search queue.
//! - [`Searcher`]: the CLI and GUI run a [`Query`] against the index and read
//!   the rows from SQLite.
//! - [`online`]: older message text is searched on the server with Graph
//!   `$search`; this turns a query into KQL and applies the filters the
//!   server can't to what it returns.
//!
//! Guide: `docs/search.md`.

mod index;
pub mod online;
pub mod query;
mod searcher;
pub mod text;

use std::path::{Path, PathBuf};

pub use index::{BATCH, Fields, INDEX_VERSION, Indexer, Opened, TOKENIZER};
pub use online::{OnlineHit, to_kql};
pub use query::{ParseError, Query, TextField, TextTerm};
pub use searcher::{LocalPage, Searcher, account_ids_named, folder_ids_named};

/// The crate name, used by the workspace smoke tests.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Search errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `famd` hasn't built the index yet (or it is being rebuilt).
    #[error("the search index at {} isn't built yet; famd builds it", .0.display())]
    NotBuilt(PathBuf),
    /// The query could not be parsed.
    #[error("{0}")]
    Query(#[from] ParseError),
    /// Tantivy failed.
    #[error("search index: {0}")]
    Index(#[from] tantivy::TantivyError),
    /// The store failed.
    #[error("store: {0}")]
    Store(familiar_store::Error),
    /// A file operation failed.
    #[error("{op} {}: {source}", path.display())]
    Io {
        /// What was being done.
        op: &'static str,
        /// To what.
        path: PathBuf,
        /// Why it failed.
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn io(op: &'static str, path: &Path, source: std::io::Error) -> Self {
        Error::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Search results.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_set() {
        assert_eq!(super::NAME, "familiar-search");
    }
}
