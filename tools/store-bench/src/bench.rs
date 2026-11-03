//! The WP 0.5 measurements. Each one is timed end to end as the caller would
//! see it: query plus decoding into Rust structs (and JSON where the surface
//! returns JSON). Nothing here talks to a GUI, so the numbers are the store's
//! share of each design-doc target, not the whole of it.

use crate::store::{self, Paths};
use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use synth::Rng;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::{DocAddress, Index, Order, Searcher};

#[derive(Serialize, Clone)]
pub struct Stat {
    pub name: String,
    pub n: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub note: String,
}

fn stat(name: &str, note: &str, mut v: Vec<Duration>) -> Stat {
    v.sort();
    let q = |p: f64| {
        let i = ((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1;
        v[i].as_secs_f64() * 1e3
    };
    let s = Stat {
        name: name.into(),
        n: v.len(),
        p50_ms: q(0.50),
        p95_ms: q(0.95),
        p99_ms: q(0.99),
        max_ms: v.last().unwrap().as_secs_f64() * 1e3,
        note: note.into(),
    };
    eprintln!(
        "  {:<34} p50 {:>8.3}  p95 {:>8.3}  p99 {:>8.3}  max {:>8.3} ms  (n={})",
        s.name, s.p50_ms, s.p95_ms, s.p99_ms, s.max_ms, s.n
    );
    s
}

fn time<F: FnMut(&mut Rng) -> Result<()>>(
    rng: &mut Rng,
    iters: usize,
    mut f: F,
) -> Result<Vec<Duration>> {
    for _ in 0..(iters / 10).max(3) {
        f(rng)?; // warm-up, not recorded
    }
    let mut out = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f(rng)?;
        out.push(t.elapsed());
    }
    Ok(out)
}

#[derive(Serialize, Debug)]
pub struct Row {
    pub id: i64,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub received_at: i64,
    pub is_read: bool,
    pub is_flagged: bool,
    pub has_attachments: bool,
    pub preview: String,
}

pub const ROW_COLS: &str =
    "id, subject, from_name, from_addr, received_at, is_read, is_flagged, has_attachments, preview";

pub fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        subject: r.get(1)?,
        from_name: r.get(2)?,
        from_addr: r.get(3)?,
        received_at: r.get(4)?,
        is_read: r.get(5)?,
        is_flagged: r.get(6)?,
        has_attachments: r.get(7)?,
        preview: r.get(8)?,
    })
}

pub fn folder_page(conn: &Connection, folder: i64, limit: i64) -> Result<Vec<Row>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {ROW_COLS} FROM messages WHERE folder_id = ?1
         ORDER BY received_at DESC, id DESC LIMIT ?2"
    ))?;
    let rows = st
        .query_map(params![folder, limit], row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

#[derive(Serialize)]
pub struct Sizes {
    pub db_bytes: u64,
    pub wal_bytes: u64,
    pub index_bytes: u64,
    pub bodies_apparent_bytes: u64,
    pub bodies_on_disk_bytes: u64,
    pub body_files: u64,
}

fn dir_size(p: &Path) -> (u64, u64, u64) {
    let (mut apparent, mut disk, mut files) = (0, 0, 0);
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let md = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if md.is_dir() {
                stack.push(e.path());
            } else {
                files += 1;
                apparent += md.len();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    disk += md.blocks() * 512;
                }
                #[cfg(not(unix))]
                {
                    disk += md.len().div_ceil(4096) * 4096;
                }
            }
        }
    }
    (apparent, disk, files)
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

#[derive(Serialize)]
pub struct Memory {
    pub rss_mb: f64,
    pub peak_rss_mb: f64,
}

fn memory() -> Option<Memory> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = |key: &str| -> Option<f64> {
        s.lines()
            .find(|l| l.starts_with(key))?
            .split_whitespace()
            .nth(1)?
            .parse::<f64>()
            .ok()
    };
    Some(Memory {
        rss_mb: kb("VmRSS:")? / 1024.0,
        peak_rss_mb: kb("VmHWM:")? / 1024.0,
    })
}

#[derive(Serialize)]
pub struct Report {
    pub messages: i64,
    pub folders: i64,
    pub conversations: i64,
    pub largest_folder: (i64, i64),
    pub largest_thread: i64,
    pub stats: Vec<Stat>,
    pub sizes: Sizes,
    pub memory_after_queries: Option<Memory>,
    pub host: String,
}

