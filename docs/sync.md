# Sync and the daemon

`famd` keeps a local copy of every account in `config.toml` and is the only process that writes to the store. `fam` and the desktop app read the store directly and send anything that changes it to `famd` over a local socket (named pipe on Windows).

## Running it

```sh
famd                 # foreground; Ctrl-C stops it
fam sync status      # what it is doing, per account
fam mail pin 1234    # keep message 1234's body on this device
fam mail search 'from:club has:attachment'   # see docs/search.md
fam sync check       # compare the store with the server, report drift
```

`famd` reads `config.toml` (`FAMILIAR_CONFIG` or the default path from `fam config path`) and takes tokens from the sign-in store (`fam auth login`). An account that isn't signed in shows `auth_required` in `fam sync status` and is retried every few minutes. A second `famd` on the same data directory refuses to start.

| Variable | Meaning |
| --- | --- |
| `FAMILIAR_DATA_DIR` | Store (`store/`), search index (`search/`), socket (`famd.sock`) and token file. Default: `%USERPROFILE%\.familiar\data` on Windows (outside AppData, which Windows redirects for packaged apps such as the Claude desktop app), otherwise the platform's local data directory plus `Familiar`. |
| `FAMILIAR_CONFIG` | The config file. |
| `FAMILIAR_GRAPH_URL` | Another Graph, such as the mock (`cargo run -p familiar-mock-graph`). |

## What it does

For each account, one task works through these steps, re-checking what is due between each:

