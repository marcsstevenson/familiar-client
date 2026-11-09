// The virtualised message list. Rust holds the sorted IDs of the view; this
// component asks for the rows on screen a page at a time and draws only those.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { isCommandError, run } from "../api";
import { listNav, processMessage, processedFolders, removeFromList, setDragged, searchQuery, selectedFolder, selectedMessage, setSelectedMessage, setSelectedRow } from "../state";
import { OnlineHits, SearchBar } from "./SearchBar";
import { type Row, type Sort, listIndexOf, listOpen, listRows, onStoreChanged } from "./api";
import { shortDate } from "./format";
import { MessageMenu } from "./MessageMenu";

export const ROW_H = 64;
const PAGE = 100;
const OVERSCAN = 8;

const SORTS: { value: Sort; label: string }[] = [
  { value: "newest", label: "Newest" },
  { value: "oldest", label: "Oldest" },
  { value: "from", label: "From" },
  { value: "subject", label: "Subject" },
];

export function MessageList() {
  let scroller!: HTMLDivElement;
  const [total, setTotal] = createSignal(0);
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewH, setViewH] = createSignal(600);
  const [sort, setSort] = createSignal<Sort>("newest");
  const [unreadOnly, setUnreadOnly] = createSignal(false);
  const [selIndex, setSelIndex] = createSignal<number | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [rev, setRev] = createSignal(0);
  const [notices, setNotices] = createSignal<string[]>([]);
  // The message whose context menu is open, and where.
  const [menu, setMenu] = createSignal<{ row: Row; x: number; y: number } | null>(null);
  // On the listbox, where focus is: a right click opens the menu for the row
  // under the pointer (selecting it), the menu key or Shift+F10 for the
  // selected row.
  const openMenu = (e: MouseEvent) => {
    e.preventDefault();
    const keyboard = e.clientX === 0 && e.clientY === 0;
    const under = (e.target as HTMLElement).closest(".list-row")?.id.match(/^msg-(\d+)$/);
    if (!keyboard && under) select(Number(under[1]));
    const i = keyboard ? selIndex() : under ? Number(under[1]) : null;
    const row = i === null ? undefined : rowAt(i);
    if (i === null || !row) return;
    const r = document.getElementById(`msg-${i}`)?.getBoundingClientRect();
    setMenu({
      row: { ...row },
      x: keyboard && r ? r.left + 24 : e.clientX,
      y: keyboard && r ? r.bottom : e.clientY,
    });
  };

  // Pages of rows for the current view generation.
  let pages = new Map<number, Row[]>();
  let inflight = new Map<number, Promise<void>>();
  let gen = 0;

  /** Loads a page; a page already loading returns the same promise. */
  function loadPage(p: number): Promise<void> {
    if (p < 0 || p * PAGE >= total() || pages.has(p)) return Promise.resolve();
    const pending = inflight.get(p);
    if (pending) return pending;
    const g = gen;
    const job = (async () => {
      try {
        const rows = await listRows(p * PAGE, PAGE);
        if (g !== gen) return;
        pages.set(p, rows);
        setRev((n) => n + 1);
      } catch (e) {
        if (g === gen) setError(isCommandError(e) ? e.message : String(e));
      } finally {
        if (g === gen) inflight.delete(p);
      }
    })();
    inflight.set(p, job);
    return job;
  }

  // Reads `rev` so rows redraw when their page arrives.
  const rowAt = (i: number): Row | undefined => {
    rev();
    return pages.get(Math.floor(i / PAGE))?.[i % PAGE];
  };

  /** Reopens the view; keeps the scroll position when `keepScroll`. */
  async function reopen(keepScroll: boolean) {
    const g = ++gen;
    const wasAt = selIndex();
    try {
      const opened = await listOpen({
        folders: selectedFolder() === null ? [] : [selectedFolder()!],
        unread_only: unreadOnly(),
        sort: sort(),
        query: searchQuery(),
      });
      const n = opened.total;
      if (g !== gen) return;
      setError(null);
      setNotices([
        ...opened.notices,
        ...(opened.capped ? [`Showing the newest ${n.toLocaleString()} matches`] : []),
      ]);
      // On a refresh, fetch the rows on screen before swapping them in, so the
      // list doesn't flash empty while famd syncs.
      const fresh = new Map<number, Row[]>();
      if (keepScroll) {
        for (const p of pagesOnScreen()) {
          if (p * PAGE < n) fresh.set(p, await listRows(p * PAGE, PAGE));
          if (g !== gen) return;
        }
      }
      pages = fresh;
      inflight = new Map();
      setTotal(n);
      if (!keepScroll) {
        scroller.scrollTop = 0;
        setScrollTop(0);
      }
      const sel = selectedMessage();
      const ix = sel === null ? null : await listIndexOf(sel);
      if (g !== gen) return;
      if (ix === null && sel !== null && sel === listNav.leaving && wasAt !== null) {
        // The open message was deleted or moved: open the one that took its
        // place (the next one down), or the new last one.
        listNav.leaving = null;
        setRev((x) => x + 1);
        prefetch();
        if (n === 0) setSelectedMessage(null);
        else select(Math.min(wasAt, n - 1));
        return;
      }
      // A key may have moved the selection while this looked up the old one;
      // then the selection already has its new place.
      if (selectedMessage() === sel) {
        setSelIndex(ix);
        if (ix !== null && !keepScroll) scrollIntoView(ix);
      }
      setRev((x) => x + 1);
      prefetch();
    } catch (e) {
      if (g !== gen) return;
      setTotal(0);
      setError(isCommandError(e) ? e.message : String(e));
    }
  }

  /** Pages covering the rows on screen plus the overscan. */
  function pagesOnScreen(): number[] {
    const first = Math.max(0, Math.floor(scrollTop() / ROW_H) - OVERSCAN);
    const last = Math.ceil((scrollTop() + viewH()) / ROW_H) + OVERSCAN;
    const out: number[] = [];
    for (let p = Math.floor(first / PAGE); p <= Math.floor(last / PAGE); p++) out.push(p);
    return out;
  }

  /** Loads the pages on screen and the one after. */
  function prefetch() {
    const ps = pagesOnScreen();
    for (const p of [...ps, (ps[ps.length - 1] ?? 0) + 1]) void loadPage(p);
  }

  const visible = () => {
    rev();
    const first = Math.max(0, Math.floor(scrollTop() / ROW_H) - OVERSCAN);
    const last = Math.min(total(), Math.ceil((scrollTop() + viewH()) / ROW_H) + OVERSCAN);
    const out: number[] = [];
    for (let i = first; i < last; i++) out.push(i);
    return out;
  };

  createEffect(on([selectedFolder, sort, unreadOnly, searchQuery], () => void reopen(false)));

  // The selected row's flags, for actions such as "mark unread". A selected
  // message whose row has left the view (the Unread filter, once opening it
  // marked it read) keeps its last flags, now read.
  createEffect(() => {
    const i = selIndex();
    const r = i === null ? undefined : rowAt(i);
    const id = selectedMessage();
    setSelectedRow((prev) =>
      r && r.id === id
        ? { id: r.id, account: r.account, is_read: r.is_read, is_flagged: r.is_flagged }
        : prev && prev.id === id
          ? { ...prev, is_read: true }
          : null,
    );
  });

  // `fam ui open` and other code can select a message by ID.
  createEffect(
    on(selectedMessage, async (id) => {
      if (id === null) return setSelIndex(null);
      const cur = selIndex();
      if (cur !== null && rowAt(cur)?.id === id) return;
      const ix = await listIndexOf(id);
      if (ix !== null && selectedMessage() === id) {
        setSelIndex(ix);
        scrollIntoView(ix);
      }
    }, { defer: true }),
  );

  onMount(async () => {
    const ro = new ResizeObserver(() => {
      setViewH(scroller.clientHeight);
      prefetch();
    });
    ro.observe(scroller);
    listNav.move = (d) => select((selIndex() ?? -1) + d);
    const stop = await onStoreChanged(() => void reopen(true));
    onCleanup(() => {
      ro.disconnect();
      stop();
    });
  });

  function scrollIntoView(ix: number) {
    const top = ix * ROW_H;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + ROW_H > scroller.scrollTop + scroller.clientHeight) {
      scroller.scrollTop = top + ROW_H - scroller.clientHeight;
    }
  }

  function select(i: number) {
    if (total() === 0) return;
    const ix = Math.max(0, Math.min(total() - 1, i));
    setSelIndex(ix);
    scrollIntoView(ix);
    const row = rowAt(ix);
    if (row) setSelectedMessage(row.id);
    else {
      // Not loaded yet: load its page, then open it if still selected.
      void loadPage(Math.floor(ix / PAGE)).then(() => {
        const r = rowAt(ix);
        if (r && selIndex() === ix) setSelectedMessage(r.id);
      });
    }
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const cur = selIndex() ?? -1;
    const page = Math.max(1, Math.floor(scroller.clientHeight / ROW_H) - 1);
    const moves: Record<string, number> = {
      ArrowDown: cur + 1,
      ArrowUp: cur - 1,
      PageDown: cur + page,
      PageUp: cur - page,
      Home: 0,
      End: total() - 1,
    };
    const to = moves[e.key];
    if (to === undefined) return;
    e.preventDefault();
    select(to);
  }

  return (
    <div class="message-list">
      {/* Sort, Unread and the count show while the pointer is over the search
          box (or they have focus), or while they aren't the defaults. */}
      <div class="list-top" classList={{ "show-controls": unreadOnly() || sort() !== "newest" }}>
      <SearchBar />
      <div class="list-header">
        <label class="list-control">
          <span class="visually-hidden">Sort by</span>
          <select
            value={searchQuery() ? "newest" : sort()}
            disabled={searchQuery() !== null}
            title={searchQuery() ? "Search results are newest first" : undefined}
            onChange={(e) => setSort(e.currentTarget.value as Sort)}
          >
            <For each={SORTS}>{(s) => <option value={s.value}>{s.label}</option>}</For>
          </select>
        </label>
        <label class="list-control">
          <input
            type="checkbox"
            checked={unreadOnly()}
            onChange={(e) => setUnreadOnly(e.currentTarget.checked)}
          />
          Unread
        </label>
        <span class="list-count">
          {total().toLocaleString()} {searchQuery() ? "found" : "messages"}
        </span>
      </div>
      </div>
      <For each={notices()}>{(n) => <p class="list-notice">{n}</p>}</For>
      <OnlineHits />
      <div
        class="list-scroller"
        ref={scroller}
        tabindex="0"
        role="listbox"
        aria-label="Messages"
        aria-activedescendant={selIndex() === null ? undefined : `msg-${selIndex()}`}
        onScroll={(e) => {
          setScrollTop(e.currentTarget.scrollTop);
          prefetch();
        }}
        onKeyDown={onKeyDown}
        onContextMenu={openMenu}
      >
        <Show when={error()}>{(e) => <p class="pane-note">{e()}</p>}</Show>
        <Show when={!error() && total() === 0}>
          <div class="pane-empty">No messages</div>
        </Show>
        <div class="list-spacer" style={{ height: `${total() * ROW_H}px` }}>
          <For each={visible()}>
            {(i) => <ListRow index={i} row={rowAt(i)} selected={selIndex() === i} onSelect={select} unified={selectedFolder() === null || searchQuery() !== null} />}
          </For>
        </div>
      </div>
      <Show when={menu()}>{(m) => <MessageMenu row={m().row} x={m().x} y={m().y} onClose={() => {
        setMenu(null);
        scroller.focus();
      }} />}</Show>
    </div>
  );
}

