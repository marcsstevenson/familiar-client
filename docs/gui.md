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

Two more IPC commands exist for the shell itself: `ui_info` (version, platform, the `[app] theme` from config.toml) and `daemon_state`.

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
