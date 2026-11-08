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

The folder pane shows "All inboxes" (every account's Inbox together), then pinned folders, then each account's tree from `folder.list`, with the well-known folders first.

Right-clicking a folder (or the menu key / Shift+F10) opens its menu: pin or unpin, a colour (shown as a dot before the name), and for folders that aren't built in, Rename… and Delete… (which asks first, then moves the folder into Deleted Items). Colours and pins are kept on this device only, in `folders.json` in the data directory, keyed by the folder's Graph ID so they survive a rename or move; `fam folder color <folder> <colour|none>`, `fam folder pin` and `fam folder unpin` change the same file. Rename and delete are `folder.rename` and `folder.delete` through the op queue.

The splitters between the panes drag to resize (arrow keys when focused, double-click to reset); the widths are kept in localStorage. The folder and list panes keep their widths when the window resizes, and the reading pane takes the change. The sort, Unread and count row above the list shows only while the pointer is over the search box (or while it isn't the default), and the "images from this sender are always shown" line shows only while the pointer is over the reading pane's header.

The list is virtualised. Rust (`src/list.rs`) holds the sorted message IDs of the open view, and the front end asks for the rows on screen by position:

- `list_open(spec)` reads and sorts the IDs for `{ folders, unread_only, sort }` (`sort` is `newest`, `oldest`, `from` or `subject`; empty `folders` means all inboxes) and returns the total.
- `list_rows(start, count)` returns up to 500 rows of the open view.
- `list_index_of(id)` finds a message's position, to keep the selection after a refresh.

A background thread checks the store's `PRAGMA data_version` once a second and sends `store_changed` when another process (famd, `fam`) has committed. The list then reopens the view and swaps in the rows on screen before redrawing, so it doesn't flash while famd syncs. The folder pane refetches its counts on the same event.

Keys in the list: the arrow keys, Page Up/Down, Home and End (and `j`/`k` from anywhere, below).

## Search and smart folders

The box above the list searches this device with the `fam mail search` query language (`from:`, `subject:`, `has:attachment`, `after:`, `is:unread`, "phrases" and so on); `/` jumps to it and Escape clears it. Rust runs the query against famd's index (`Searcher::all_ids`) and the list shows the newest 10,000 matches, newest first, through the same virtualised list as a folder. "Server too" runs `mail.search --scope all` and lists what only the server found, labelled online, above the results; those messages aren't on this device yet, so they can't be opened.

`[[views]]` in `config.toml` appear under Views in the folder pane, and choosing one runs its query:

```toml
[[views]]
name = "Club, unread"
query = "account:parklands is:unread"
```

The bench (below) times five searches to the first page of rows; on the 500k synthetic store they took 5 to 7 ms, and `is:unread` (30k matches) 71 ms median, against the 100 ms target.

## Command palette and keys

Ctrl+K (Cmd+K on macOS) opens the palette. It lists the app's actions with their keys, and every registered command with its `fam` form. Choosing a command opens a form generated from its input schema (`fam schema`), prefilled with the selected message where the command takes `message` or `messages`, and shows the matching `fam` command line as you fill it in. Run shows the command's JSON output.

`ui/src/actions/surface.ts` says where each registered command lives: `palette`, or `terminal` for the two that need one (`auth.login`, `mcp.serve`). It is a `Record` over every command name, so a new command fails `tsc` (and CI's front-end build) until it is placed there.

Actions and their default keys (`ui/src/actions/builtin.ts`):

| Action ID | Key | Outlook key | Does |
| --- | --- | --- | --- |
| `palette` | Ctrl+K | | Show all commands |
| `next`, `previous` | `j`, `k` | arrows (in the list) | Move through the list |
| `archive` | `e` | Backspace | `fam mail archive` |
| `process` | (none; set one in `[keys]`) | | `fam mail process` |
| `delete` | `#` | Delete, Ctrl+D | `fam mail delete` |
| `flag` | `s` | Insert | `fam mail flag` (or `--clear`) |
| `toggle_read` | `u` | | `fam mail read` (or `--unread`) |
| `mark_read`, `mark_unread` | | Ctrl+Q, Ctrl+U | `fam mail read` / `--unread` |
| `move` | | Ctrl+Shift+V | The palette's Move form |
| `reply`, `reply_all`, `forward` | `r`, `a`, `f` | Ctrl+R, Ctrl+Shift+R, Ctrl+F | Open a compose window replying to or forwarding the selected message |
| `compose` | `c` | Ctrl+N | New message |
| `search` | `/` | Ctrl+E, F3 | Go to the search box |
| `theme` | | | Cycle the theme |

The Outlook keys are bound as well as the single keys; `[keys]` changes only the single key. Hovering a row shows a delete button, and a tick for accounts with a `processed_folder` in config.toml that moves the message there (creating the folder if needed), and dragging a row onto a folder of the same account moves it there. When the open message is deleted, archived, processed or moved, the next one down opens (or the one above, at the end of the list). Accounts and folders with subfolders collapse with their arrow (or Left and Right); the app remembers which.

Change a key in `config.toml`, by action ID:

```toml
[keys]
archive = "y"
palette = "Ctrl+P"
```

Single keys are ignored while typing in a field.

## `fam ui`

The app listens on its own socket (`familiar-ui.sock` in the data directory, or the pipe `\\.\pipe\familiar-ui-<hash>` on Windows), with the same line protocol as famd. `fam ui open <message>` shows a message (its folder, then the message) and `fam ui folder [<folder>] [--account <name>]` shows a folder, or all inboxes with no folder. Both bring the window to the front, and fail with exit code 11 when the app isn't running.

## Timing and accessibility checks

`FAMILIAR_BENCH_OUT=<file> familiar` runs `ui/src/bench.ts` and writes its numbers as JSON to the file, then exits (with `FAMILIAR_BENCH_HOLD` set it stays open, so a harness can measure idle memory). It times:

- cold start to the first rows;
- opening the unified view, and five searches to their first page of rows;
- 600 frames of scrolling at 40 px, and jumps around the list;
- 30 presses of `j`, to the frame showing the new selection and to the frame showing its header in the reading pane.

It also runs axe-core's WCAG 2.1 A and AA rules over the app's page (not the message frame) and lists serious and critical violations.

`scripts/bench-gui.sh <data dir> <report.json>` runs all that under `xvfb-run` on Linux (on a private D-Bus session, since the runner's session bus held each launch for 30 s, and after one warm-up launch to fill font and WebKit caches), adds idle private memory (anonymous memory over the app and its WebKit processes, 10 s after the bench), and checks the report against `crates/gui/perf-budget.json` with `scripts/check-bench.mjs`. Each entry there has a `limit`, which fails the build, and the design doc's `target`. The limits are looser than the targets where CI's software-rendered runner can't show them (a scroll frame there is about 17 to 20 ms however fast the list is). CI's "GUI performance and accessibility" job runs it on main against a fresh synthetic store:

```sh
cargo build --release -p familiar-gui -p familiar-store -p familiar-search --examples --bins
target/release/examples/seed /tmp/fam-bench/store 500000
target/release/examples/index /tmp/fam-bench
bash scripts/bench-gui.sh /tmp/fam-bench bench.json
```

`scripts/check-contrast.mjs` (in the lint job) checks every text colour in `tokens.css` at 4.5:1 against the surfaces it sits on, and the focus ring at 3:1, in the light and dark sets.

## famd

When the window opens, the app looks for famd on its socket. If nothing answers, it starts famd from the same folder as `familiar` (or from PATH), detached, so famd keeps syncing after the window closes. famd's log then goes to `famd.log` in the data directory. The app checks famd every 5 seconds and starts it again if it has stopped (after a failed start it waits 30 seconds). The status bar shows whether sync is running and, if famd stopped, the last line it logged.

## Window and themes

On Windows and Linux the window has no system frame and the app draws its own title bar with the window buttons. On macOS the system traffic lights sit over the left end of the app's bar.

Colours, spacing and type sizes are tokens in `ui/src/styles/tokens.css`, with one light and one dark set; `theme.ts` resolves `system` and sets `data-theme` to `light` or `dark`. The theme starts from `[app] theme` (`system`, `light` or `dark`); the button in the title bar cycles through them for the session. In the dark theme, message bodies show dark too, as in Outlook: light backgrounds turn dark and dark text turns light, each keeping its hue (`ui/src/reading/dark.ts`); pictures and emoji are left as they are. The ☀ button by the subject shows that message as sent, until another message opens.

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

## Compose

Each draft gets a window of its own (`compose-<n>`), opened from the pencil in the title bar, Ctrl+N, or from code with `openCompose` in `ui/src/compose`:

```ts
import { openCompose } from "./compose";
openCompose({ kind: "new" });                       // also { account, to }
openCompose({ kind: "reply", message: id, all: true });
openCompose({ kind: "forward", message: id });
openCompose({ kind: "draft", draft: id });          // an existing draft, from anywhere
```

The window saves with the same commands as `fam draft`, so drafts round-trip between the two. A new message becomes a draft (`draft.create`) on its first save; a reply or forward is made with `draft.reply`/`draft.forward` as the window opens. After that each save is a `draft.update` carrying only what changed. Saves happen four seconds after typing stops, on Ctrl+S, before sending and when the window closes. Send (Ctrl+Enter) runs `draft.send`; the status bar counts down the undo-send delay with an Undo button (`ops.cancel`), and the window closes when it has gone. If many outside people would see each other, the window offers to move them to Bcc.

- **Rich and plain text.** Rich text is a contenteditable with the browser's editing commands; plain text is a textarea, saved as plain text the way `fam draft --body` is.
- **Signatures** come from `[accounts.<name>.signature]` through the `compose_info` IPC command, with images from local files turned into `data:` URIs. The window keeps the signature in a `<div id="Signature">`, as `fam` does, and swaps it when the From account or the Signature menu changes. A saved new message moves to another account by being saved afresh there (the old draft is deleted); replies keep their account. The From menu also lists each account's `aliases`; choosing one sets the draft's From (`--from`), and a reply starts on the alias the original was sent to.
- **Images** pasted, dropped on the text or picked with the picture button go into the body as `data:` URIs; the engine turns them into inline attachments. Opening an existing draft shows its `cid:` images by reading the draft's attachments through `api`, keeping their links so they aren't attached again.
- **Attachments** are dropped anywhere else in the window or picked with Attach (`file_info` and `pick_files` IPC commands, 3 MB each for now), and uploaded on the next save. Removing one the draft already has sends `remove_attachment`.
- **Autocomplete** in To, Cc and Bcc uses `contacts.search`.
- Bcc isn't in the store, so an opened draft reads it from the server; offline it stays as it was unless changed here.
