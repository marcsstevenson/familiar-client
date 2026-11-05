# The desktop app

`crates/gui` is Familiar's desktop app: Tauri 2 in Rust, with a SolidJS + TypeScript front end in `crates/gui/ui`. The binary is `familiar`.

## Build and run

The front end is built by npm, then embedded in the binary by Cargo:

```sh
cd crates/gui/ui && npm ci && npm run build && cd -
cargo run -p familiar-gui            # or: cargo build --release -p familiar-gui
```

A plain `cargo build` without the npm step still compiles. `build.rs` writes a placeholder page that says the front end isn't built. That keeps the Rust CI jobs free of Node.

Linux needs the WebKitGTK development packages (`libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev` on Ubuntu). Windows uses the WebView2 runtime that ships with Windows 10 and 11.

## How it talks to the engine

The front end has one way in: the `run` IPC command, which calls a command from the same registry `fam` and the MCP server use. `ui/src/api.ts` types it from the generated `bindings/commands.ts`:

```ts
const folders = await run("folder.list", {});
```

Changes made from the app are recorded with the source `gui`, so they are never checked against the agent grants. `mcp.serve` is refused, since it needs a terminal.

The shell has a few IPC commands of its own: `ui_info` (version, platform, the `[app] theme` from config.toml), `daemon_state`, the list commands below and `bench_report`.

## Folders and the message list

The folder pane shows "All inboxes" (every account's Inbox together), then each account's tree from `folder.list`, with the well-known folders first.

The list is virtualised. Rust (`src/list.rs`) holds the sorted message IDs of the open view, and the front end asks for the rows on screen by position:

- `list_open(spec)` reads and sorts the IDs for `{ folders, unread_only, sort }` (`sort` is `newest`, `oldest`, `from` or `subject`; empty `folders` means all inboxes) and returns the total.
- `list_rows(start, count)` returns up to 500 rows of the open view.
- `list_index_of(id)` finds a message's position, to keep the selection after a refresh.

A background thread checks the store's `PRAGMA data_version` once a second and sends `store_changed` when another process (famd, `fam`) has committed. The list then reopens the view and swaps in the rows on screen before redrawing, so it doesn't flash while famd syncs. The folder pane refetches its counts on the same event.

Keys in the list: `j`/`k` or the arrow keys, Page Up/Down, Home and End.

## Timing

`FAMILIAR_BENCH_OUT=<file> familiar` runs `ui/src/bench.ts`: it times opening the unified view, scrolls 600 frames at 40 px, jumps around the list, writes the numbers as JSON to the file and exits. For a large synthetic store:

```sh
cargo run --release -p familiar-store --example seed -- /tmp/fam-bench/store 500000
FAMILIAR_DATA_DIR=/tmp/fam-bench FAMILIAR_BENCH_OUT=bench.json target/release/familiar
```

## famd

When the window opens, the app looks for famd on its socket. If nothing answers, it starts famd from the same folder as `familiar` (or from PATH), detached, so famd keeps syncing after the window closes. famd's log then goes to `famd.log` in the data directory. The app checks famd every 5 seconds and starts it again if it has stopped (after a failed start it waits 30 seconds). The status bar shows whether sync is running and, if famd stopped, the last line it logged.

## Window and themes

On Windows and Linux the window has no system frame and the app draws its own title bar with the window buttons. On macOS the system traffic lights sit over the left end of the app's bar.

Colours, spacing and type sizes are tokens in `ui/src/styles/tokens.css`, with a light and a dark set. The theme starts from `[app] theme` (`system`, `light` or `dark`); the button in the title bar cycles through them for the session.
