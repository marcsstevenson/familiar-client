# WP 0.4 results: Tauri 2 vs GPUI

3 Nov 2026 · Linux, measured in the cloud · Windows run still to do on Marc's PC

## Recommendation

**Use Tauri 2 + SolidJS**, provided the Windows run (`bench/run_windows.ps1`, about 10 minutes on Marc's PC) shows cold start under 500 ms and keypress p95 under 16 ms on WebView2. If Tauri misses those on Windows by a wide margin, revisit before Phase 3.

The deciding result is HTML mail, not speed:

1. **GPUI can't show a real newsletter by itself.** GPUI Kit's native HTML view drops the `<style>` block and ignores layout tables and most inline CSS, so the test newsletter comes out as a jumble of text and grey boxes ([screenshot](bench/results/gpui.png)). A GPUI mail client therefore has to embed a webview anyway, as the design doc predicted.
2. **Once GPUI embeds a webview, its advantages mostly go.** The GPUI + webview build used *more* memory than Tauri (408 MB vs 386 MB PSS here), and after the webview appeared **none of the 150 real key presses reached the app** (GPUI + native view: 150 of 150; Tauri: 150 of 150). A child webview takes keyboard focus, so every shortcut needs focus hand-off code written per OS. On Linux, wry child webviews work on X11 only (no Wayland), GPUI's X11 handle had to be re-wrapped for wry, and the GTK main loop has to be pumped from GPUI's loop.
3. **Tauri's weak numbers here come from software rendering.** This container has no GPU, so WebKitGTK and GPUI both draw on the CPU (GPUI through lavapipe Vulkan). Tauri missed the start and keypress targets in this setup, and GPUI was clearly faster at both. That is a real point in GPUI's favour, and it is exactly what the Windows run must settle.
4. **Developer experience favours Tauri.** Half the dependencies (252 vs 566 crates), a 8 MB vs 40 MB stripped binary, a stable API, and a UI that Claude can drive and screenshot with ordinary web tooling (WebDriver on Windows and Linux, or the plain web UI in Playwright with the Rust commands mocked). GPUI is pre-1.0 and its crates are mid-split: `gpui` 0.2.2 on crates.io vs the `gpui-pre` 0.3.11 snapshot that GPUI Kit pins, and this small prototype hit several snags (a renamed prelude import, the XCB vs Xlib window handle mismatch with wry). To its credit, GPUI Kit now ships a headless UI test harness.

## Linux results

Ubuntu 24.04 under Xvfb, 4 vCPU Xeon 2.1 GHz, 16 GB, **no GPU** (WebKitGTK software rendering; GPUI on Mesa lavapipe). Release builds, 5 launches each, 1280×800 window, 500,000 rows. Raw data: `bench/results/linux-*.json`.

| Measure | Target | GPUI, native HTML | GPUI + webview | Tauri 2 |
| --- | --- | --- | --- | --- |
| Cold start to usable inbox, median | < 500 ms | **205 ms** | 340 ms | 1,027 ms ✗ |
| Idle memory, whole process tree (PSS) | < 150 MB | 204 MB ✗ | 408 MB ✗ | 386 MB ✗ |
| Processes | | 1 | 3 | 3 |
| Idle CPU | | 0.4% | 5%¹ | 0.0% |
| Scroll frame time p50 / p95 | 60 fps | 48 / 48 ms² | 48 / 48 ms² | **19 / 25 ms** |
| Blank rows while scrolling | none | none | none | none (0 of 3,000 frames) |
| Keypress to visible, real key, p50 / p95 | < 16 ms | **12 / 19 ms** | keys never arrived ✗ | 51 / 71 ms ✗ |
| Keypress, in-app handler to frame, p95 | < 16 ms | **14 ms** | 15 ms (self-driven) | 59 ms ✗ |
| Open a message (sanitise + render) p50 / p95 | < 50 ms | 8 / 15 ms³ | 47 / 68 ms ✗ | 49 / 61 ms ✗ |
| Sanitise only, p50 | | 2.5 ms | 2.4 ms | 2.0 ms |
| HTML fidelity | faithful | **broken layout** | faithful | faithful |
| Stripped release binary | | 40 MB⁴ | 40 MB⁴ | 8 MB |
| Crates in the dependency tree | | 566 | 566 | 252 |

¹ Measured with a shorter settle time than the other two columns.
² GPUI ticks at 16 ms on this display; each software-rendered frame of the full window took two to three ticks. Not meaningful for a real GPU.
³ Fast because the native view renders only a simplified version of the message.
⁴ Includes GPUI Kit's full component library, wry and GTK. A trimmed GPUI build would be smaller.

**Reading the numbers:** with no GPU, both toolkits miss the scrolling and memory targets, so those columns compare the two only roughly. Memory especially: lavapipe and WebKit's software compositor both inflate it. The comparisons that do carry over to real hardware are the structural ones: GPUI alone can't render the newsletter, GPUI plus a webview loses its memory edge and its keyboard, and Tauri needed no blank-row placeholders at 500k rows when fetching rows from Rust a page at a time.

## What still needs measuring

- **Windows (primary):** cold start, WebView2 memory (it shares the Edge runtime, so private bytes matter more than working set), keypress p95 and scroll on Marc's GPU. Run `bench\run_windows.ps1`; it covers all three variants and prints the same table.
- **macOS:** not available here; not needed for the decision.

## Notes for Phase 3

- A browser element can only be about 33.5 million pixels tall. At 56 px rows, 500,000 rows (28 million px) fits, but past about 600,000 the list needs scaled virtual scrolling. Marc's largest mailbox is about 9,500 messages, so this is a note, not a risk.
- The Tauri prototype sanitises in Rust (`ammonia`) and shows the result in `<iframe sandbox="">` with no `allow-scripts` or `allow-same-origin` and a CSP that blocks every network load. The sanitiser test checks that script, handlers, forms, iframes, `javascript:` links and remote images are all gone.
- WebKit keeps painting for several seconds after a long scroll (up to about 150% CPU for roughly 10 seconds here) before it settles to 0%. Worth checking on WebView2.
