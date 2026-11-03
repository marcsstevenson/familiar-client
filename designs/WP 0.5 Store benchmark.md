# WP 0.5: Store and search at 500,000 messages

Run on 3 November 2026 in the cloud (Linux x86_64, 4 vCPUs, Intel Xeon @ 2.1 GHz, 15 GB RAM). Code is in `tools/synth` and `tools/store-bench`; raw numbers are in `tools/store-bench/results/`.

## Summary

The SQLite (WAL) + Tantivy design holds up at 500,000 messages. Every store operation behind the design doc's targets runs in single-digit milliseconds or less at the 95th percentile, mostly well under 1 ms. Three things need design decisions in Phase 1, and none of them threatens the targets if handled:

1. **Phrase search over very common words** misses the 100 ms search target (109 ms p95 warm, 230–680 ms with a cold disk cache) when every body is indexed. With the design's one-year body window it drops to 14 ms p95. Keep the window, and search newest segments first with early stop if phrase search must cover all history.
2. **Folder counts must be stored, not computed.** Recounting all folders takes 0.86 s. Stored counters take 0.03 ms. WP 1.5 should keep `total` and `unread` per folder, updated by the writer.
3. **One file per body wastes disk.** 500,000 small zstd files take 487 MB of data but 2.0 GB on disk, because each file uses at least one 4 KB block. Small bodies should go inline in SQLite or into pack files; only large bodies need their own file.

Scrolling should use keyset (cursor) paging rather than `OFFSET`, which is 18 times slower and grows with folder size. Both are within a frame at this size, so this is a design note rather than a risk.

## What was tested

**Synthetic mailbox (`tools/synth`).** A fixed seed always gives the same mailbox, on any platform, because the random generator is defined in the crate. 500,000 messages over 2006–2026 with volume growing over time; three accounts (60/25/15%) with 46, 14 and 12 folders; 290,392 conversations of 1–13 messages that span Inbox, Sent Items and user folders; 5,000 correspondents and a 40,000-word vocabulary, both Zipf-distributed; quoted replies; 75% HTML bodies; 12% with attachments; Graph-shaped IDs (152-character immutable IDs, 76-character conversation IDs). The largest folder holds 75,193 messages.

**Store prototype (`tools/store-bench`).**

- SQLite in WAL mode with `synchronous = NORMAL`: `accounts`, `folders` (with stored counters), `messages` (indexed by folder and time, conversation, Graph ID and Internet message ID), and a `folder_threads` summary table that the writer updates on every insert, so a threaded folder view is an index scan.
- Bodies as zstd (level 3) files named by their BLAKE3 hash.
- A Tantivy 0.27 index of subject, from, to/cc and body text, with fast fields for folder, account, date and attachments. The database is the source of truth; search returns IDs and rows are read from SQLite.

Two stores were built: **all bodies** (every body kept and indexed, the stress case) and **one-year window** (bodies kept and indexed for the last 365 days only, as the design specifies; 47,062 of 500,000 messages).

Each measurement is 1,000 runs after a warm-up (200 for the `OFFSET` and new-process cases, 10 for the full recount), with random folders, positions, messages and search terms. Times include decoding rows into Rust structs, and JSON where the surface returns JSON. They are the store's share of each target; GUI rendering is WP 0.4's job.

## Results against the design doc's targets

Times are milliseconds at the 95th percentile, warm cache, unless marked.

| Design target | Measurement | All bodies | 1-year window | Verdict |
| --- | --- | --- | --- | --- |
| Search first page < 100 ms | One common word, 50 newest hits | 6.9 | 3.0 | Met |
| | Mid-frequency word | 0.74 | 0.36 | Met |
| | Rare word | 0.51 | 0.28 | Met |
| | Two words (AND) | 3.1 | 0.91 | Met |
| | **Phrase of two very common words** | **109** | 14 | **Missed with all bodies** |
| | `from:` surname | 0.59 | 0.54 | Met |
| | `subject:` word | 0.31 | 0.30 | Met |
| | Word + folder | 0.97 | 0.38 | Met |
| | Word + has attachment | 5.2 | 2.9 | Met |
| | Word + date range (one year) | 7.9 | 3.9 | Met |
| | Common word by relevance (BM25) | 3.7 | 2.5 | Met |
| | Cold cache, new process, common word | 49–83 (10 runs) | | Met |
| | Cold cache, new process, common phrase | 230–677 (5 runs) | | Missed |
| Open a local message < 50 ms | Header row + read and decompress body | 0.05 | 0.05 | Met (store share) |
| `list --json` < 50 ms | New connection + 50 rows + JSON, in process | 1.0 | 0.74 | Met |
| | New process, warm cache | 4.3 | 4.1 | Met |
| | New process, cold cache | 20–26 (10 runs) | | Met |
| Cold start to usable inbox < 500 ms | Store share: open + first page, cold cache | 20–26 | | Met (store share; app start is WP 0.4) |
| Keypress < 16 ms | Folder first page, any folder | 0.12 | 0.15 | Met |
| | Largest folder first page | 0.09 | 0.08 | Met |
| | Threaded folder first page | 0.08 | 0.09 | Met |
| | Folder tree with stored counts | 0.03 | 0.03 | Met |
| | **Folder counts recomputed by scan** | **1,000** | 846 | **Must be stored, not computed** |
| Scrolling 500,000 at 60 fps (16 ms frame) | Keyset page at a random position | 0.27 | 0.20 | Met |
| | `OFFSET` page at a random position | 4.9 | 2.5 | Met at 75k; grows linearly with position |
| Thread assembly (no target) | Random message's conversation | 0.02 | 0.01 | Fast |
| | Largest conversation | 0.03 | 0.03 | Fast |
| Readers don't block the writer | Folder first page while writing ~3,100 msgs/s | 0.27 (vs 0.09 idle) | 0.28 | Met |
| Idle memory: daemon < 50 MB | Peak RSS of a CLI `list` / `search` process | 13 / 25 MB | | Plausible; daemon not built yet |

