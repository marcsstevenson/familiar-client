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

Keys in the list: the arrow keys, Page Up/Down, Home and End (and `j`/`k` from anywhere, below).

## Command palette and keys

Ctrl+K (Cmd+K on macOS) opens the palette. It lists the app's actions with their keys, and every registered command with its `fam` form. Choosing a command opens a form generated from its input schema (`fam schema`), prefilled with the selected message where the command takes `message` or `messages`, and shows the matching `fam` command line as you fill it in. Run shows the command's JSON output.

`ui/src/actions/surface.ts` says where each registered command lives: `palette`, or `terminal` for the two that need one (`auth.login`, `mcp.serve`). It is a `Record` over every command name, so a new command fails `tsc` (and CI's front-end build) until it is placed there.

Actions and their default keys (`ui/src/actions/builtin.ts`):

| Action ID | Key | Does |
| --- | --- | --- |
| `palette` | Ctrl+K | Show all commands |
| `next`, `previous` | `j`, `k` | Move through the list |
| `archive` | `e` | `fam mail archive` |
| `delete` | `#` | `fam mail delete` |
| `flag` | `s` | `fam mail flag` (or `--clear`) |
| `toggle_read` | `u` | `fam mail read` (or `--unread`) |
| `reply`, `reply_all`, `forward` | `r`, `a`, `f` | Draft a reply or forward (the command form until the compose window lands) |
| `compose` | `c` | New message |
| `theme` | | Cycle the theme |

Change a key in `config.toml`, by action ID:

```toml
[keys]
archive = "y"
palette = "Ctrl+P"
```

Single keys are ignored while typing in a field.

## `fam ui`

The app listens on its own socket (`familiar-ui.sock` in the data directory, or the pipe `\\.\pipe\familiar-ui-<hash>` on Windows), with the same line protocol as famd. `fam ui open <message>` shows a message (its folder, then the message) and `fam ui folder [<folder>] [--account <name>]` shows a folder, or all inboxes with no folder. Both bring the window to the front, and fail with exit code 11 when the app isn't running.

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

## Reading pane

The reading pane (`ui/src/reading`, Rust in `src/reading`) opens whatever message `selectedMessage()` in `ui/src/state.ts` names, and marks it read.

- **Header first.** It asks the store for the message with `local` set, so the header shows at once, then asks again without it when the body isn't on this device, which downloads it through famd.
- **Sanitised in Rust.** `src/reading/sanitize.rs` runs every body through ammonia's allow list: scripts, event handlers, forms, frames, `<meta>`, `<base>` and any link that isn't `http`, `https`, `mailto` or `#` go. Plain-text bodies are escaped and their links made clickable.
- **Shown where nothing runs.** The document goes into an iframe with `sandbox="allow-same-origin"` and no `allow-scripts`, carrying its own CSP (`default-src 'none'`, images only from `data:` and the image proxy). The pane reaches into the frame to show where the link under the pointer goes, warns when a link's text names a different site, and opens links in the system browser.
- **Remote images blocked.** They are replaced by a same-size placeholder and counted. "Show images" allows them for that message for the session; "Always show from" adds the sender to `reading.json` in the data directory. Allowed images load through the app's `famimg` scheme (`src/reading/remote.rs`), so the webview never contacts a remote server; each proxied URL carries a random token made at startup, so a message cannot write its own proxy URL (in CSS, say) to get past the block. The proxy sends no cookies or referrer, stops reading past 15 MB and answers only with images. Images in CSS (`background: url(...)`) stay blocked by the CSP either way.
- **Inline images** (`cid:`) are downloaded with the message's attachments and become `data:` URIs.
- **Attachments** are listed under the header. Images and text files preview inside the app; Save puts the file in Downloads without replacing anything. This is the same code as `fam mail attachments`.
- **Prefetch.** Opening a message downloads the next two in its folder, and their inline images, in the background.

### HTML test set

`testdata/html` holds hostile and ordinary bodies: every script trick we know, frames and forms, remote content of each kind, mutation XSS, deceptive links, an Outlook reply and a newsletter. `cargo test -p familiar-gui html_test_set` sanitises each one with images blocked and shown, and fails if anything active or remote survives. To render them in Chromium (WebView2's engine) inside the app's sandbox and CSP, and fail if a script runs or a request leaves the page:

```sh
FAMILIAR_HTML_OUT=$PWD/target/reading-html cargo test -p familiar-gui html_test_set
node scripts/check-reading-html.cjs target/reading-html
```

Add a file whenever a real message renders wrongly or unsafely.
