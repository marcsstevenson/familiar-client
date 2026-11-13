// The attachment viewer: covers the window below the title bar and shows one
// attachment at a time, with Previous/Next through the message's files.
//
// PDFs go to the webview's own PDF viewer (WebView2's on Windows) as a
// `blob:` URL in a frame. It parses the file in its own sandboxed process,
// so an untrusted PDF never runs inside this window. Images get fit, zoom
// and actual size; text is shown as is.
import { Match, Show, Switch, createEffect, createSignal, on, onCleanup } from "solid-js";
import { type AttachmentInfo, type Preview, errorText, previewAttachment } from "./api";
import { humanSize } from "./Attachments";

type Loaded = { state: "loading" } | { state: "ready"; p: Preview } | { state: "error"; text: string };

const ZOOMS = [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 3, 4, 6, 8];

export function AttachmentViewer(props: {
  message: number;
  files: AttachmentInfo[];
  index: number;
  /** The preview already fetched for `index`, so it isn't fetched twice. */
  first: Preview;
  onIndex: (i: number) => void;
  onSave: (a: AttachmentInfo) => void;
  onClose: () => void;
}) {
  const [loaded, setLoaded] = createSignal<Loaded>({ state: "ready", p: props.first });
  // null = fit to the window; otherwise a scale of the image's real size.
  const [zoom, setZoom] = createSignal<number | null>(null);
  const [natural, setNatural] = createSignal<{ w: number; h: number } | null>(null);
  let root!: HTMLDivElement;
  let imgBox: HTMLDivElement | undefined;
  let skipFirst = true;

  const current = () => props.files[props.index]!;

  createEffect(
    on(
      () => props.index,
      (i) => {
        setZoom(null);
        setNatural(null);
        if (skipFirst) {
          skipFirst = false;
          return;
        }
        setLoaded({ state: "loading" });
        const a = props.files[i]!;
        previewAttachment(props.message, a.id)
          .then((p) => props.index === i && setLoaded({ state: "ready", p }))
          .catch((e) => props.index === i && setLoaded({ state: "error", text: errorText(e) }));
      },
    ),
  );

  const step = (d: number) => {
    const n = props.files.length;
    if (n > 1) props.onIndex((props.index + d + n) % n);
  };

  const image = () => {
    const l = loaded();
    return l.state === "ready" && l.p.kind === "image" ? l.p : null;
  };

  // The scale the image is shown at right now, for the zoom buttons.
  const scale = () => {
    const z = zoom();
    if (z !== null) return z;
    const n = natural();
    if (!n || !imgBox) return 1;
    return Math.min(1, imgBox.clientWidth / n.w, imgBox.clientHeight / n.h);
  };
  const zoomBy = (dir: 1 | -1) => {
    const s = scale();
    const next = dir > 0 ? ZOOMS.find((z) => z > s + 0.001) : [...ZOOMS].reverse().find((z) => z < s - 0.001);
    if (next !== undefined) setZoom(next);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
    else if (e.key === "ArrowLeft") step(-1);
    else if (e.key === "ArrowRight") step(1);
    else if (image() && (e.key === "+" || e.key === "=")) zoomBy(1);
    else if (image() && e.key === "-") zoomBy(-1);
    else if (image() && e.key === "0") setZoom(null);
    else if (image() && e.key === "1") setZoom(1);
    else return;
    e.preventDefault();
  };

  const onWheel = (e: WheelEvent) => {
    if (!e.ctrlKey || !image()) return;
    e.preventDefault();
    zoomBy(e.deltaY < 0 ? 1 : -1);
  };

  queueMicrotask(() => root.focus());

  return (
    <div
      class="attachment-viewer"
      role="dialog"
      aria-modal="true"
      aria-label={`Viewing ${current().name}`}
      tabindex="-1"
      ref={root}
      onKeyDown={onKey}
    >
      <div class="attachment-viewer-bar">
        <Show when={props.files.length > 1}>
          <button onClick={() => step(-1)} title="Previous attachment (Left arrow)" aria-label="Previous attachment">
            ‹
          </button>
          <span class="muted">
            {props.index + 1} of {props.files.length}
          </span>
          <button onClick={() => step(1)} title="Next attachment (Right arrow)" aria-label="Next attachment">
            ›
          </button>
        </Show>
        <span class="attachment-viewer-name" title={current().name}>
          {current().name}
        </span>
        <span class="muted">{humanSize(current().size)}</span>
        <span class="statusbar-spacer" />
        <Show when={image()}>
          <button onClick={() => zoomBy(-1)} title="Zoom out (-)" aria-label="Zoom out">
            −
          </button>
          <span class="attachment-viewer-zoom muted">{Math.round(scale() * 100)}%</span>
          <button onClick={() => zoomBy(1)} title="Zoom in (+)" aria-label="Zoom in">
            +
          </button>
          <button onClick={() => setZoom(null)} aria-pressed={zoom() === null} title="Fit to window (0)">
            Fit
          </button>
          <button onClick={() => setZoom(1)} aria-pressed={zoom() === 1} title="Actual size (1)">
            100%
          </button>
        </Show>
        <button onClick={() => props.onSave(current())}>Save</button>
        <button onClick={props.onClose} aria-label="Close viewer" title="Close (Esc)">
          Close
        </button>
      </div>
      <Switch>
        <Match when={loaded().state === "loading"}>
          <div class="attachment-viewer-empty muted">Loading {current().name}…</div>
        </Match>
        <Match when={loaded().state === "error" && (loaded() as { text: string })}>
          {(l) => <div class="attachment-viewer-empty">Couldn't open {current().name}: {l().text}</div>}
        </Match>
        <Match when={image()}>
          {(p) => (
            <div
              class="attachment-viewer-image"
              classList={{ fit: zoom() === null }}
              ref={imgBox}
              onWheel={onWheel}
            >
              <img
                src={p().src}
                alt={current().name}
                style={zoom() !== null && natural() ? { width: `${natural()!.w * zoom()!}px` } : undefined}
                onLoad={(e) => setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
                onClick={() => setZoom(zoom() === null ? 1 : null)}
              />
            </div>
          )}
        </Match>
        <Match when={loaded().state === "ready" && (loaded() as { p: Preview }).p}>
          {(p) => (
            <Switch>
              <Match when={p().kind === "pdf" && (p() as { data: string })} keyed>
                {(pdf) => <PdfFrame data={pdf.data} name={current().name} />}
              </Match>
              <Match when={p().kind === "text" && (p() as { text: string })}>
                {(t) => <pre class="attachment-viewer-text">{t().text}</pre>}
              </Match>
              <Match when={p().kind === "none"}>
                <div class="attachment-viewer-empty muted">
                  No preview for {current().name}. Use Save to open it in its own app.
                </div>
              </Match>
            </Switch>
          )}
        </Match>
      </Switch>
    </div>
  );
}

function PdfFrame(props: { data: string; name: string }) {
  const bin = atob(props.data);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  const url = URL.createObjectURL(new Blob([bytes], { type: "application/pdf" }));
  onCleanup(() => URL.revokeObjectURL(url));
  return <iframe class="attachment-viewer-pdf" src={url} title={props.name} />;
}
