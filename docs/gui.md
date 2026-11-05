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
