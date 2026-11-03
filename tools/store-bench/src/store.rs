//! Prototype store: SQLite (WAL) metadata, zstd body files keyed by BLAKE3
//! content hash, and a Tantivy index. Loading goes through the same
//! per-message path a sync writer would use, so load time is a fair first
//! estimate of first-sync local cost (network excluded).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::schema::{Field, Schema, FAST, INDEXED, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter};

pub const SCHEMA: &str = r#"
CREATE TABLE accounts (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL,
    email TEXT NOT NULL
);
CREATE TABLE folders (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    parent_id  INTEGER REFERENCES folders(id),
    name       TEXT NOT NULL,
    well_known TEXT,
    total      INTEGER NOT NULL DEFAULT 0,
    unread     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE messages (
    id                  INTEGER PRIMARY KEY,
    account_id          INTEGER NOT NULL,
    folder_id           INTEGER NOT NULL,
    graph_id            TEXT NOT NULL,
    internet_message_id TEXT NOT NULL,
    conversation_id     TEXT NOT NULL,
    subject             TEXT NOT NULL,
    from_name           TEXT NOT NULL,
    from_addr           TEXT NOT NULL,
    to_json             TEXT NOT NULL,
    cc_json             TEXT NOT NULL,
    received_at         INTEGER NOT NULL,
    is_read             INTEGER NOT NULL,
    is_flagged          INTEGER NOT NULL,
    importance          INTEGER NOT NULL,
    has_attachments     INTEGER NOT NULL,
    attachments_json    TEXT NOT NULL,
    preview             TEXT NOT NULL,
    body_hash           BLOB,
    body_size           INTEGER
);
CREATE INDEX messages_folder_time  ON messages(folder_id, received_at DESC, id DESC);
CREATE INDEX messages_conversation ON messages(conversation_id, received_at);
CREATE UNIQUE INDEX messages_graph_id ON messages(account_id, graph_id);
CREATE INDEX messages_imid         ON messages(internet_message_id);

-- One row per conversation per folder, kept up to date by the writer, so a
-- threaded folder view is an index range scan rather than a GROUP BY.
CREATE TABLE folder_threads (
    folder_id        INTEGER NOT NULL,
    conversation_id  TEXT NOT NULL,
    last_received_at INTEGER NOT NULL,
    last_message_id  INTEGER NOT NULL,
    message_count    INTEGER NOT NULL,
    unread_count     INTEGER NOT NULL,
    PRIMARY KEY (folder_id, conversation_id)
) WITHOUT ROWID;
CREATE INDEX folder_threads_time ON folder_threads(folder_id, last_received_at DESC);
"#;

pub const INSERT_MESSAGE: &str = "INSERT INTO messages (id, account_id, folder_id, graph_id, internet_message_id,
    conversation_id, subject, from_name, from_addr, to_json, cc_json, received_at, is_read, is_flagged,
    importance, has_attachments, attachments_json, preview, body_hash, body_size)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)";

pub const UPSERT_THREAD: &str = "INSERT INTO folder_threads VALUES (?1, ?2, ?3, ?4, 1, ?5)
    ON CONFLICT (folder_id, conversation_id) DO UPDATE SET
      message_count    = message_count + 1,
      unread_count     = unread_count + excluded.unread_count,
      last_message_id  = CASE WHEN excluded.last_received_at >= last_received_at
                              THEN excluded.last_message_id ELSE last_message_id END,
      last_received_at = max(last_received_at, excluded.last_received_at)";

pub struct Paths {
    pub db: PathBuf,
    pub bodies: PathBuf,
    pub index: PathBuf,
}

impl Paths {
    pub fn new(data: &Path) -> Self {
        Paths {
            db: data.join("store.sqlite"),
            bodies: data.join("bodies"),
            index: data.join("index"),
        }
    }
}

pub fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA mmap_size = 268435456;
         PRAGMA cache_size = -32000;",
    )?;
    Ok(())
}

pub fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA mmap_size = 268435456; PRAGMA cache_size = -32000;")?;
    Ok(conn)
}

