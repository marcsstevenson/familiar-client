// The virtualised message list. Rust holds the sorted IDs of the view; this
// component asks for the rows on screen a page at a time and draws only those.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { isCommandError, run } from "../api";
import { countLabel, followUpView, leavingRows, listNav, multiPicked, notify, picked, pickedMessages, processMessage, processedFolders, removeFromList, setDragged, setPicked, searchQuery, selectedFolder, selectedMessage, selectedRow, setSelectedMessage, setSelectedRow } from "../state";
import { OnlineHits, SearchBar } from "./SearchBar";
import { type Row, type Sort, listIndexOf, listOpen, listRows, onStoreChanged } from "./api";
import { shortDate, snoozeLabel } from "./format";
import { MessageMenu } from "./MessageMenu";

export const ROW_H = 64;
const PAGE = 100;
const OVERSCAN = 8;
/** The most messages Shift or Ctrl+A picks at once. */
const MAX_PICK = 1000;

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
  // After a row has faded out (see leavingRows), the rows from this index down
  // slide up into its place.
  const [shiftFrom, setShiftFrom] = createSignal<number | null>(null);
  let shiftTimer: ReturnType<typeof setTimeout> | undefined;
  // The message whose context menu is open, and where; `multi` when it is
  // open for the multiple selection.
  const [menu, setMenu] = createSignal<{ row: Row; x: number; y: number; multi?: { ids: number[]; accounts: string[] } } | null>(null);
  // Multiple selection: where Shift ranges start (an index), the message the
  // keyboard cursor is on while nothing is open, and, after the open message
  // or the selection left the list, where to open the next one.
  let anchor: number | null = null;
  let cursorId: number | null = null;
  let landing: { ids: ReadonlySet<number>; at: number } | null = null;
  const isPicked = (i: number) => {
    const r = rowAt(i);
    return r !== undefined && picked().has(r.id);
  };
  /** The message the list is on: the open one, or the cursor's in a multiple selection. */
  const focusId = () => selectedMessage() ?? (multiPicked() ? cursorId : null);
  // On the listbox, where focus is: a right click opens the menu for the row
  // under the pointer (selecting it), the menu key or Shift+F10 for the
  // selected row.
  const openMenu = (e: MouseEvent) => {
    e.preventDefault();
    const keyboard = e.clientX === 0 && e.clientY === 0;
    const under = (e.target as HTMLElement).closest(".list-row")?.id.match(/^msg-(\d+)$/);
    // A right click inside the multiple selection keeps it.
    if (!keyboard && under && !(multiPicked() && isPicked(Number(under[1])))) select(Number(under[1]));
    const i = keyboard ? selIndex() : under ? Number(under[1]) : null;
    const row = i === null ? undefined : rowAt(i);
    if (i === null || !row) return;
    const r = document.getElementById(`msg-${i}`)?.getBoundingClientRect();
    setMenu({
      row: { ...row },
      x: keyboard && r ? r.left + 24 : e.clientX,
      y: keyboard && r ? r.bottom : e.clientY,
      multi: multiPicked() ? { ids: pickedMessages().ids, accounts: [...new Set(picked().values())] } : undefined,
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
    if (!keepScroll) {
      // Another folder or search: the multiple selection belonged to the old one.
      setPicked(new Map());
      anchor = null;
      landing = null;
    }
    try {
      const opened = await listOpen({
        folders: selectedFolder() === null ? [] : [selectedFolder()!],
        unread_only: unreadOnly(),
        sort: sort(),
        query: searchQuery(),
        followups: followUpView(),
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
      const shift = keepScroll ? closedGap(pages, fresh) : null;
      pages = fresh;
      inflight = new Map();
      if (shift !== null) {
        // Set with the new rows, so no frame shows them in place first.
        clearTimeout(shiftTimer);
        setShiftFrom(shift);
        setRev((x) => x + 1);
        shiftTimer = setTimeout(() => setShiftFrom(null), 200);
      }
      setTotal(n);
      if (!keepScroll) {
        scroller.scrollTop = 0;
        setScrollTop(0);
      }
      const sel = focusId();
      const ix = sel === null ? null : await listIndexOf(sel);
      if (g !== gen) return;
      if (landing && ix === null) {
        // The open message (or the multiple selection) was deleted or moved:
        // open the one that took its place (the next one down), or the new
        // last one. Not yet if the refresh still has a leaving row there.
        const to = Math.min(landing.at, n - 1);
        const there = rowAt(to);
        if (!there || !landing.ids.has(there.id)) {
          landing = null;
          setRev((x) => x + 1);
          prefetch();
          if (n === 0) setSelectedMessage(null);
          else select(to);
          return;
        }
      }
      // A key may have moved the selection while this looked up the old one;
      // then the selection already has its new place.
      if (focusId() === sel) {
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

  /** The index of a leaving row that the refresh has taken out, when every
   * row after it on screen moved up by one; null otherwise (no animation). */
  function closedGap(before: Map<number, Row[]>, after: Map<number, Row[]>): number | null {
    const leaving = leavingRows();
    if (leaving.size === 0) return null;
    const at = (m: Map<number, Row[]>, i: number) => m.get(Math.floor(i / PAGE))?.[i % PAGE];
    for (const i of visible()) {
      const was = at(before, i);
      if (!was || !leaving.has(was.id)) continue;
      const next = at(before, i + 1);
      return next === undefined || at(after, i)?.id === next.id ? i : null;
    }
    return null;
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

  createEffect(on([selectedFolder, sort, unreadOnly, searchQuery, followUpView], () => void reopen(false)));

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

  // A multiple selection cleared (Esc, Clear selection) leaves nothing open,
  // so no row shows as selected either.
  createEffect(
    on(multiPicked, (multi, was) => {
      if (was && !multi && selectedMessage() === null && landing === null) setSelIndex(null);
    }),
  );

  // `fam ui open` and other code can select a message by ID.
  createEffect(
    on(selectedMessage, async (id) => {
      // A multiple selection keeps the keyboard cursor with nothing open.
      if (id === null) return multiPicked() ? undefined : setSelIndex(null);
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
    listNav.leaving = (ids) => {
      const gone = new Set(ids);
      if (picked().size > 0) {
        const left = new Map([...picked()].filter(([id]) => !gone.has(id)));
        if (left.size === picked().size) return;
        if (left.size >= 2) {
          setPicked(left);
          return;
        }
        // The selection is leaving: open what takes the place of its first row.
        setPicked(new Map());
        setSelectedMessage(null);
        landing = { ids: gone, at: firstIndexOf(gone) ?? selIndex() ?? 0 };
        return;
      }
      const at = selIndex();
      const open = selectedMessage();
      if (open !== null && gone.has(open) && at !== null) landing = { ids: gone, at };
    };
    listNav.stayed = () => {
      landing = null;
    };
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

  /** The lowest index of a loaded row among `ids`. */
  function firstIndexOf(ids: ReadonlySet<number>): number | null {
    let best: number | null = null;
    for (const [p, rows] of pages) {
      rows.forEach((r, k) => {
        if (ids.has(r.id) && (best === null || p * PAGE + k < best)) best = p * PAGE + k;
      });
    }
    return best;
  }

  /** Opens row `i` on its own, ending any multiple selection. */
  function select(i: number) {
    if (total() === 0) return;
    const ix = Math.max(0, Math.min(total() - 1, i));
    landing = null;
    anchor = ix;
    if (picked().size > 0) setPicked(new Map());
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

  /** Sets the multiple selection to `next`, with the cursor on row `cursor`.
   * One message left opens it; none closes the reading pane. */
  function applyPicked(next: Map<number, string>, cursor: number) {
    if (next.size >= 2) {
      setPicked(next);
      setSelectedMessage(null);
      setSelIndex(cursor);
      cursorId = rowAt(cursor)?.id ?? null;
      scrollIntoView(cursor);
      return;
    }
    const only = [...next.keys()][0];
    const keep = anchor;
    if (only !== undefined && rowAt(cursor)?.id === only) select(cursor);
    else {
      setPicked(new Map());
      setSelectedMessage(only ?? null);
      if (only === undefined) setSelIndex(cursor);
    }
    anchor = keep;
  }

  /** The multiple selection to start from: the picked messages, or the open one. */
  function pickedOrOpen(): Map<number, string> {
    if (picked().size > 0) return new Map(picked());
    const open = selectedRow();
    return open && open.id === selectedMessage() ? new Map([[open.id, open.account]]) : new Map();
  }

  /** Ctrl+click: adds row `i` to the selection, or takes it out. */
  function toggle(i: number) {
    const row = rowAt(i);
    if (!row) return;
    const next = pickedOrOpen();
    if (next.has(row.id)) next.delete(row.id);
    else next.set(row.id, row.account);
    anchor = i;
    applyPicked(next, i);
  }

  /** Shift+click and Shift+arrows: picks the rows from the anchor to `i`
   * (added to the selection with Ctrl). */
  async function pickTo(i: number, add: boolean) {
    if (total() === 0) return;
    const ix = Math.max(0, Math.min(total() - 1, i));
    const from = anchor ?? selIndex() ?? ix;
    await pickRange(from, ix, ix, add);
    anchor = from;
  }

  /** Picks rows `a` to `b` (either order), at most MAX_PICK of them, loading
   * their pages first, with the cursor on `cursor`. */
  async function pickRange(a: number, b: number, cursor: number, add: boolean) {
    let lo = Math.min(a, b);
    let hi = Math.max(a, b);
    if (hi - lo + 1 > MAX_PICK) {
      if (cursor === lo) hi = lo + MAX_PICK - 1;
      else lo = hi - MAX_PICK + 1;
      notify(`Picked ${countLabel(MAX_PICK)}, the most at once`);
    }
    const g = gen;
    const need: number[] = [];
    for (let p = Math.floor(lo / PAGE); p <= Math.floor(hi / PAGE); p++) need.push(p);
    await Promise.all(need.map(loadPage));
    if (g !== gen) return;
    const next = add ? pickedOrOpen() : new Map<number, string>();
    for (let k = lo; k <= hi; k++) {
      const r = rowAt(k);
      if (r) next.set(r.id, r.account);
    }
    applyPicked(next, cursor);
  }

  /** A press on row `i`: opens it, or with Ctrl or Shift changes the selection. */
  function press(i: number, e: MouseEvent) {
    // The context menu handles right clicks; a press on a picked row may be
    // the start of dragging them all, so a plain click opens it instead.
    if (e.button !== 0) return;
    const add = e.ctrlKey || e.metaKey;
    if (e.shiftKey) void pickTo(i, add);
    else if (add) toggle(i);
    else if (!(multiPicked() && isPicked(i))) select(i);
  }

  function onKeyDown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "a") {
      e.preventDefault();
      if (total() > 0) void pickRange(0, total() - 1, Math.min(selIndex() ?? 0, MAX_PICK - 1), false).then(() => {
        anchor = 0;
      });
      return;
    }
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
    if (e.shiftKey) void pickTo(to, false);
    else select(to);
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
            {(i) => (
              <ListRow
                index={i}
                row={rowAt(i)}
                selected={multiPicked() ? isPicked(i) : selIndex() === i}
                cursor={multiPicked() && selIndex() === i}
                multi={multiPicked()}
                shifting={(shiftFrom() ?? Infinity) <= i}
                onPress={press}
                onClick={(i, e) => {
                  // A plain click inside the multiple selection (not a drag) opens that row.
                  if (e.button === 0 && !e.shiftKey && !e.ctrlKey && !e.metaKey && multiPicked() && isPicked(i)) select(i);
                }}
                onToggle={toggle}
                unified={selectedFolder() === null || searchQuery() !== null || followUpView()}
              />
            )}
          </For>
        </div>
      </div>
      <Show when={menu()}>{(m) => <MessageMenu row={m().row} multi={m().multi} x={m().x} y={m().y} onClose={() => {
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
  /** The keyboard cursor, in a multiple selection. */
  cursor: boolean;
  /** A multiple selection is on: every row shows its tick box. */
  multi: boolean;
  shifting: boolean;
  unified: boolean;
  onPress: (i: number, e: MouseEvent) => void;
  onClick: (i: number, e: MouseEvent) => void;
  onToggle: (i: number) => void;
}) {
  return (
    <div
      id={`msg-${props.index}`}
      class="list-row"
      classList={{
        selected: props.selected,
        cursor: props.cursor,
        multi: props.multi,
        unread: props.row ? !props.row.is_read : false,
        leaving: props.row ? leavingRows().has(props.row.id) : false,
        "shift-up": props.shifting,
      }}
      role="option"
      aria-selected={props.selected}
      style={{ transform: `translateY(${props.index * ROW_H}px)` }}
      onMouseDown={(e) => props.onPress(props.index, e)}
      onClick={(e) => props.onClick(props.index, e)}
      draggable={props.row !== undefined}
      onDragStart={(e) => {
        const r = props.row;
        if (!r || !e.dataTransfer) return;
        e.dataTransfer.effectAllowed = "move";
        // Dragging a picked row takes the whole multiple selection.
        const all = props.multi && props.selected ? pickedMessages() : { ids: [r.id], account: r.account };
        e.dataTransfer.setData("text/plain", all.ids.length > 1 ? countLabel(all.ids.length) : r.subject);
        setDragged(all);
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
              <Show when={r().snoozed_until}>
                {(t) => (
                  <span class="row-snooze" title={`Snoozed until ${snoozeLabel(t())}`}>
                    💤 {snoozeLabel(t())}
                  </span>
                )}
              </Show>
              <Show when={r().follow_up_due}>
                {(due) => (
                  <span
                    class="row-followup"
                    classList={{ due: due() * 1000 <= Date.now() }}
                    title={due() * 1000 <= Date.now() ? "No reply yet: time to follow up" : `Remind me ${shortDate(due())} if nobody replies`}
                  >
                    {due() * 1000 <= Date.now() ? "No reply" : `⏰ ${shortDate(due())}`}
                  </span>
                )}
              </Show>
              <span class="row-date">{shortDate(r().received_at)}</span>
            </div>
            <div class="row-line">
              <Show when={r().spam_warning}>
                {(w) => (
                  <span class="row-spam" title={`Spam check: ${w()}`} aria-label={`Warning: ${w()}`}>
                    ⚠ {w().split(" · ")[0]}
                  </span>
                )}
              </Show>
              <Show when={r().importance === 2}>
                <span class="row-icon important" aria-label="High importance">!</span>
              </Show>
              <span class="row-subject">{r().subject || "(no subject)"}</span>
              <Show when={props.unified}>
                <span class="row-account">{r().account}</span>
              </Show>
            </div>
            <div class="row-preview">{r().preview}</div>
            {/* Tick box for picking several with the mouse (Ctrl+click does
                the same); shown on hover and on every row while several are
                picked. */}
            <span
              class="row-check"
              classList={{ checked: props.multi && props.selected }}
              title={props.multi && props.selected ? "Unpick" : "Pick (Ctrl+click)"}
              aria-hidden="true"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                props.onToggle(props.index);
              }}
            />
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
