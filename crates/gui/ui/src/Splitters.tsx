// The two splitters between the panes. The folder and list panes keep the
// widths set here (kept between runs); the reading pane takes whatever the
// window has left, so resizing the window only changes the reading pane.
import { createSignal } from "solid-js";

const KEY = "familiar.panes";
const DEFAULTS = { folders: 240, list: 400 };
const MIN = { folders: 140, list: 260 };
/** The reading pane keeps at least this much while dragging. */
const MIN_READING = 320;
const STEP = 16;

type Widths = { folders: number; list: number };

function load(): Widths {
  try {
    const w = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<Widths> | null;
    if (w && Number.isFinite(w.folders) && Number.isFinite(w.list)) {
      return { folders: Math.max(MIN.folders, w.folders!), list: Math.max(MIN.list, w.list!) };
    }
  } catch {
    // Defaults below.
  }
  return { ...DEFAULTS };
}

function save(w: Widths) {
  try {
    localStorage.setItem(KEY, JSON.stringify(w));
  } catch {
    // Not kept; the widths still apply until the window closes.
  }
}

export const [paneWidths, setPaneWidths] = createSignal<Widths>(load());

/** `grid-template-columns` for the panes. */
export const paneColumns = () => `${paneWidths().folders}px ${paneWidths().list}px 1fr`;

/** Sets pane `which` to `px`, kept within its minimum and leaving the reading pane room. */
function resize(which: keyof Widths, px: number) {
  const w = paneWidths();
  const other = which === "folders" ? w.list : w.folders;
  const max = Math.max(MIN[which], window.innerWidth - other - MIN_READING);
  setPaneWidths({ ...w, [which]: Math.round(Math.min(max, Math.max(MIN[which], px))) });
}

function Splitter(props: { which: keyof Widths; label: string; left: number }) {
  let startX = 0;
  let startW = 0;
  return (
    <div
      class="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      aria-valuenow={paneWidths()[props.which]}
      aria-valuemin={MIN[props.which]}
      tabindex="0"
      title="Drag to resize; double-click to reset"
      style={{ left: `${props.left - 3}px` }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        startX = e.clientX;
        startW = paneWidths()[props.which];
      }}
      onPointerMove={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
        resize(props.which, startW + e.clientX - startX);
      }}
      onPointerUp={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
        e.currentTarget.releasePointerCapture(e.pointerId);
        save(paneWidths());
      }}
      onDblClick={() => {
        resize(props.which, DEFAULTS[props.which]);
        save(paneWidths());
      }}
      onKeyDown={(e) => {
        const d = e.key === "ArrowLeft" ? -STEP : e.key === "ArrowRight" ? STEP : 0;
        if (d === 0) return;
        e.preventDefault();
        e.stopPropagation();
        resize(props.which, paneWidths()[props.which] + d);
        save(paneWidths());
      }}
    />
  );
}

export function Splitters() {
  return (
    <>
      <Splitter which="folders" label="Resize the folder pane" left={paneWidths().folders} />
      <Splitter which="list" label="Resize the message list" left={paneWidths().folders + paneWidths().list} />
    </>
  );
}