#[derive(Clone, Copy)]
pub struct SearchFields {
    pub msg_id: Field,
    pub subject: Field,
    pub from: Field,
    pub to: Field,
    pub body: Field,
    pub folder: Field,
    pub account: Field,
    pub received: Field,
    pub attachment: Field,
}

pub fn search_schema() -> (Schema, SearchFields) {
    let mut b = Schema::builder();
    let f = SearchFields {
        msg_id: b.add_u64_field("msg_id", INDEXED | FAST | STORED),
        subject: b.add_text_field("subject", TEXT),
        from: b.add_text_field("from", TEXT),
        to: b.add_text_field("to", TEXT),
        body: b.add_text_field("body", TEXT),
        folder: b.add_u64_field("folder", INDEXED | FAST),
        account: b.add_u64_field("account", INDEXED | FAST),
        received: b.add_i64_field("received", INDEXED | FAST),
        attachment: b.add_u64_field("attachment", INDEXED | FAST),
    };
    (b.build(), f)
}

pub fn fields_of(schema: &Schema) -> Result<SearchFields> {
    Ok(SearchFields {
        msg_id: schema.get_field("msg_id")?,
        subject: schema.get_field("subject")?,
        from: schema.get_field("from")?,
        to: schema.get_field("to")?,
        body: schema.get_field("body")?,
        folder: schema.get_field("folder")?,
        account: schema.get_field("account")?,
        received: schema.get_field("received")?,
        attachment: schema.get_field("attachment")?,
    })
}

pub fn body_path(root: &Path, hash: &[u8; 32]) -> PathBuf {
    let hex = blake3::Hash::from_bytes(*hash).to_hex();
    root.join(&hex[0..2])
        .join(&hex[2..4])
        .join(format!("{hex}.zst"))
}

#[derive(Default, serde::Serialize)]
pub struct LoadReport {
    pub messages: usize,
    pub seed: u64,
    pub total_secs: f64,
    pub generate_secs: f64,
    pub sqlite_secs: f64,
    pub bodies_secs: f64,
    pub tantivy_add_secs: f64,
    pub tantivy_commit_secs: f64,
    pub finalise_secs: f64,
    pub body_window_days: Option<i64>,
    pub bodies_kept: usize,
    pub unique_bodies: usize,
    pub raw_body_bytes: u64,
}

