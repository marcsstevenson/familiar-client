//! WP 0.4 GPUI prototype: three panes, a virtualised list of 500k synthetic
//! rows, and a reading pane showing a sanitised HTML newsletter.
//!
//! GPUI has no web engine, so the reading pane has two modes:
//! - `FAMILIAR_GPUI_HTML=native` (default): GPUI Kit's native HTML view, which
//!   renders a Markdown-like subset (no CSS layout, no `<style>`).
//! - `FAMILIAR_GPUI_HTML=webview`: a system webview (wry: WebView2, WKWebView,
//!   WebKitGTK) placed over the reading pane as a child window, which is what a
//!   real GPUI mail client would need for faithful HTML.
//!
//! Bench mode: set FAMILIAR_BENCH_OUT=<file>. The app then scrolls, moves the
//! selection and opens messages by itself and writes JSON lines to the file.
//! External key presses (j/k) are timed in every mode.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bakeoff_common::{self as common, Bench, ROW_COUNT, now_ns};
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;

actions!(mail, [SelectNext, SelectPrev, Quit]);

const ROW_H: f32 = 56.0;

struct Mail {
    focus: FocusHandle,
    list: UniformListScrollHandle,
    selected: usize,
    html: SharedString,
    html_rev: usize,
    /// Native-mode reading pane, a separate cached view so scrolling the
    /// list does not re-lay-out the message.
    reading: Entity<ReadingPane>,
    bench: Arc<Bench>,
    first_frame_done: bool,
    use_webview: bool,
    webview: Option<wry::WebView>,
    /// Open waiting for the webview's page-load event (webview mode only).
    pending_web: Arc<Mutex<Option<Pending>>>,
    /// Set when a selection change is waiting for its frame.
    pending: Option<Pending>,
}

#[derive(Clone, Copy)]
struct Pending {
    kind: &'static str,
    t_start: u128,
    sanitize_ns: u128,
}

