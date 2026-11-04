# Familiar

A local-first mail and calendar client for Microsoft accounts: a Rust engine with a daemon (`famd`), a CLI (`fam`) and a Tauri desktop app on top. See [the design document](designs/Email%20Client%20Design%20Document.md) and [the development plan](designs/Development%20Plan.md).

## Workspace

| Crate | Path | What it is |
| --- | --- | --- |
| `familiar-core` | `crates/core` | Command registry and shared types |
| `familiar-provider-graph` | `crates/provider-graph` | Microsoft Graph provider |
| `familiar-store` | `crates/store` | SQLite store (only `famd` writes) |
| `familiar-search` | `crates/search` | Tantivy search and query language |
| `familiar-rules` | `crates/rules` | Mail rules |
| `familiar-daemon` | `crates/daemon` | `famd`, the sync daemon |
| `familiar-cli` | `crates/cli` | `fam`, the command line |
| `familiar-gui` | `crates/gui` | Desktop app (stub until Phase 3) |

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
