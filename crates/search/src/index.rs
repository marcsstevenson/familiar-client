// SPDX-License-Identifier: MIT OR Apache-2.0
//! The index on disk and the indexer that keeps it up to date.
//!
//! The index holds only what search needs: the message ID, folder, account,
//! received time and attachment flag as fast fields, and the subject,
//! sender, recipients and body text as tokenised text. Rows are always read
//! back from SQLite, which stays the source of truth (WP 0.5).
//!
//! `famd` owns the [`Indexer`]. It drains the store's `search_queue`, which
//! triggers fill on every message change the index cares about, so the
//! index follows whatever wrote the store. The CLI and GUI open a
//! [`crate::Searcher`] on the same directory; Tantivy lets readers in other
//! processes open the index while one writer commits.

use std::path::{Path, PathBuf};

use familiar_store::{Message, MessageId, Reader, StoreWriter};
use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, Schema, TextFieldIndexing, TextOptions,
};
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

use crate::text::body_text;
use crate::{Error, Result};

/// Bump when the schema or tokenising changes; an index with another
/// version is rebuilt from the store.
pub const INDEX_VERSION: u32 = 1;

/// The file in the index directory that holds [`INDEX_VERSION`].
const VERSION_FILE: &str = "familiar-index-version";

/// The tokenizer every text field uses: split on non-alphanumerics, drop
/// tokens over 40 bytes, lower-case, fold accents (so `cafe` finds `café`).
pub const TOKENIZER: &str = "familiar";

/// Messages read from the queue per index commit.
pub const BATCH: usize = 2_000;

/// Memory for the index writer while it runs. Tantivy's minimum; the writer
/// only exists while there is work, so the daemon stays small when idle.
const WRITER_HEAP: usize = 15_000_000;

/// The index's fields.
#[derive(Debug, Clone, Copy)]
pub struct Fields {
    /// Local message ID.
    pub id: Field,
    /// Local account ID.
    pub account: Field,
    /// Local folder ID.
    pub folder: Field,
    /// Received time, Unix seconds.
    pub received: Field,
    /// 1 if it has file attachments.
    pub attachment: Field,
    /// Subject.
    pub subject: Field,
    /// Sender name and address.
    pub from: Field,
    /// To and Cc names and addresses.
    pub to: Field,
    /// Body text (only while the body is on this device).
    pub body: Field,
}

pub(crate) fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let text = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let f = Fields {
        id: b.add_u64_field("id", INDEXED | FAST),
        account: b.add_u64_field("account", INDEXED | FAST),
        folder: b.add_u64_field("folder", INDEXED | FAST),
        received: b.add_i64_field("received", INDEXED | FAST),
        attachment: b.add_u64_field("attachment", INDEXED),
        subject: b.add_text_field("subject", text.clone()),
        from: b.add_text_field("from", text.clone()),
        to: b.add_text_field("to", text.clone()),
        body: b.add_text_field("body", text),
    };
    (b.build(), f)
}

pub(crate) fn register_tokenizer(index: &Index) {
    let a = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build();
    index.tokenizers().register(TOKENIZER, a);
}

fn version_of(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join(VERSION_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Opens an existing index of the current version. Errors with
/// [`Error::NotBuilt`] if there is none yet (or it needs a rebuild).
pub(crate) fn open_existing(dir: &Path) -> Result<(Index, Fields)> {
    if version_of(dir) != Some(INDEX_VERSION) {
        return Err(Error::NotBuilt(dir.to_path_buf()));
    }
    let index = Index::open_in_dir(dir).map_err(|e| match e {
        tantivy::TantivyError::OpenDirectoryError(_) | tantivy::TantivyError::OpenReadError(_) => {
            Error::NotBuilt(dir.to_path_buf())
        }
        e => Error::Index(e),
    })?;
    let (want, f) = schema();
    if index.schema() != want {
        return Err(Error::NotBuilt(dir.to_path_buf()));
    }
    register_tokenizer(&index);
    Ok((index, f))
}

/// Keeps the index in step with the store. Only `famd` has one.
pub struct Indexer {
    dir: PathBuf,
    index: Index,
    f: Fields,
}

impl std::fmt::Debug for Indexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Indexer").field("dir", &self.dir).finish()
    }
}

/// What [`Indexer::open`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// The index was there and current.
    Existing,
    /// It was missing, of another version or unreadable, and has been
    /// created empty. Every message must be queued again
    /// ([`familiar_store::WriteTx::requeue_all_for_search`]).
    Rebuilt,
}