1. **Folder tree** by delta, at start and every 5 minutes.
2. **First sync: headers for all history.** One folder at a time: Inbox first (one page of its newest headers is stored before its delta round, so new mail shows at once), then Sent Items, Drafts, Archive, other folders, and Deleted Items and Junk last. Each folder's delta round lists every message without bodies. An interrupted round resumes from its saved `nextLink`. When every folder has finished a round, the first sync is recorded and `fam sync status` stops saying `(first sync)`.
3. **Polling.** Inbox every 30 seconds, every other folder every 5 minutes, all due folders in one call so a move between them is applied locally. Changes come from each folder's saved delta link.
4. **Bodies for the window**, newest first, 64 at a time, after any round that brought new or changed mail.
5. **Ageing**, at start and hourly: bodies older than the window are dropped unless pinned or cached, headers older than `sync.headers` are pruned (when it isn't `all`), the cache is trimmed to `sync.cache_limit`, and body files nothing refers to are deleted.

A failed step is recorded on the folder (shown under `errors` in `fam sync status`) and the account waits 30 seconds, doubling to 10 minutes, before trying again. Throttling is handled below this, by the provider's scheduler (at most 4 requests in flight per mailbox, `Retry-After` honoured mailbox-wide).

## Which bodies are kept

| Kind | Kept | How it gets there |
| --- | --- | --- |
| Window | While the message is newer than `sync.window` | The body backfill |
| Pinned | Always | `fam mail pin`; downloaded first if needed |
| Cached | Until the cache needs room (least recently used first) | Opening an older message (`fam mail fetch`, or the app, through the `mail.fetch_body` request), or unpinning one outside the window |

`sync.cache_limit` bounds the cached bodies only; window and pinned bodies don't count against it. `sync.older_bodies = "none"` refuses on-demand downloads of bodies older than the window (pinning still downloads).

## The socket

JSON-RPC 2.0, one JSON object per line. The protocol types and a blocking client are in `familiar_core::daemon`.

| Method | Params | Result |
| --- | --- | --- |
| `daemon.status` | none | version, pid, start time, and per account: phase, last error, last round, next poll, headers and bodies this run, failures |
| `mail.pin` | `{"message": id, "pinned": bool}` | `{"message", "pinned", "has_body", "fetched"}` |
| `mail.fetch_body` | `{"message": id}` | `{"message", "fetched", "cached"}` |
| `sync.now` | `{"account": key}` (optional) | `{}` |
| `sync.check` | `{"account", "limit", "no_settle"}` (all optional) | the check report, below |
| `graph.get` | `{"account", "path"}` (`account` optional with one account) | Graph's JSON response (`fam api`, GET only; see [cli-read.md](cli-read.md#fam-api)) |
| `daemon.stop` | none | `{}`, then the daemon exits |

Errors carry the command error kind in `error.data.kind`, so a failure inside `famd` exits `fam` with the same code it would have locally (see [exit-codes.md](exit-codes.md)).

On Unix the socket is `<data dir>/famd.sock`, mode 0600. On Windows the pipe is `\\.\pipe\familiar-famd-<hash of the data dir>`, refuses remote clients and keeps the default pipe security (write access for its creator, SYSTEM and administrators only).

## The consistency check

`fam sync check` asks `famd` to compare the store with the server and report every difference. It never fixes anything: drift means a sync bug, and the report is what to send. It exits 0 either way; `clean` in `--json` output says whether drift was found.

1. It asks for a sync round (as `sync.now` does) and waits until the account is idle again, bodies included.
2. It lists the server's folder tree and every message in every folder, 1,000 at a time with only the fields it compares, and compares them with the store one folder at a time.
3. If anything differed, it asks for another round, then fetches each differing message again (up to 2,000) and re-reads it from the store. Only what still differs is reported; the rest shows as `settled_on_recheck`. `--no-settle` skips both rounds.

| Kind | Meaning |
| --- | --- |
| `folder_missing`, `folder_extra`, `folder_changed` | A folder only on one side, or with a different name or parent |
| `message_missing`, `message_extra` | A message only on the server, or only in the store |
| `wrong_folder` | The store has the message in another folder |
| `read_state`, `flag`, `received_at` | The field differs |
| `body_missing` | Inside the sync window without a body |
| `body_outside_window` | A body more than a day (or two ageing runs) past the window that is neither pinned nor cached |
| `counter` | A folder or thread counter in the store disagrees with its messages |

Headers older than `sync.headers` are pruned on purpose and not compared. Before an account's first sync has finished the check runs without waiting for a round and says that differences are expected. `--limit` caps the list (default 50); the counts by kind are always complete.

## Conversations

Threads are built locally, not by asking Graph per folder. Every message row points at a `threads` row keyed by the account and Graph's `conversationId`, so a conversation is every message of that account sharing the ID, in whatever folder: Inbox, Sent Items, Drafts, custom folders and the trash. A move (local or reported by delta) changes only the folder, so the thread stays whole. Messages without a `conversationId` are their own thread.

`familiar_store::Reader` gives two views (WP 1.8, `crates/store/src/conversation.rs`):

- `conversation(thread)` and `conversation_of(message)`: the whole conversation oldest first, with each message's folder, `from_me` (sent from the account's address), `in_trash` (Deleted Items or Junk Email; `without_trash()` leaves those out), the subject without `RE:`/`FW:` prefixes, the participants and per-folder counts. Copies of one message (same Internet Message-ID, such as mail to yourself in both Inbox and Sent Items) are shown once: the copy outside the trash, then one with a body, then the oldest; it is unread if any copy is, and the others are listed under `copies`.
- `conversations(folder, after, limit)`: a threaded folder list, newest first by the folder's own newest message and keyset paged with `row.cursor()`. Each row has the folder's counts and the totals across all folders, plus the newest message anywhere, so an Inbox row shows a reply that is in Sent Items.

The read commands built on these views (`fam mail list`, `thread`, `fetch`, `fam folder list`) are described in [cli-read.md](cli-read.md).

## Tests

`crates/daemon/tests/` runs the daemon against the mock Graph with poll intervals in milliseconds and a clock the test moves:

- `sync.rs`: first sync then a burst of new mail, moves, deletes and read changes; window ageing, pinning and the cache; the socket and `fam sync status`; restart without a full round.
- `check.rs`: `sync.check` clean after a full sync and with changes in flight, and finding drift injected into the store (each kind, nothing fixed), including counter drift.
- `large.rs` (also runs `sync.check` after the first sync and after the burst): 20,000 messages in CI; 500,000 with `cargo test --release -p familiar-daemon --test large -- --ignored --nocapture`.