function ListRow(props: {
  index: number;
  row: Row | undefined;
  selected: boolean;
  unified: boolean;
  onSelect: (i: number) => void;
}) {
  return (
    <div
      id={`msg-${props.index}`}
      class="list-row"
      classList={{ selected: props.selected, unread: props.row ? !props.row.is_read : false }}
      role="option"
      aria-selected={props.selected}
      style={{ transform: `translateY(${props.index * ROW_H}px)` }}
      onMouseDown={() => props.onSelect(props.index)}
      draggable={props.row !== undefined}
      onDragStart={(e) => {
        const r = props.row;
        if (!r || !e.dataTransfer) return;
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", r.subject);
        setDragged({ id: r.id, account: r.account });
      }}
      onDragEnd={() => setDragged(null)}
    >
      <Show when={props.row} fallback={<div class="row-placeholder" />}>
        {(r) => (
          <>
            <div class="row-line">
              <span class="row-from">{r().from}</span>
              <Show when={r().has_attachments}>
                <span class="row-icon" title="Has attachments" aria-label="Has attachments">📎</span>
              </Show>
              <Show when={r().is_flagged}>
                <span class="row-icon flag" title="Flagged" aria-label="Flagged">⚑</span>
              </Show>
              <span class="row-date">{shortDate(r().received_at)}</span>
            </div>
            <div class="row-line">
              <Show when={r().importance === 2}>
                <span class="row-icon important" aria-label="High importance">!</span>
              </Show>
              <span class="row-subject">{r().subject || "(no subject)"}</span>
              <Show when={props.unified}>
                <span class="row-account">{r().account}</span>
              </Show>
            </div>
            <div class="row-preview">{r().preview}</div>
            {/* For the mouse; the keyboard has Delete (and a key for Processed
                if one is set). Not buttons inside the option, so screen
                readers keep one stop per row. */}
            <Show when={processedFolders[r().account]}>
              {(folder) => (
                <span
                  class="row-action row-process"
                  title={`Processed: move to ${folder()}`}
                  aria-hidden="true"
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={(e) => {
                    e.stopPropagation();
                    void processMessage(r().id, r().account);
                  }}
                >
                  ✓
                </span>
              )}
            </Show>
            <span
              class="row-action row-delete"
              title="Delete"
              aria-hidden="true"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                const id = r().id;
                void removeFromList(id, "Moved to Deleted Items", () => run("mail.delete", { messages: [id] }));
              }}
            >
              🗑
            </span>
          </>
        )}
      </Show>
    </div>
  );
}
