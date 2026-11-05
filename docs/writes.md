# Writes and the op queue

Familiar writes local-first (WP 2.1, 2.2). A write changes the store at once,
so `fam mail list` and the GUI show it straight away, and queues an **op**
for the server in the same transaction. `famd` sends ops and keeps trying
until the server has them.

## Commands

| Command | Does | Access |
| --- | --- | --- |
| `fam mail read <ids>... [--unread]` | Mark read or unread | write |
| `fam mail flag <ids>... [--clear]` | Flag, or clear the flag | write |
| `fam mail move <ids>... --to <folder>` | Move to a folder of the same account | write |
| `fam mail archive <ids>...` | Move to Archive | write |
| `fam mail delete <ids>... [--permanent --yes]` | Move to Deleted Items; `--permanent` deletes items already there for good | delete |
| `fam folder create <name> [--parent <folder>]` | Create a folder | write |
| `fam folder rename <folder> <name>` | Rename | write |
| `fam folder move <folder> (--to <folder> \| --top)` | Move under another folder, or to the top level | write |
| `fam folder delete <folder> [--permanent --yes]` | Move into Deleted Items, or delete for good from there | delete |
| `fam ops list [--all]` | Changes waiting for the server, and failed ones | read |
| `fam ops retry <id>` / `fam ops cancel <id>` | Try a failed change again / drop a queued or failed one and undo it here | write |

Every write takes `--dry-run` (say what would change, change nothing) and
`--wait` (wait until the server has it, up to `--wait-secs`, default 60).
Folders are named by path (`Inbox/Club`), well-known name (`inbox`,
`archive`, `deleteditems`) or plain name. Folder commands need `--account`
when more than one account is synced.

Exit codes: a refused change under `--wait` is `server` (12); a `--wait`
that times out is `offline` (7) and the change stays queued; `--wait`
without famd is `daemon_unavailable` (11) and nothing changes; a permanent
delete without `--yes` is `confirmation_required` (10).

## How the queue behaves

- **Order.** Each account's ops run one at a time in queue order, so a move
  into a folder created a moment before waits for the folder.
- **Without famd.** `fam` takes the store's writer lock itself, makes the
  change and queues it. famd sends it when it starts.
- **Restarts and kills.** An op famd had claimed when it stopped is put back
  at the front of the queue at start. Every op is safe to send twice:
  read/flag/move are idempotent with immutable IDs, folder create looks for
  an existing folder of that name, and deletes treat "not found" as done.
- **Passing failures** (offline, throttled, 5xx, expired sign-in) wait 2 s,
  doubling to 5 min, and go again, for as long as it takes.
- **Refusals** (4xx, item gone) fail the op and undo its local change. A
  failed folder create also fails and undoes later ops that needed the
  folder.
- **Sync and pending changes.** While an item has an open op, sync leaves it
  alone, so a slow server can't flip a message back. The round after the
  op lands brings the server's view.
- **Audit log.** Every queued, done and failed op is recorded with its
  source (`cli`, `mcp`, an agent name) in the store's `audit` table.

Code: `familiar_core::ops` (planning, local apply/undo), `familiar_daemon::ops`
(the runner), `familiar_provider_graph` `write.rs` (Graph calls), mock routes
in `crates/mock-graph/src/writes.rs`. Tests: `crates/daemon/tests/writes.rs`.
