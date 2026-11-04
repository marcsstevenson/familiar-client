# Sync and the daemon

`famd` keeps a local copy of every account in `config.toml` and is the only process that writes to the store. `fam` and the desktop app read the store directly and send anything that changes it to `famd` over a local socket (named pipe on Windows).

## Running it

```sh
famd                 # foreground; Ctrl-C stops it
fam sync status      # what it is doing, per account
fam mail pin 1234    # keep message 1234's body on this device
```

`famd` reads `config.toml` (`FAMILIAR_CONFIG` or the default path from `fam config path`) and takes tokens from the sign-in store (`fam auth login`). An account that isn't signed in shows `auth_required` in `fam sync status` and is retried every few minutes. A second `famd` on the same data directory refuses to start.

| Variable | Meaning |
| --- | --- |
| `FAMILIAR_DATA_DIR` | Store (`store/`), socket (`famd.sock`) and token file. Default: the platform's local data directory plus `Familiar`. |
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
| Cached | Until the cache needs room (least recently used first) | Opening an older message (the `mail.fetch_body` request, which `fam mail fetch` (WP 1.10) and the app will use), or unpinning one outside the window |

`sync.cache_limit` bounds the cached bodies only; window and pinned bodies don't count against it. `sync.older_bodies = "none"` refuses on-demand downloads of bodies older than the window (pinning still downloads).

## The socket

JSON-RPC 2.0, one JSON object per line. The protocol types and a blocking client are in `familiar_core::daemon`.

| Method | Params | Result |
| --- | --- | --- |
| `daemon.status` | none | version, pid, start time, and per account: phase, last error, last round, next poll, headers and bodies this run, failures |
| `mail.pin` | `{"message": id, "pinned": bool}` | `{"message", "pinned", "has_body", "fetched"}` |
| `mail.fetch_body` | `{"message": id}` | `{"message", "fetched", "cached"}` |
| `sync.now` | `{"account": key}` (optional) | `{}` |
| `daemon.stop` | none | `{}`, then the daemon exits |

Errors carry the command error kind in `error.data.kind`, so a failure inside `famd` exits `fam` with the same code it would have locally (see [exit-codes.md](exit-codes.md)).

On Unix the socket is `<data dir>/famd.sock`, mode 0600. On Windows the pipe is `\\.\pipe\familiar-famd-<hash of the data dir>`, refuses remote clients and keeps the default pipe security (write access for its creator, SYSTEM and administrators only).

## Tests

`crates/daemon/tests/` runs the daemon against the mock Graph with poll intervals in milliseconds and a clock the test moves:

- `sync.rs`: first sync then a burst of new mail, moves, deletes and read changes; window ageing, pinning and the cache; the socket and `fam sync status`; restart without a full round.
- `large.rs`: 20,000 messages in CI; 500,000 with `cargo test --release -p familiar-daemon --test large -- --ignored --nocapture`.
