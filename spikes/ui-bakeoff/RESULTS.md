# WP 0.4 results: Tauri 2 vs GPUI

3–4 Nov 2026 · Windows on Marc's PC (primary) and Linux in the cloud

## Recommendation

**Use Tauri 2 + SolidJS.** On Windows it met every speed target: cold start 442 ms, real keypress p95 11 ms, message open p95 30 ms and 165 fps scrolling at 500k rows. It misses the 150 MB idle memory target (399 MB private), but so does every option that can show HTML mail: GPUI with a webview used 351 MB, and it was slower to start (663 ms) and to open a message (68 ms). Only GPUI without a webview stayed small (142 MB), and it can't render a newsletter.

**Change to the design doc:** the 150 MB GUI memory target can't be met by any design that embeds a web engine. Raise it to about 400 MB private for the GUI, keep 50 MB for the headless daemon, and look for savings in Phase 3 (WebView2 memory-usage target level, one reading frame reused rather than recreated).

## Windows results

AMD Ryzen 9 7950X, 63 GB, NVIDIA RTX 4080 driving a 3840×2160 display at 165 Hz. WebView2 156, Rust 1.99.1 (MSVC). Release builds, 5 launches each, 30 real key presses per launch through SendKeys. Raw data: `bench/results/windows-*.json`.

| Measure | Target | GPUI, native HTML | GPUI + webview | Tauri 2 |
| --- | --- | --- | --- | --- |
| Cold start to usable inbox, median | < 500 ms | 514 ms ✗ | 663 ms ✗ | **442 ms** |
| Idle memory, private bytes (process tree) | < 150 MB | **142 MB** | 351 MB ✗ | 399 MB ✗ (290–405) |
| Idle memory, working set | | 105 MB | 468 MB | 557 MB |
| Processes | | 2 | 8 | 8 |
| Idle CPU (one core) | | 0.9% | 2.5% | 1.3% |
| Scroll frame p50 / p95 / p99 | 60 fps | 6.1 / 6.4 / 6.7 ms | 6.1 / 6.3 / 6.5 ms | 6.1 / 6.2 / 6.2 ms |
| Frames over 16.7 ms | none | 0% | 0% | 0% |
| Real keypress to frame, p95 | < 16 ms | 9.2 ms | 6.7 ms | 11.0 ms |
| Keypress, self-driven, p95 | < 16 ms | 6.2 ms | 6.1 ms | 2.8 ms |
| Open a message p50 / p95 | < 50 ms | 5.8 / 6.1 ms¹ | 66 / 68 ms ✗ | 22 / 30 ms |
| HTML newsletter | faithful | broken layout | faithful | faithful |
| Release binary | | 19.8 MB | 19.8 MB | 9.3 MB |

¹ Fast because the native view renders only a simplified version of the message.

All three scroll at the display's 165 Hz, so scrolling doesn't separate them. On Windows the GPUI + webview build did receive every key press (150 of 150), so the keyboard loss seen on Linux is an X11 child-window problem, not a Windows one. Focus would still move into the webview once the user clicks in a message, which a GPUI app would need to handle.

## Why Tauri over GPUI

The deciding result is HTML mail, not speed. Points 1, 2 and 4 come from building the prototypes; point 3 is about the Linux run.

1. **GPUI can't show a real newsletter by itself.** GPUI Kit's native HTML view drops the `<style>` block and ignores layout tables and most inline CSS, so the test newsletter comes out as a jumble of text and grey boxes ([screenshot](bench/results/gpui.png)). A GPUI mail client therefore has to embed a webview anyway, as the design doc predicted.
2. **Once GPUI embeds a webview, its advantages mostly go.** On Windows it used 351 MB private against Tauri's 399 MB, started slower and opened messages slower. On Linux it used *more* memory than Tauri (408 vs 386 MB PSS), and **none of the 150 real key presses reached the app** after the webview appeared. A child webview takes keyboard focus, so every shortcut needs focus hand-off code written per OS. On Linux, wry child webviews work on X11 only (no Wayland), GPUI's X11 handle had to be re-wrapped for wry, and the GTK main loop has to be pumped from GPUI's loop.
3. **Tauri's weak Linux numbers came from software rendering.** The cloud container has no GPU, so WebKitGTK and GPUI both drew on the CPU. Tauri missed the start and keypress targets there, but met them on Windows with a GPU. Linux desktop users with a GPU should be checked in Phase 3.
4. **Developer experience favours Tauri.** Half the dependencies (252 vs 566 crates), a smaller binary (9 MB vs 20 MB on Windows), a stable API, and a UI that Claude can drive and screenshot with ordinary web tooling (WebDriver on Windows and Linux, or the plain web UI in Playwright with the Rust commands mocked). GPUI is pre-1.0 and its crates are mid-split: `gpui` 0.2.2 on crates.io vs the `gpui-pre` 0.3.11 snapshot that GPUI Kit pins, and this small prototype hit several snags (a renamed prelude import, the XCB vs Xlib window handle mismatch with wry). To its credit, GPUI Kit now ships a headless UI test harness.

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

- **macOS:** not available; not needed for the decision.
- **Linux with a GPU:** WebKitGTK on real hardware, in Phase 3.

## Notes for Phase 3

- A browser element can only be about 33.5 million pixels tall. At 56 px rows, 500,000 rows (28 million px) fits, but past about 600,000 the list needs scaled virtual scrolling. Marc's largest mailbox is about 9,500 messages, so this is a note, not a risk.
- The Tauri prototype sanitises in Rust (`ammonia`) and shows the result in `<iframe sandbox="">` with no `allow-scripts` or `allow-same-origin` and a CSP that blocks every network load. The sanitiser test checks that script, handlers, forms, iframes, `javascript:` links and remote images are all gone.
- WebKit keeps painting for several seconds after a long scroll (up to about 150% CPU for roughly 10 seconds here) before it settles to 0%. Worth checking on WebView2.