pub struct Search {
    searcher: Searcher,
    parser: QueryParser,
    msg_id_cols: Vec<tantivy::columnar::Column<u64>>,
}

impl Search {
    pub fn open(paths: &Paths) -> Result<Self> {
        let index = Index::open_in_dir(&paths.index)?;
        let f = store::fields_of(&index.schema())?;
        let reader = index.reader()?;
        let searcher = reader.searcher();
        let mut parser = QueryParser::for_index(&index, vec![f.subject, f.from, f.to, f.body]);
        parser.set_conjunction_by_default();
        let msg_id_cols = searcher
            .segment_readers()
            .iter()
            .map(|s| s.fast_fields().u64("msg_id"))
            .collect::<tantivy::Result<Vec<_>>>()?;
        Ok(Search {
            searcher,
            parser,
            msg_id_cols,
        })
    }

    fn id_of(&self, a: DocAddress) -> i64 {
        self.msg_id_cols[a.segment_ord as usize]
            .first(a.doc_id)
            .unwrap_or(0) as i64
    }

    /// First page of results, newest first, hydrated from SQLite.
    pub fn by_date(&self, conn: &Connection, q: &str, limit: usize) -> Result<Vec<Row>> {
        let query = self.parser.parse_query(q)?;
        let top = self.searcher.search(
            &query,
            &TopDocs::with_limit(limit).order_by_fast_field::<i64>("received", Order::Desc),
        )?;
        hydrate(conn, top.into_iter().map(|(_, a)| self.id_of(a)))
    }

    /// First page by relevance (BM25), hydrated from SQLite.
    fn by_score(&self, conn: &Connection, q: &str, limit: usize) -> Result<Vec<Row>> {
        let query = self.parser.parse_query(q)?;
        let top = self
            .searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())?;
        hydrate(conn, top.into_iter().map(|(_, a)| self.id_of(a)))
    }
}

fn hydrate(conn: &Connection, ids: impl Iterator<Item = i64>) -> Result<Vec<Row>> {
    let mut st = conn.prepare_cached(&format!("SELECT {ROW_COLS} FROM messages WHERE id = ?1"))?;
    let mut out = Vec::new();
    for id in ids {
        out.push(st.query_row([id], row)?);
    }
    Ok(out)
}