impl Indexer {
    /// Opens the index in `dir`, or creates it empty (removing anything
    /// unusable there) and says so.
    pub fn open(dir: &Path) -> Result<(Self, Opened)> {
        match open_existing(dir) {
            Ok((index, f)) => {
                return Ok((
                    Self {
                        dir: dir.to_path_buf(),
                        index,
                        f,
                    },
                    Opened::Existing,
                ));
            }
            Err(Error::NotBuilt(_)) => {}
            Err(e) => return Err(e),
        }
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| Error::io("remove", dir, e))?;
        }
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
        let (schema, f) = schema();
        let index = Index::create_in_dir(dir, schema)?;
        register_tokenizer(&index);
        // Written last: an index without it is rebuilt next time.
        let v = dir.join(VERSION_FILE);
        std::fs::write(&v, INDEX_VERSION.to_string()).map_err(|e| Error::io("write", &v, e))?;
        Ok((
            Self {
                dir: dir.to_path_buf(),
                index,
                f,
            },
            Opened::Rebuilt,
        ))
    }

    /// Opens the index for `w`'s store in `dir`, and if it had to be
    /// rebuilt, queues every message for indexing.
    pub fn open_for<W: StoreWriter>(dir: &Path, w: &mut W) -> Result<(Self, Opened)> {
        let (ix, opened) = Self::open(dir)?;
        if opened == Opened::Rebuilt {
            w.write(|tx| tx.requeue_all_for_search())?;
        }
        Ok((ix, opened))
    }

    /// The index directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn document(&self, m: &Message, r: &Reader) -> Result<TantivyDocument> {
        let f = &self.f;
        let mut d = TantivyDocument::default();
        d.add_u64(f.id, m.id.0 as u64);
        d.add_u64(f.account, m.account.0 as u64);
        d.add_u64(f.folder, m.folder.0 as u64);
        d.add_i64(f.received, m.received_at);
        d.add_u64(f.attachment, u64::from(m.has_attachments));
        d.add_text(f.subject, &m.subject);
        d.add_text(f.from, format!("{} {}", m.from.name, m.from.addr));
        let mut to = String::new();
        for a in m.to.iter().chain(&m.cc) {
            to.push_str(&a.name);
            to.push(' ');
            to.push_str(&a.addr);
            to.push(' ');
        }
        d.add_text(f.to, to);
        if let Some(b) = &m.body {
            match r.body(&b.hash) {
                Ok(raw) => d.add_text(f.body, body_text(&raw, b.is_html)),
                // The body went between reading the row and the body (aged
                // out); the change is queued and the next batch fixes it.
                Err(familiar_store::Error::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(d)
    }

    /// Indexes everything waiting in the store's queue, in batches of
    /// [`BATCH`], committing each. `w` is the store's writer, used only to
    /// clear the queue entries once their batch is committed. Returns how
    /// many messages were (re)indexed or removed.
    ///
    /// This is CPU and disk work; the daemon runs it on a blocking thread.
    pub fn drain<W: StoreWriter>(&mut self, w: &mut W) -> Result<usize> {
        let mut total = 0;
        loop {
            let (n, more) = self.drain_some(w, usize::MAX)?;
            total += n;
            if !more {
                return Ok(total);
            }
        }
    }

    /// Like [`Indexer::drain`] but stops after `max_batches` batches, so a
    /// rebuild doesn't hold up shutdown. Returns how many messages were
    /// handled and whether more are waiting.
    pub fn drain_some<W: StoreWriter>(
        &mut self,
        w: &mut W,
        max_batches: usize,
    ) -> Result<(usize, bool)> {
        let r = Reader::open(&w.paths().root)?;
        let mut writer: Option<IndexWriter<TantivyDocument>> = None;
        let mut done = 0;
        let mut more = false;
        for i in 0.. {
            let batch = r.search_queue(BATCH)?;
            if batch.ids.is_empty() {
                break;
            }
            if i == max_batches {
                more = true;
                break;
            }
            let iw = match &mut writer {
                Some(iw) => iw,
                None => writer.insert(self.index.writer_with_num_threads(1, WRITER_HEAP)?),
            };
            let rows = r.messages_by_ids(&batch.ids)?;
            for id in &batch.ids {
                iw.delete_term(Term::from_field_u64(self.f.id, id.0 as u64));
            }
            for m in &rows {
                iw.add_document(self.document(m, &r)?)?;
            }
            iw.commit()?;
            w.write(|tx| tx.clear_search_queue(batch.upto))?;
            done += batch.ids.len();
        }
        if let Some(iw) = writer {
            // Let merges finish so the writer's memory and lock go now.
            iw.wait_merging_threads()?;
        }
        Ok((done, more))
    }

    /// Removes these messages from the index at once, without waiting for
    /// the queue. Used by tests.
    pub fn remove(&mut self, ids: &[MessageId]) -> Result<()> {
        let mut iw: IndexWriter<TantivyDocument> =
            self.index.writer_with_num_threads(1, WRITER_HEAP)?;
        for id in ids {
            iw.delete_term(Term::from_field_u64(self.f.id, id.0 as u64));
        }
        iw.commit()?;
        iw.wait_merging_threads()?;
        Ok(())
    }
}