### Sizes

| | All bodies | 1-year window |
| --- | --- | --- |
| SQLite database | 695 MB (1.4 KB per message) | 674 MB |
| Tantivy index | 372 MB | 81 MB |
| Body files: data | 510 MB in 500,000 files | 48 MB in 47,062 files |
| Body files: on disk | **2,058 MB** | 194 MB |
| Raw body bytes before compression | 926 MB | 87 MB |

### Load time

Loading went through the same per-message path a sync writer would use, with indexes in place: 185 s for all bodies (about 2,700 messages a second) and 103 s for the one-year window. Of the all-bodies run, 70 s was SQLite, 70 s writing body files, 33 s generating mail and 9 s Tantivy. Real first sync is limited by Graph, not by this; Marc's 9,500-message Hotmail would load locally in about 4 s.

## Targets that look out of reach, and what to do

- **Phrase search over all history.** Positions for very common words are large, so a phrase of two of them reads a lot of the index. The one-year window fixes this for local search, and the design already sends older body search to Graph `$search`. If all-history local phrase search is wanted later, search time-ordered segments newest first and stop after 50 hits, or add a word-pair index. Recommend: keep the window, and add the cold-cache phrase case to the CI performance suite.
- **Cold-cache search.** The first search after boot reads the index from disk. 49–83 ms for a common word is within target, but the phrase case isn't. The daemon can warm the index in the background at start.

## Recommendations for Phase 1

- **WP 1.5 (store):** keep folder `total`/`unread` counters and the `folder_threads` table, maintained by the single writer. Store bodies under about 4 KB inline (or in pack files) instead of one file each. Consider mapping conversation IDs to integers: the 76-character ID is stored three times per message (row, index, thread table) and accounts for much of the 1.4 KB per message.
- **WP 1.9 (search):** Tantivy needs no special tuning for the targets with a one-year window. Store only the message ID in the index and read rows from SQLite, as here. Read state and flags change often, so filter those in SQLite, not in the index.
- **GUI list (Phase 3):** page with a `(received_at, id)` cursor, not `OFFSET`. For a scrollbar that jumps to any position, keep a sparse position-to-cursor map.
- **WP 1.7 (daemon memory):** the benchmark process reached 530 MB RSS, mostly memory-mapped database and index pages plus the benchmark's own buffers. The daemon should cap SQLite's `mmap_size` and `cache_size` and be measured on its own against the 50 MB target.

## Limits of this run

- **Hardware.** A cloud Linux VM, not Marc's Windows PC. Windows has slower small-file creation and Defender scans new files, which would make one-file-per-body worse. Re-run `store-bench load` and `bench` on Marc's machine before the Phase 0 gate if exact Windows numbers matter; `cold.sh` is Linux-only.
- **Synthetic mail.** Bodies average 1.9 KB, smaller than real HTML newsletters, and the vocabulary is invented words with a Zipf distribution, so search costs are indicative. No two bodies are identical, so content-hash de-duplication saved nothing here; real mail (newsletters, forwarded copies) will dedupe some.
- **Not measured here:** attachments, HTML sanitising and rendering, GUI frame times (WP 0.4), and index updates during search (Tantivy readers see a snapshot and reload after each commit).

## How to reproduce

```sh
cd tools
cargo build --release
./target/release/store-bench load  --data /tmp/m500k --messages 500000            # all bodies
./target/release/store-bench load  --data /tmp/m500k-1y --body-window-days 365    # one-year window
./target/release/store-bench bench --data /tmp/m500k --iters 1000 --out results.json
sudo ./store-bench/cold.sh /tmp/m500k 1 "$(./target/release/synth --vocab 1)"     # cold cache, Linux
```

The seed defaults to 42; `--seed` changes it.