pub fn run(data: &Path, seed: u64, iters: usize, exe: &Path) -> Result<Report> {
    let paths = Paths::new(data);
    let conn = store::open_reader(&paths.db)?;
    let gen = synth::Generator::new(seed, 0);
    let vocab: Vec<String> = gen.vocab().to_vec();
    let contacts = gen.contacts().to_vec();
    let mut rng = Rng::new(seed ^ 0x5eed);
    let mut stats = Vec::new();

    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    let messages = count("SELECT count(*) FROM messages")?;
    let folders_n = count("SELECT count(*) FROM folders")?;
    let conversations = count("SELECT count(DISTINCT conversation_id) FROM messages")?;
    let largest_folder: (i64, i64) = conn.query_row(
        "SELECT id, total FROM folders ORDER BY total DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (largest_conv, largest_thread): (String, i64) = conn.query_row(
        "SELECT conversation_id, count(*) c FROM messages GROUP BY conversation_id ORDER BY c DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let nonempty: Vec<i64> = conn
        .prepare("SELECT id FROM folders WHERE total > 0")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    eprintln!(
        "store: {messages} messages, {folders_n} folders ({} non-empty), {conversations} conversations, \
         largest folder {} has {}, largest thread {largest_thread}",
        nonempty.len(),
        largest_folder.0,
        largest_folder.1
    );
    let (inbox, inbox_total) = largest_folder;

    // --- Folder views -----------------------------------------------------
    stats.push(stat(
        "folder first page (any folder)",
        "50 rows, newest first, random non-empty folder",
        time(&mut rng, iters, |rng| {
            let f = *rng.pick(&nonempty);
            folder_page(&conn, f, 50)?;
            Ok(())
        })?,
    ));
    stats.push(stat(
        "folder first page (largest)",
        "50 rows, newest first, the largest folder",
        time(&mut rng, iters, |_| {
            folder_page(&conn, inbox, 50)?;
            Ok(())
        })?,
    ));

    let cursors: Vec<(i64, i64)> = conn
        .prepare("SELECT received_at, id FROM messages WHERE folder_id = ?1 ORDER BY random() LIMIT 2000")?
        .query_map([inbox], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    stats.push(stat(
        "folder page deep (keyset)",
        "50 rows after a random position in the largest folder, by (received_at, id) cursor",
        time(&mut rng, iters, |rng| {
            let (t, id) = *rng.pick(&cursors);
            let mut st = conn.prepare_cached(&format!(
                "SELECT {ROW_COLS} FROM messages WHERE folder_id = ?1 AND (received_at, id) < (?2, ?3)
                 ORDER BY received_at DESC, id DESC LIMIT 50"
            ))?;
            let _: Vec<Row> = st.query_map(params![inbox, t, id], row)?.collect::<rusqlite::Result<_>>()?;
            Ok(())
        })?,
    ));
    stats.push(stat(
        "folder page deep (OFFSET)",
        "50 rows at a random OFFSET in the largest folder; what a naive virtual list would do",
        time(&mut rng, iters / 5, |rng| {
            let off = rng.below(inbox_total as u64) as i64;
            let mut st = conn.prepare_cached(&format!(
                "SELECT {ROW_COLS} FROM messages WHERE folder_id = ?1
                 ORDER BY received_at DESC, id DESC LIMIT 50 OFFSET ?2"
            ))?;
            let _: Vec<Row> = st
                .query_map(params![inbox, off], row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(())
        })?,
    ));
    stats.push(stat(
        "folder page deep (OFFSET, ids only)",
        "same, but the OFFSET walks only the covering index and rows are fetched by id",
        time(&mut rng, iters / 5, |rng| {
            let off = rng.below(inbox_total as u64) as i64;
            let mut st = conn.prepare_cached(
                "SELECT id FROM messages WHERE folder_id = ?1
                 ORDER BY received_at DESC, id DESC LIMIT 50 OFFSET ?2",
            )?;
            let ids: Vec<i64> = st
                .query_map(params![inbox, off], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            hydrate(&conn, ids.into_iter())?;
            Ok(())
        })?,
    ));
    stats.push(stat(
        "threaded folder first page",
        "50 conversations, newest first, from the folder_threads summary table, largest folder",
        time(&mut rng, iters, |_| {
            let mut st = conn.prepare_cached(
                "SELECT t.conversation_id, t.message_count, t.unread_count, m.subject, m.from_name, m.received_at, m.preview
                 FROM folder_threads t JOIN messages m ON m.id = t.last_message_id
                 WHERE t.folder_id = ?1 ORDER BY t.last_received_at DESC LIMIT 50",
            )?;
            let n = st.query_map([inbox], |r| r.get::<_, String>(0))?.count();
            assert!(n > 0);
            Ok(())
        })?,
    ));
    stats.push(stat(
        "folder tree with counts",
        "all folders with stored total/unread counters",
        time(&mut rng, iters, |_| {
            let mut st = conn.prepare_cached(
                "SELECT id, account_id, parent_id, name, well_known, total, unread FROM folders ORDER BY account_id, id",
            )?;
            let _ = st.query_map([], |r| r.get::<_, i64>(0))?.count();
            Ok(())
        })?,
    ));
    stats.push(stat(
        "folder counts recomputed (scan)",
        "GROUP BY over all messages; shows why counters must be stored",
        time(&mut rng, 10, |_| {
            let mut st = conn.prepare_cached(
                "SELECT folder_id, count(*), sum(is_read = 0) FROM messages GROUP BY folder_id",
            )?;
            let _ = st.query_map([], |r| r.get::<_, i64>(0))?.count();
            Ok(())
        })?,
    ));

    // --- Threads and messages --------------------------------------------
    stats.push(stat(
        "thread assembly",
        "all messages of a random message's conversation, across folders, oldest first",
        time(&mut rng, iters, |rng| {
            let id = rng.range(1, messages as u64) as i64;
            let mut st = conn.prepare_cached(&format!(
                "SELECT {ROW_COLS}, folder_id FROM messages
                 WHERE conversation_id = (SELECT conversation_id FROM messages WHERE id = ?1)
                 ORDER BY received_at"
            ))?;
            let n = st.query_map([id], row)?.count();
            assert!(n > 0);
            Ok(())
        })?,
    ));
    stats.push(stat(
        "thread assembly (largest)",
        "the largest conversation in the store",
        time(&mut rng, iters, |_| {
            let mut st = conn.prepare_cached(&format!(
                "SELECT {ROW_COLS}, folder_id FROM messages WHERE conversation_id = ?1 ORDER BY received_at"
            ))?;
            let _ = st.query_map([&largest_conv], row)?.count();
            Ok(())
        })?,
    ));
    let with_body: Vec<i64> = conn
        .prepare(
            "SELECT id FROM messages WHERE body_hash IS NOT NULL ORDER BY random() LIMIT 5000",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    stats.push(stat(
        "open message (local body)",
        "header row + read and decompress the body file",
        time(&mut rng, iters, |rng| {
            let id = *rng.pick(&with_body);
            let mut st = conn.prepare_cached(&format!(
                "SELECT {ROW_COLS}, to_json, cc_json, attachments_json, body_hash FROM messages WHERE id = ?1"
            ))?;
            let (_r, hash): (Row, Vec<u8>) = st.query_row([id], |r| Ok((row(r)?, r.get(12)?)))?;
            let hash: [u8; 32] = hash.try_into().unwrap();
            let raw = std::fs::read(store::body_path(&paths.bodies, &hash))?;
            let body = zstd::stream::decode_all(&raw[..])?;
            assert!(!body.is_empty());
            Ok(())
        })?,
    ));

    // --- Search ------------------------------------------------------------
    let search = Search::open(&paths)?;
    let last_name = |rng: &mut Rng| -> String {
        // Weighted towards frequent correspondents, like real searches.
        let i = (rng.f64().powi(3) * contacts.len() as f64) as usize;
        contacts[i]
            .name
            .split_whitespace()
            .last()
            .unwrap()
            .to_lowercase()
    };
    let word = |rng: &mut Rng, lo: usize, hi: usize| {
        vocab[rng.range(lo as u64, hi as u64) as usize].clone()
    };
    type Gen<'a> = Box<dyn Fn(&mut Rng) -> String + 'a>;
    let cases: Vec<(&str, &str, Gen)> = vec![
        (
            "search: common word",
            "one word from frequency ranks 0-50 (matches a large share of mail)",
            Box::new(|r: &mut Rng| word(r, 0, 50)),
        ),
        (
            "search: mid word",
            "one word from ranks 500-3000",
            Box::new(|r: &mut Rng| word(r, 500, 3000)),
        ),
        (
            "search: rare word",
            "one word from ranks 10k-39k",
            Box::new(|r: &mut Rng| word(r, 10_000, 39_999)),
        ),
        (
            "search: two words (AND)",
            "common + mid word",
            Box::new(|r: &mut Rng| format!("{} {}", word(r, 0, 50), word(r, 500, 3000))),
        ),
        (
            "search: phrase",
            "two common words as a phrase",
            Box::new(|r: &mut Rng| format!("\"{} {}\"", word(r, 0, 20), word(r, 0, 20))),
        ),
        (
            "search: from:",
            "from:<surname>, frequent correspondents more likely",
            Box::new(|r: &mut Rng| format!("from:{}", last_name(r))),
        ),
        (
            "search: subject:",
            "subject:<mid word>",
            Box::new(|r: &mut Rng| format!("subject:{}", word(r, 200, 3000))),
        ),
        (
            "search: word + folder",
            "mid word restricted to the largest folder",
            Box::new(|r: &mut Rng| format!("{} AND folder:{}", word(r, 500, 3000), inbox)),
        ),
        (
            "search: word + has:attachment",
            "common word AND attachment",
            Box::new(|r: &mut Rng| format!("{} AND attachment:1", word(r, 0, 50))),
        ),
        (
            "search: word + date range",
            "common word in a random year",
            Box::new(|r: &mut Rng| {
                let y = r.range(0, 19) as i64;
                let a = synth::HISTORY_START + y * 31_557_600;
                format!(
                    "{} AND received:[{} TO {}]",
                    word(r, 0, 50),
                    a,
                    a + 31_557_600
                )
            }),
        ),
    ];
    for (name, note, q) in &cases {
        stats.push(stat(
            name,
            &format!("{note}; 50 newest hits, hydrated"),
            time(&mut rng, iters, |rng| {
                search.by_date(&conn, &q(rng), 50)?;
                Ok(())
            })?,
        ));
    }
    stats.push(stat(
        "search: common word by relevance",
        "BM25 top 50 instead of newest first",
        time(&mut rng, iters, |rng| {
            search.by_score(&conn, &word(rng, 0, 50), 50)?;
            Ok(())
        })?,
    ));

    // --- CLI-shaped list ----------------------------------------------------
    stats.push(stat(
        "list --json (in process, new conn)",
        "open a new SQLite connection, first 50 rows of the largest folder, serialise to JSON",
        time(&mut rng, iters, |_| {
            let c = store::open_reader(&paths.db)?;
            let rows = folder_page(&c, inbox, 50)?;
            let s = serde_json::to_string(&rows)?;
            assert!(s.len() > 2);
            Ok(())
        })?,
    ));
    stats.push(stat(
        "list --json (new process)",
        "spawn `store-bench list --json`, wait for all output; a cold CLI invocation",
        time(&mut rng, (iters / 5).max(20), |_| {
            let out = std::process::Command::new(exe)
                .args(["list", "--data"])
                .arg(data)
                .args(["--folder", &inbox.to_string(), "--json"])
                .output()?;
            assert!(out.status.success() && out.stdout.len() > 2);
            Ok(())
        })?,
    ));

    // --- Reads while the writer works ---------------------------------------
    stats.push(contention(&paths, inbox, iters, messages)?);

    let (idx, _, _) = dir_size(&paths.index);
    let (ba, bd, bf) = dir_size(&paths.bodies);
    let mut wal = paths.db.clone().into_os_string();
    wal.push("-wal");
    let sizes = Sizes {
        db_bytes: file_len(&paths.db),
        wal_bytes: file_len(Path::new(&wal)),
        index_bytes: idx,
        bodies_apparent_bytes: ba,
        bodies_on_disk_bytes: bd,
        body_files: bf,
    };
    Ok(Report {
        messages,
        folders: folders_n,
        conversations,
        largest_folder,
        largest_thread,
        stats,
        sizes,
        memory_after_queries: memory(),
        host: host(),
    })
}

/// Folder first-page latency while another connection writes: new messages
/// in 100-row transactions plus read-flag flips, as fast as it can.
fn contention(paths: &Paths, inbox: i64, iters: usize, messages: i64) -> Result<Stat> {
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let db = paths.db.clone();
    let (s2, w2) = (stop.clone(), written.clone());
    let writer = std::thread::spawn(move || -> Result<()> {
        let mut conn = Connection::open(&db)?;
        store::configure(&conn)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let mut gen = synth::Generator::new(0xC0FFEE, usize::MAX);
        let mut rng = Rng::new(9);
        let mut next_id: i64 =
            conn.query_row("SELECT max(id) + 1 FROM messages", [], |r| r.get(0))?;
        while !s2.load(Ordering::Relaxed) {
            let tx = conn.transaction()?;
            {
                let mut ins = tx.prepare_cached(store::INSERT_MESSAGE)?;
                let mut flip =
                    tx.prepare_cached("UPDATE messages SET is_read = 1 - is_read WHERE id = ?1")?;
                for _ in 0..100 {
                    let m = gen.next().unwrap();
                    ins.execute(params![
                        next_id,
                        m.account_id,
                        inbox,
                        format!("{}-{next_id}", m.graph_id),
                        m.internet_message_id,
                        m.conversation_id,
                        m.subject,
                        m.from.name,
                        m.from.addr,
                        "[]",
                        "[]",
                        synth::HISTORY_END + next_id,
                        m.is_read,
                        m.is_flagged,
                        m.importance,
                        m.has_attachments(),
                        "[]",
                        m.preview,
                        None::<Vec<u8>>,
                        None::<i64>,
                    ])?;
                    next_id += 1;
                    flip.execute([rng.range(1, messages as u64) as i64])?;
                }
            }
            tx.commit()?;
            w2.fetch_add(100, Ordering::Relaxed);
        }
        Ok(())
    });
    let conn = store::open_reader(&paths.db)?;
    // Wait until the writer is committing, then sample every 2 ms so the
    // reads overlap many write transactions and checkpoints.
    while written.load(Ordering::Relaxed) < 1_000 {
        std::thread::sleep(Duration::from_millis(5));
    }
    let w0 = written.load(Ordering::Relaxed);
    let t0 = Instant::now();
    let mut v = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        folder_page(&conn, inbox, 50)?;
        v.push(t.elapsed());
        std::thread::sleep(Duration::from_millis(2));
    }
    let secs = t0.elapsed().as_secs_f64();
    let w1 = written.load(Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap()?;
    let n = w1 - w0;
    // Put the store back so the benchmark can be re-run on the same data.
    let c = Connection::open(&paths.db)?;
    c.execute("DELETE FROM messages WHERE id > ?1", [messages])?;
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(stat(
        "folder first page under write load",
        &format!(
            "largest folder while a writer inserts and flags ({:.0} msgs/s written during the run)",
            n as f64 / secs
        ),
        v,
    ))
}

fn host() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    format!(
        "{} {}, {} logical CPUs, {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        cpu
    )
}