impl Mail {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut m = Mail {
            focus,
            list: UniformListScrollHandle::new(),
            selected: 0,
            html: SharedString::default(),
            html_rev: 0,
            reading: cx.new(|_| ReadingPane {
                html: SharedString::default(),
                rev: 0,
            }),
            bench: Arc::new(Bench::from_env()),
            first_frame_done: false,
            use_webview: std::env::var("FAMILIAR_GPUI_HTML").as_deref() == Ok("webview"),
            webview: None,
            pending_web: Arc::new(Mutex::new(None)),
            pending: None,
        };
        m.load_html(cx);
        m
    }

    fn load_html(&mut self, cx: &mut Context<Self>) -> u128 {
        let t = now_ns();
        self.html = common::sanitize(&common::newsletter_html(self.selected)).into();
        self.html_rev += 1;
        let dt = now_ns() - t;
        if !self.use_webview {
            let (html, rev) = (self.html.clone(), self.html_rev);
            self.reading.update(cx, |r, cx| {
                r.html = html;
                r.rev = rev;
                cx.notify();
            });
        }
        dt
    }

    fn select(
        &mut self,
        ix: usize,
        kind: &'static str,
        t_start: u128,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = ix.min(ROW_COUNT - 1);
        self.list
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        let sanitize_ns = self.load_html(cx);
        let p = Pending {
            kind,
            t_start,
            sanitize_ns,
        };
        if let Some(wv) = &self.webview {
            if kind == "open" {
                *self.pending_web.lock().unwrap() = Some(p);
            }
            let _ = wv.load_html(&self.html);
        }
        if !(self.webview.is_some() && kind == "open") {
            self.pending = Some(p);
        }
        cx.notify();
        // Timed when the frame that shows the change has been drawn.
        cx.on_next_frame(window, |this, _w, _cx| {
            if let Some(p) = this.pending.take() {
                let t = now_ns();
                this.bench.event(
                    p.kind,
                    json!({
                        "start_ns": p.t_start.to_string(),
                        "latency_ms": (t - p.t_start) as f64 / 1e6,
                        "sanitize_ms": p.sanitize_ns as f64 / 1e6,
                    }),
                );
            }
        });
    }

    fn on_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected + 1, "key", now_ns(), window, cx);
    }
    fn on_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected.saturating_sub(1), "key", now_ns(), window, cx);
    }

    fn run_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            // 1. Scroll: 40 px per frame for 600 frames, recording frame times.
            let frames = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u128>::with_capacity(601)));
            for i in 0..600 {
                let (tx, rx) = futures::channel::oneshot::channel::<()>();
                let frames2 = frames.clone();
                this.update_in(cx, |this, window, cx| {
                    let h = &this.list.0.borrow().base_handle;
                    let mut off = h.offset();
                    off.y -= px(40.0);
                    h.set_offset(off);
                    cx.notify();
                    cx.on_next_frame(window, move |_, _, _| {
                        frames2.borrow_mut().push(now_ns());
                        let _ = tx.send(());
                    });
                    let _ = i;
                })
                .ok();
                let _ = rx.await;
            }
            this.update(cx, |this, _| {
                let f = frames.borrow();
                let d: Vec<f64> = f.windows(2).map(|w| (w[1] - w[0]) as f64 / 1e6).collect();
                this.bench.event("scroll", json!({ "frame_ms": d }));
            })
            .ok();

            // 2. Self-driven selection moves and message opens, 30 of each.
            for i in 0..30 {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                this.update_in(cx, |this, window, cx| {
                    let kind = if i % 2 == 0 { "self_key" } else { "open" };
                    let next = if kind == "open" {
                        1000 + i * 7919
                    } else {
                        this.selected + 1
                    };
                    this.select(next, kind, now_ns(), window, cx);
                })
                .ok();
            }
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, _| this.bench.event("bench_done", json!({})))
                .ok();
        })
        .detach();
    }

    fn attach_webview(&mut self, window: &mut Window) {
        let vs = window.viewport_size();
        let pending = self.pending_web.clone();
        let bench = self.bench.clone();
        let wv = wry::WebViewBuilder::new()
            .with_html(self.html.to_string())
            .with_bounds(reading_bounds(vs.width.into(), vs.height.into()))
            .with_navigation_handler(|url| url.starts_with("about:") || url.starts_with("data:"))
            .with_on_page_load_handler(move |ev, _url| {
                if matches!(ev, wry::PageLoadEvent::Finished) {
                    if let Some(p) = pending.lock().unwrap().take() {
                        bench.event(
                            p.kind,
                            json!({
                                "start_ns": p.t_start.to_string(),
                                "latency_ms": (now_ns() - p.t_start) as f64 / 1e6,
                                "sanitize_ms": p.sanitize_ns as f64 / 1e6,
                            }),
                        );
                    }
                }
            })
            .build_as_child(&ParentHandle::of(window))
            .expect("child webview");
        self.webview = Some(wv);
    }

    fn render_row(&self, ix: usize) -> impl IntoElement + use<> {
        let r = common::row(ix);
        let sel = ix == self.selected;
        div()
            .id(ix)
            .h(px(ROW_H))
            .px_3()
            .py_1()
            .line_height(px(16.0))
            .flex()
            .flex_col()
            .justify_center()
            .border_b_1()
            .border_color(rgb(0xe6e8ec))
            .bg(if sel { rgb(0xdbe7f8) } else { rgb(0xffffff) })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_sm()
                    .child(
                        div()
                            .when(r.unread, |d| d.font_weight(FontWeight::BOLD))
                            .child(r.from),
                    )
                    .child(div().text_color(rgb(0x6b7385)).child(r.date)),
            )
            .child(div().text_sm().truncate().child(r.subject))
            .child(
                div()
                    .text_xs()
                    .truncate()
                    .text_color(rgb(0x6b7385))
                    .child(r.preview),
            )
    }
}