/// `body_window_days`: keep and index bodies only for mail this recent, as
/// the design's sync window does. `None` keeps every body (the stress case).
pub fn load(data: &Path, seed: u64, n: usize, body_window_days: Option<i64>) -> Result<LoadReport> {
    let window_start = body_window_days.map_or(i64::MIN, |d| synth::HISTORY_END - d * 86_400);
    let paths = Paths::new(data);
    if data.exists() {
        anyhow::bail!("{} already exists; remove it first", data.display());
    }
    std::fs::create_dir_all(&paths.bodies)?;
    std::fs::create_dir_all(&paths.index)?;

    let start = Instant::now();
    let mut rep = LoadReport {
        messages: n,
        seed,
        body_window_days,
        ..Default::default()
    };

    let mut conn = Connection::open(&paths.db)?;
    configure(&conn)?;
    conn.execute_batch(SCHEMA)?;

    let (schema, f) = search_schema();
    let index = Index::create_in_dir(&paths.index, schema)?;
    let mut writer: IndexWriter = index.writer(512 * 1024 * 1024)?;

    let mut gen = synth::Generator::new(seed, n);
    {
        let tx = conn.transaction()?;
        for a in gen.accounts() {
            tx.execute(
                "INSERT INTO accounts VALUES (?1, ?2, ?3)",
                params![a.id, a.name, a.email],
            )?;
        }
        for fo in gen.folders() {
            tx.execute(
                "INSERT INTO folders (id, account_id, parent_id, name, well_known) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![fo.id, fo.account_id, fo.parent_id, fo.name, fo.well_known],
            )?;
        }
        tx.commit()?;
    }

    let mut t_gen = Duration::ZERO;
    let mut t_sql = Duration::ZERO;
    let mut t_body = Duration::ZERO;
    let mut t_tan = Duration::ZERO;
    let mut done = 0usize;
    let batch = 5_000;

    while done < n {
        let tx = conn.transaction()?;
        {
            let mut ins = tx.prepare_cached(INSERT_MESSAGE)?;
            let mut thr = tx.prepare_cached(UPSERT_THREAD)?;
            for _ in 0..batch {
                let t0 = Instant::now();
                let Some(m) = gen.next() else { break };
                t_gen += t0.elapsed();

                let t0 = Instant::now();
                let bytes = m.body_html.as_deref().unwrap_or(&m.body_text).as_bytes();
                let keep_body = m.received_at >= window_start;
                let hash = keep_body.then(|| *blake3::hash(bytes).as_bytes());
                if let Some(hash) = &hash {
                    let path = body_path(&paths.bodies, hash);
                    if !path.exists() {
                        std::fs::create_dir_all(path.parent().unwrap())?;
                        std::fs::write(&path, zstd::bulk::compress(bytes, 3)?)?;
                        rep.unique_bodies += 1;
                    }
                    rep.bodies_kept += 1;
                    rep.raw_body_bytes += bytes.len() as u64;
                }
                t_body += t0.elapsed();

                let t0 = Instant::now();
                ins.execute(params![
                    m.id as i64,
                    m.account_id,
                    m.folder_id,
                    m.graph_id,
                    m.internet_message_id,
                    m.conversation_id,
                    m.subject,
                    m.from.name,
                    m.from.addr,
                    serde_json::to_string(&m.to)?,
                    serde_json::to_string(&m.cc)?,
                    m.received_at,
                    m.is_read,
                    m.is_flagged,
                    m.importance,
                    m.has_attachments(),
                    serde_json::to_string(&m.attachments)?,
                    m.preview,
                    hash.as_ref().map(|h| &h[..]),
                    bytes.len() as i64,
                ])?;
                thr.execute(params![
                    m.folder_id,
                    m.conversation_id,
                    m.received_at,
                    m.id as i64,
                    !m.is_read as i64
                ])?;
                t_sql += t0.elapsed();

                let t0 = Instant::now();
                let to: Vec<String> =
                    m.to.iter()
                        .chain(m.cc.iter())
                        .map(|a| format!("{} {}", a.name, a.addr))
                        .collect();
                writer.add_document(doc!(
                    f.msg_id => m.id,
                    f.subject => m.subject.as_str(),
                    f.from => format!("{} {}", m.from.name, m.from.addr),
                    f.to => to.join(" "),
                    f.body => if keep_body { m.body_text.as_str() } else { "" },
                    f.folder => m.folder_id as u64,
                    f.account => m.account_id as u64,
                    f.received => m.received_at,
                    f.attachment => m.has_attachments() as u64,
                ))?;
                t_tan += t0.elapsed();
                done += 1;
            }
        }
        let t0 = Instant::now();
        tx.commit()?;
        t_sql += t0.elapsed();
        if done.is_multiple_of(50_000) {
            eprintln!(
                "  loaded {done}/{n} ({:.0}s)",
                start.elapsed().as_secs_f64()
            );
        }
    }

    let t0 = Instant::now();
    writer.commit().context("tantivy commit")?;
    writer.wait_merging_threads()?;
    rep.tantivy_commit_secs = t0.elapsed().as_secs_f64();

    let t0 = Instant::now();
    conn.execute_batch(
        "UPDATE folders SET
           total  = (SELECT count(*) FROM messages m WHERE m.folder_id = folders.id),
           unread = (SELECT count(*) FROM messages m WHERE m.folder_id = folders.id AND m.is_read = 0);
         ANALYZE;
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    rep.finalise_secs = t0.elapsed().as_secs_f64();

    rep.generate_secs = t_gen.as_secs_f64();
    rep.sqlite_secs = t_sql.as_secs_f64();
    rep.bodies_secs = t_body.as_secs_f64();
    rep.tantivy_add_secs = t_tan.as_secs_f64();
    rep.total_secs = start.elapsed().as_secs_f64();
    Ok(rep)
}
