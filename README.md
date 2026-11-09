# Familiar

A local-first mail and calendar client for Microsoft accounts (Outlook.com, Hotmail and Microsoft 365). It's a Rust engine with a sync daemon (`famd`), a command line (`fam`), an MCP server and a Tauri desktop app, all built on one core.

## Why Familiar

### It's blazingly fast

Every screen and every command reads a local SQLite store, never the network. Sync happens in the background, so nothing you do waits on Microsoft.

Measured on real accounts (Hotmail with 9,400 messages in 46 folders, plus a Microsoft 365 account, release build on Windows):

| What | Time |
| --- | --- |
| `fam mail list --json`, merged inbox | 17 ms median (target 50 ms) |
| `fam mail thread` | 17 ms |
| `fam mail search`, 50 hits | about 22 ms median |
| `famd` memory, synced and idle | under 40 MB |
| Whole store on disk, both accounts | 22 MB |

Measured on a synthetic 500,000-message mailbox:

| What | Time |
| --- | --- |
| Search, p95, 50 newest hits | under 12 ms for every query type but `is:unread` (35 ms) |
| Desktop app, first page of search results | 5 to 7 ms |
| Desktop app, open the unified inbox | 44 ms median |
| Desktop app, scroll frame p95 | 19 ms, with no blank rows |
| First sync of all 500,000 headers from the mock server | 76 s |

The desktop app's numbers are checked in CI: a job seeds 500,000 messages and fails the build if anything goes over the budget in `crates/gui/perf-budget.json`. Details are in the [development plan](designs/Development%20Plan.md) and the [WP 0.5 store benchmark](designs/WP%200.5%20Store%20benchmark.md).

### 100% CLI coverage, so AIs can use it

Everything the desktop app can do, `fam` can do. That's enforced by the architecture, not by discipline: every action is a typed command in one registry (`crates/core`), and the CLI, its JSON schema, the MCP tools and the desktop app's TypeScript bindings are all generated from it. The desktop app has a single way into the engine, a `run` call into that same registry. A [parity test](crates/cli/tests/parity.rs) fails the build if a command is missing from any surface.

That makes Familiar a mail and calendar tool for agents as well as people:

- **52 commands** today, across mail, drafts, folders, calendar, contacts, sync and the op queue. All of them take `--json` and return output that matches a published schema (`fam schema --json`), with [stable exit codes](docs/exit-codes.md).
- **MCP server built in.** `fam mcp serve` offers every command as an MCP tool, for example `claude mcp add familiar -- fam mcp serve --allow read,draft`. See [docs/mcp.md](docs/mcp.md).
- **A ready-made skill** for Claude Code in [`skills/familiar`](skills/familiar), with a [command reference](skills/familiar/commands.md) generated from the registry.
- **Safe by default.** Agents get read and draft access only. Other writes need a grant in your config (sending and deleting only per account), and every write goes in an audit log with who asked (`fam audit list`). See [docs/agent.md](docs/agent.md).

### Runs on any OS

Windows, macOS and Linux. CI builds and tests the whole workspace on all three on every merge. The engine, daemon and CLI have no GUI dependencies at all (CI checks this on a machine with no display libraries), so `famd` and `fam` also run headless on a server or in a small container. Only the desktop app needs a webview.

### Restorable from a single config file

Everything that defines your setup lives in one `config.toml`: accounts, aliases, per-account signatures for new mail and replies, calendar settings, sync window, saved views, rules, keyboard shortcuts and agent permissions. It holds no passwords, tokens or mail. Tokens live in the OS keychain, and mail is synced back from the server.

So a new machine is three steps:

1. Copy your `config.toml` into place (`fam config path` shows where) and run `fam config validate`.
2. Sign in to each account with `fam auth login <account>`, using the account names from your config (`hotmail`, `parklands` and so on).
3. Start `famd` and let it sync.

Keep your config in a private repo of its own, since it has your signatures in it. [`config.example.toml`](config.example.toml) shows every setting, and [docs/config.md](docs/config.md) explains them. (Folder colours and pins are kept per device, in `folders.json`, so they don't travel with the config.)

## How it fits together

`famd` is the only process that writes the store. It syncs with Microsoft Graph by polling delta queries, keeps a durable queue of your local changes and sends them in order, and serves a local socket. `fam` and the desktop app read the store directly, which is why they work offline, and send writes to `famd`. Headers are kept for all of history; full bodies for a window you choose (one year by default), with older ones fetched on demand. Search is local (Tantivy), with an option to include the server for old message text.

See [the design document](designs/Email%20Client%20Design%20Document.md) and [the development plan](designs/Development%20Plan.md).

## Workspace

| Crate | Path | What it is |
| --- | --- | --- |
| `familiar-core` | `crates/core` | Command registry and shared types |
| `familiar-config` | `crates/config` | `config.toml` parsing, validation and write-back |
| `familiar-auth` | `crates/auth` | Microsoft sign-in, token storage and refresh |
| `familiar-provider-graph` | `crates/provider-graph` | Microsoft Graph provider |
| `familiar-mock-graph` | `crates/mock-graph` | Local mock of Graph for offline tests |
| `familiar-store` | `crates/store` | SQLite store (only `famd` writes) |
| `familiar-search` | `crates/search` | Tantivy search and query language |
| `familiar-rules` | `crates/rules` | Mail rules |
| `familiar-daemon` | `crates/daemon` | `famd`, the sync daemon |
| `familiar-cli` | `crates/cli` | `fam`, the command line and MCP server |
| `familiar-gui` | `crates/gui` | Desktop app (Tauri 2 and SolidJS) |

Only `familiar-gui` may depend on GUI or webview libraries; CI enforces this with `scripts/check-headless-deps.sh` on a runner with no display libraries.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
scripts/check-licence-headers.sh
```

Every Rust file starts with `// SPDX-License-Identifier: MIT OR Apache-2.0`. `spikes/` and `tools/` hold the Phase 0 experiments and are separate workspaces.

## Licence

MIT or Apache-2.0, at your option.
