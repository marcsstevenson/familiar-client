# WP 0.4 UI toolkit bake-off: Tauri 2 vs GPUI

Throwaway prototypes for the Phase 0 toolkit decision. The same tiny app is built twice:

- three panes: folders, a virtualised list of **500,000 synthetic messages**, and a reading pane;
- the reading pane shows a **sanitised ~60 KB HTML newsletter** (layout tables, a `<style>` block, remote images and tracking pixels, plus script, handlers, a form and an iframe that the sanitiser must remove);
- `j`/`k` and the arrow keys move the selection and open the message.

| Folder | What it is |
| --- | --- |
| `common/` | Shared Rust: deterministic synthetic rows, the newsletter generator, the `ammonia` sanitiser with remote images blocked and a strict CSP, and the bench recorder |
| `gpui/` | GPUI prototype (`gpui-kit` 0.9 on `gpui-pre` 0.3.11). `FAMILIAR_GPUI_HTML=native` uses GPUI Kit's native HTML view; `FAMILIAR_GPUI_HTML=webview` places a system webview (wry) over the reading pane as a child window |
| `tauri/` | Tauri 2.12 + SolidJS + strict TypeScript. Rows come from Rust a page at a time; the message goes into `<iframe sandbox="">` with a CSP that blocks every network load |
| `bench/` | `run_linux.py` (Xvfb, xdotool) and `run_windows.ps1` (SendKeys) harnesses |

No real mail, names or addresses are used anywhere.

## Build

Linux needs the usual Tauri and GPUI packages, for example on Ubuntu 24.04:

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev build-essential libssl-dev \
  libvulkan-dev mesa-vulkan-drivers libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev \
  libfontconfig-dev libfreetype-dev libasound2-dev clang
```

Windows needs the MSVC build tools and the WebView2 runtime (already on Windows 10/11). Then, from this folder:

```sh
cd tauri && npm ci && npm run build && cd ..
cargo build --release
```

## Run the bench

Each app runs in bench mode when `FAMILIAR_BENCH_OUT` names a file. It then scrolls the list 40 px per frame for 600 frames, makes 15 selection moves and 15 opens of distant messages by itself, and logs each timing. The harness adds real key presses, idle memory for the whole process tree (so WebKit/WebView2 helper processes count) and idle CPU.

```sh
python3 bench/run_linux.py --runs 5 gpui gpui-webview tauri     # Linux, starts its own Xvfb
python3 bench/run_linux.py --no-xvfb --display :0 gpui tauri    # Linux desktop with a real GPU
```

```powershell
.\bench\run_windows.ps1 -Runs 5                                 # Windows
```

Results land in `bench/results/` (ignored by git apart from the committed summaries).

## What each number means

| Measure | How |
| --- | --- |
| Cold start | Harness timestamp just before spawn, to the first frame that shows rows and the first message (process start, warm disk cache) |
| Idle memory | Linux: PSS summed over the process tree. Windows: working set and private bytes summed over the tree |
| Scroll frame time | Time between frames while the list scrolls 40 px per frame (GPUI: `on_next_frame`; Tauri: `requestAnimationFrame`) |
| Key latency, in app | From the key event (Tauri: the event's own timestamp; GPUI: action handler) to the frame that shows the new selection |
| Key latency, external | From the harness sending the key through the OS to that frame |
| Open | From selecting a distant message to its HTML being painted (sanitise + render) |
