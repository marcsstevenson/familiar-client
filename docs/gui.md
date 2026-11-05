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
- **Signatures** come from `[accounts.<name>.signature]` through the `compose_info` IPC command, with images from local files turned into `data:` URIs. The window keeps the signature in a `<div id="Signature">`, as `fam` does, and swaps it when the From account or the Signature menu changes. A saved new message moves to another account by being saved afresh there (the old draft is deleted); replies keep their account.
- **Images** pasted, dropped on the text or picked with the picture button go into the body as `data:` URIs; the engine turns them into inline attachments. Opening an existing draft shows its `cid:` images by reading the draft's attachments through `api`, keeping their links so they aren't attached again.
- **Attachments** are dropped anywhere else in the window or picked with Attach (`file_info` and `pick_files` IPC commands, 3 MB each for now), and uploaded on the next save. Removing one the draft already has sends `remove_attachment`.
- **Autocomplete** in To, Cc and Bcc uses `contacts.search`.
- Bcc isn't in the store, so an opened draft reads it from the server; offline it stays as it was unless changed here.
