# Reading mail from the command line

The Phase 1 read commands (WP 1.10). Each one is a registry command, so it has `--json` output matching its schema in `fam schema --json`, the same exit codes as every other command ([exit-codes.md](exit-codes.md)), an MCP tool and a TypeScript binding.

| Command | What it does | Needs famd |
| --- | --- | --- |
| `fam folder list` | Every folder, as a tree, with counts | No |
| `fam mail list` | A folder's messages, newest first | No |
| `fam mail search` | Search every account; see [search.md](search.md) | Only for `--scope all` |
| `fam mail thread <id>` | A whole conversation across folders | No |
| `fam mail fetch <id>` | A message with its text | Only to download a body that isn't local |
| `fam mail pin <id>` | Keep a body on this device; see [sync.md](sync.md) | Yes |
| `fam mail attachments <id>` | List a message's attachments, or save them with `--save DIR` | Yes |
| `fam api <path>` | GET any Microsoft Graph path with an account's token | Yes |

The commands that don't need famd read the local store directly, so they work offline and while famd is stopped. Before anything has synced they exit 6 (`not_found`). IDs are local message IDs, as shown by `list`, `search` and `thread`.

## fam folder list

```sh
fam folder list
fam folder list --account parklands --json
```

Folders by account, each account's tree depth first with siblings by name. Each row has `id`, `account`, `name`, `path` (such as `Inbox/Receipts`), `parent`, `depth`, `well_known` (`inbox`, `sentitems`, `drafts`, `deleteditems`, `archive`, `junkemail`, ...), `total`, `unread` and `remote_id`.

## fam mail list

```sh
fam mail list                                   # every account's Inbox, merged
fam mail list --account parklands --unread --json
fam mail list --folder "Cricket Club/Juniors"
fam mail list --folder sentitems --flagged --limit 20
fam mail list --after 1793000000.4512           # next page
```

`--folder` takes a path (`Inbox/Receipts`), a well-known name (`sentitems`) or a plain name (`Receipts`), ignoring case, and defaults to the Inbox. When two folders in one account share the name, the command says so and lists their paths (exit 2). Without `--account` the folder is listed in every account that has it, merged newest first. `--account` takes the account's name in `config.toml` or its address.

The output has `folders` (what was listed, with counts), `messages` and, when there are more, `next`: pass it as `--after`. Pages are keyset paged, so new mail arriving between pages doesn't shift or repeat rows.

Each message (the same header shape is used by `thread` and `fetch`):

| Field | |
| --- | --- |
| `id`, `thread`, `folder_id` | Local IDs |
| `account`, `folder` | Account name and folder path |
| `subject`, `from`, `to`, `cc` | Addresses are `{"name", "addr"}` |
| `received_at`, `sent_at` | RFC 3339, UTC |
| `is_read`, `is_flagged`, `importance`, `has_attachments` | `importance` is `low`, `normal` or `high` |
| `preview` | Graph's short text preview |
| `body_local`, `body_pinned` | Whether the body is on this device, and pinned |
| `remote_id`, `internet_message_id` | Graph immutable ID (for `fam api`) and RFC 5322 Message-ID |

## fam mail thread

```sh
fam mail thread 1234
fam mail thread 1234 --no-trash --json
```

The conversation that message 1234 belongs to, oldest first, across every folder of its account: replies in Sent Items, drafts, messages filed in subfolders and, unless `--no-trash`, Deleted Items and Junk Email. Copies of one message (same Message-ID, such as mail to yourself) show once, unread if any copy is, with the other copies' IDs in `copies`. Each message also has `from_me` and `in_trash`. The top level has `subject` (without `RE:`/`FW:`), `participants`, `message_count`, `unread_count`, `last_received_at` and per-folder counts. How conversations are assembled: [sync.md](sync.md#conversations).

## fam mail fetch

```sh
fam mail fetch 1234
fam mail fetch 1234 --format html --json
fam mail fetch 1234 --local
```

The header plus `body` and `body_type` (`text` or `html`). By default HTML is converted to plain text; `--format html` returns the body as stored (a plain-text message stays `text`).

Bodies newer than `sync.window` are already local. An older one is downloaded through famd into the on-demand cache (`downloaded: true`, `cached: true`), which is bounded by `sync.cache_limit` and evicts least recently used first; use `fam mail pin` to keep one for good. If famd isn't running the command exits 11 and says so; `--local` never downloads and returns the header without `body`. With `sync.older_bodies = "none"` an old body is refused with exit 5.

## fam mail attachments

```sh
fam mail attachments 812                         # names, types and sizes
fam mail attachments 812 --save ~/Downloads      # save them all
fam mail attachments 812 --save . --name minutes.pdf
```

Attachments aren't synced, so this reads them from the server through famd. A saved file never replaces one already there: the second `minutes.pdf` becomes `minutes (2).pdf`. Inline images (the ones shown inside the body) are left out unless `--inline`. Attached Outlook items and OneDrive links are listed with their kind but can't be saved. The reading pane in the desktop app uses the same code.

## fam api

```sh
fam api /me
fam api '/me/mailFolders/inbox/messages?$top=3&$select=subject,from' --account hotmail
fam api 'https://graph.microsoft.com/v1.0/me/messages?$skip=10' --json   # a nextLink
```

The escape hatch for anything Familiar doesn't model yet. It GETs a path under `https://graph.microsoft.com/v1.0`, or a full link Graph returned such as `@odata.nextLink`, with the account's token, and prints Graph's JSON unchanged (pretty-printed either way). Quote paths that contain `$` or `&`.

- **GET only.** Writes arrive with the durable op queue in Phase 2. `--method GET` is accepted; any other method exits 5 (`permission_denied`) before anything is sent. The command's access level is `read`.
- **Graph only.** Relative paths must start with one `/`; full links must be on `graph.microsoft.com`. Anything else exits 2.
- **Through famd.** famd holds the tokens and paces each mailbox's requests, so the call waits its turn at user priority (ahead of background sync), follows `Retry-After` on 429 and 503, and refreshes an expired token once. Without famd it exits 11.
- **Which account.** `--account` names it; it may be left out when famd syncs a single account.
- **Errors.** Graph's 404 exits 6, 400 exits 2, 403 exits 5, throttling that outlasts the retries exits 8, sign-in problems exit 4 and other failures exit 12, each with Graph's error code and message.

famd adds `Prefer: IdType="ImmutableId"` to every request, so IDs in responses match `remote_id`.

## Tests

- `crates/cli/tests/mail.rs`: the `fam` binary against a store synced from the mock's small mailbox: folder tree and paths, list by name, path and well-known name, filters, paging, the three-folder conversation, text and HTML bodies, `--local`, exit codes before anything has synced and without famd, and `fam api` refusing writes and non-Graph URLs.
- `crates/daemon/tests/read.rs`: with famd running against the mock, `fetch` downloads an old body into the cache, and `fam api` GETs Graph with the account's token and keeps Graph's error kinds.
