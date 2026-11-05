# Search

`fam mail search` searches every account on this device at once. Every header is local, so searching by sender, subject, date, folder, account, attachment or read state is instant and works offline. Message text is local only for the sync window (`sync.window`, one year by default); for older mail, `--scope all` also asks the server.

```sh
fam mail search 'from:club subject:"AGM" has:attachment after:2026-01-01'
fam mail search 'invoice before:2015-01-01' --scope all
fam mail search 'folder:inbox is:unread' --json
fam mail search 'newsletter' --after 1793000000.4512   # next page
```

## Query language

Every part must match. Put `-` in front of any part to exclude it instead.

| Write | Finds |
| --- | --- |
| `invoice`, `"annual meeting"` | the word or phrase in the subject, sender, recipients or message text |
| `from:ann`, `from:"Ann Smith"`, `from:ann@example.com` | the sender's name or address |
| `to:ann` | a To or Cc recipient |
| `subject:agm`, `subject:"annual meeting"` | the subject |
| `body:invoice` | the message text only |
| `has:attachment` | messages with file attachments |
| `after:2026-01-01` | received on or after that day |
| `before:2015-01-01` | received before that day |
| `folder:inbox`, `folder:"Club/Committee"`, `in:sent` | in a folder, by name, path or well-known name (`inbox`, `sent`, `drafts`, `deleted`, `junk`, `archive`) |
| `account:parklands`, `account:marc@example.com` | in an account, by its name in `config.toml` or its address |
| `is:unread`, `is:read`, `is:flagged` | by read or flag state |

Dates are `YYYY-MM-DD`, `YYYY-MM` or `YYYY`, in UTC, or an age such as `30d`, `2w`, `6m` or `1y` (`after:30d` is the last 30 days). Words match whole words, ignoring case and accents (`cafe` finds `café`). An operator the language doesn't know, such as `re:` in a pasted subject, is searched as text, and `AND` between parts is ignored. A malformed value (`is:urgent`, `after:yesterday`) is a usage error (exit 2).

Results are newest first, 50 to a page (`--limit` up to 500). `next` in the output is the token for `--after`.

## Online results

With `--scope all` the words, phrases and `from:`/`to:`/`subject:`/`body:` parts go to Graph `$search` through `famd`, which holds the tokens and queues the request ahead of background sync. Graph returns at most 1,000 results and can't combine `$search` with filters, so dates, folders, accounts, attachments and read state are applied locally to what comes back. Server results that the local index would find anyway are dropped; the rest are merged into the list and labelled `"source": "online"` (`(online)` in text output). Their headers are usually local too (`id` is set), but their text isn't (`body_local: false`).

`online` in the output says what happened:

| Value | Meaning |
| --- | --- |
| `not_requested` | `--scope local` (the default), or a later page |
| `not_needed` | The query has no words, only filters; every header is local |
| `searched` | The server was searched (a failure for one account is in `notices`) |
| `offline` | The server couldn't be reached; a notice says older message text wasn't searched |
| `daemon_unavailable` | `famd` isn't running, so the server wasn't searched |
| `failed` | The server search failed for every account; see `notices` |

With `--scope local` and a query that searches text, the last page carries a notice that text older than the window wasn't searched.

## How it works

- **The index** is Tantivy, in `search/` next to the store under the data directory. It holds the message ID, account, folder, received time and attachment flag, and the subject, sender, recipients and body text. Rows are always read from SQLite, which stays the source of truth.
- **Keeping it up to date.** Triggers on `messages` add a row to the store's `search_queue` table for every change the index cares about (new, deleted, moved, re-dated, a different subject, sender, recipients or attachment flag, a body arriving or ageing out), whoever made it. `famd` drains the queue every 2 seconds, up to 2,000 messages per commit, and deletes the rows it handled only after the index has committed them. A change made meanwhile gets a higher queue position and is picked up next time. While the queue isn't empty, search results carry a notice that the newest mail may be missing.
- **Read and flag state** change too often to re-index. `is:unread` and `is:flagged` take the matching IDs from small partial indexes in SQLite and hand them to Tantivy as a set; rows are checked again when read.
- **Rebuilds.** The index directory holds `familiar-index-version`. If it is missing, of another version or unreadable, `famd` recreates the index empty at start and queues every message.
- **Memory.** The index writer (15 MB, one thread) exists only while there is work, so an idle `famd` doesn't hold it. `fam` opens the index read-only per command.

## Speed

`crates/search/tests/scale.rs` loads the WP 0.5 synthetic mailbox (seed 42, 2006–2026, three accounts) through the store's writer with bodies for one year, indexes it from the queue and times each kind of query through `Searcher::search`, 50 newest hits with rows read from SQLite. CI runs 10,000 messages; the full run is:

```sh
cargo test --release -p familiar-search --test scale -- --ignored --nocapture
```

Results are in the development plan's results log (WP 1.9).