impl Render for Mail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_done {
            self.first_frame_done = true;
            cx.on_next_frame(window, |this, window, cx| {
                if this.use_webview {
                    this.attach_webview(window);
                }
                this.bench.event("first_frame", json!({}));
                if this.bench.enabled() {
                    this.run_bench(window, cx);
                }
            });
        }
        let folders = div()
            .w(px(220.0))
            .h_full()
            .bg(rgb(0xf4f5f7))
            .border_r_1()
            .border_color(rgb(0xe0e2e6))
            .p_2()
            .flex()
            .flex_col()
            .children(common::FOLDERS.iter().enumerate().map(|(i, f)| {
                div()
                    .px_2()
                    .py(px(3.0))
                    .text_sm()
                    .rounded_sm()
                    .when(i == 0, |d| d.bg(rgb(0xdbe7f8)))
                    .child(SharedString::from(*f))
            }));
        let list = uniform_list(
            "messages",
            ROW_COUNT,
            cx.processor(|this, range: std::ops::Range<usize>, _w, _cx| {
                range.map(|ix| this.render_row(ix)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list)
        .w(px(400.0))
        .h_full()
        .border_r_1()
        .border_color(rgb(0xe0e2e6));
        let reading = div()
            .flex_1()
            .h_full()
            .bg(rgb(0xffffff))
            .when(!self.use_webview, |d| {
                d.child(
                    self.reading
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
            });
        if let Some(wv) = &self.webview {
            let vs = window.viewport_size();
            let _ = wv.set_bounds(reading_bounds(vs.width.into(), vs.height.into()));
        }
        div()
            .key_context("Mail")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .size_full()
            .flex()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x1d2330))
            .font_family("DejaVu Sans")
            .child(folders)
            .child(list)
            .child(reading)
    }
}

/// The parent window handle for the child webview. On Windows and macOS this
/// is GPUI's own handle. On Linux GPUI hands out an XCB handle but wry only
/// accepts Xlib, so the same X window id is re-wrapped. (Wayland has no
/// child-webview support in wry at all.)
struct ParentHandle(raw_window_handle::RawWindowHandle);

impl ParentHandle {
    fn of(window: &Window) -> Self {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let raw = HasWindowHandle::window_handle(window)
            .expect("window handle")
            .as_raw();
        #[cfg(target_os = "linux")]
        if let RawWindowHandle::Xcb(h) = raw {
            return ParentHandle(RawWindowHandle::Xlib(
                raw_window_handle::XlibWindowHandle::new(h.window.get() as _),
            ));
        }
        ParentHandle(raw)
    }
}

impl raw_window_handle::HasWindowHandle for ParentHandle {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the handle comes from a live GPUI window that outlives the webview.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

struct ReadingPane {
    html: SharedString,
    rev: usize,
}

impl Render for ReadingPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("reading")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .child(TextView::html(("html", self.rev), self.html.clone()))
    }
}

fn reading_bounds(w: f32, h: f32) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(620.0, 0.0).into(),
        size: wry::dpi::LogicalSize::new((w - 620.0).max(1.0), h).into(),
    }
}

fn main() {
    let t_main = now_ns();
    #[cfg(target_os = "linux")]
    if std::env::var("FAMILIAR_GPUI_HTML").as_deref() == Ok("webview") {
        gtk::init().expect("gtk init");
    }
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Bench::from_env().event("main", json!({ "main_ns": t_main.to_string() }));
        cx.bind_keys([
            KeyBinding::new("j", SelectNext, Some("Mail")),
            KeyBinding::new("down", SelectNext, Some("Mail")),
            KeyBinding::new("k", SelectPrev, Some("Mail")),
            KeyBinding::new("up", SelectPrev, Some("Mail")),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Familiar (GPUI)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Mail::new(window, cx)),
        )
        .expect("open window");
        cx.activate(true);
        // WebKitGTK needs the GTK main loop; pump it from GPUI's loop.
        #[cfg(target_os = "linux")]
        if std::env::var("FAMILIAR_GPUI_HTML").as_deref() == Ok("webview") {
            cx.spawn(async move |cx| {
                loop {
                    while gtk::events_pending() {
                        gtk::main_iteration_do(false);
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(4))
                        .await;
                }
            })
            .detach();
        }
    });
}
