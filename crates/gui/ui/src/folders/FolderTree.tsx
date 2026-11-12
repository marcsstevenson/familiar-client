// The folder pane: the unified Inbox, then each account's folder tree.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { type CommandError, isCommandError, run, setUnreadBadge } from "../api";
import type { FolderRow } from "../../../../../bindings/commands";
import { onStoreChanged } from "../list/api";
import { FolderMenu } from "./FolderMenu";
import { countLabel, dragged, followUpView, notify, removeFromList, searchQuery, selectedFolder, setDragged, setFollowUpView, setSearchQuery, setSelectedFolder, setSelectedMessage } from "../state";

/** Each account's folders, in tree order. `folder.list` already sorts
 * siblings: the order set by dragging, then well-known folders, then name. */
function ordered(rows: FolderRow[]): Map<string, FolderRow[]> {
  const byAccount = new Map<string, FolderRow[]>();
  for (const r of rows) {
    const list = byAccount.get(r.account) ?? [];
    list.push(r);
    byAccount.set(r.account, list);
  }
  return byAccount;
}

/** The folder being dragged to a new place, and the group (siblings, or the
 * pinned folders) it can move within. */
const [movingFolder, setMovingFolder] = createSignal<{ id: number; group: string } | null>(null);

/** Moves folder `id` before or after `target` within `group` (in its shown
 * order) and saves the group's new order. */
async function reorder(group: FolderRow[], id: number, target: number, before: boolean, pinned: boolean, done: () => void) {
  const ids = group.map((f) => f.id).filter((x) => x !== id);
  const at = ids.indexOf(target);
  if (at < 0 || id === target) return;
  ids.splice(before ? at : at + 1, 0, id);
  if (ids.every((x, i) => x === group[i]?.id)) return;
  try {
    await run("folder.reorder", { folders: ids, pinned });
  } catch (e) {
    notify(isCommandError(e) ? e.message : "Couldn't move the folder", true);
  }
  done();
}

/** Moves folder `f` one place up or down within `group` (Alt+Up/Down). */
function step(group: FolderRow[], f: FolderRow, dir: -1 | 1, pinned: boolean, done: () => void) {
  const i = group.findIndex((x) => x.id === f.id);
  const other = group[i + dir];
  if (i < 0 || !other) return;
  void reorder(group, f.id, other.id, dir < 0, pinned, done);
}

// Collapsed accounts (`a:<key>`) and folders (`f:<id>`), kept between runs.
const COLLAPSED_KEY = "familiar.folders.collapsed";
function loadCollapsed(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}
const [collapsed, setCollapsed] = createSignal(loadCollapsed());
function setOpen(key: string, open: boolean) {
  const next = new Set(collapsed());
  if (open) next.delete(key);
  else next.add(key);
  setCollapsed(next);
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]));
  } catch {
    // Not kept; the tree still works.
  }
}

/** The rows to show: those not under a collapsed folder. Returns the rows
 * themselves, so `<For>` keeps each button (and its focus) across a toggle. */
function shown(rows: FolderRow[]): FolderRow[] {
  let hideBelow: number | null = null;
  return rows.filter((f) => {
    if (hideBelow !== null && f.depth > hideBelow) return false;
    hideBelow = collapsed().has(`f:${f.id}`) ? f.depth : null;
    return true;
  });
}

/** The ids of folders with subfolders. */
function parents(rows: FolderRow[]): Set<number> {
  return new Set(rows.filter((f, i) => (rows[i + 1]?.depth ?? -1) > f.depth).map((f) => f.id));
}

/** Moves the messages dragged from the list into `f`. */
function dropOn(f: FolderRow) {
  const d = dragged();
  setDragged(null);
  if (!d) return;
  const what = d.ids.length > 1 ? `${countLabel(d.ids.length)} to ${f.name}` : `to ${f.name}`;
  void removeFromList(d.ids, `Moved ${what}`, () => run("mail.move", { messages: d.ids, to: f.path }));
}

function choose(id: number | null) {
  setSearchQuery(null);
  setFollowUpView(false);
  setSelectedFolder(id);
  setSelectedMessage(null);
}

function chooseView(query: string) {
  setFollowUpView(false);
  setSearchQuery(query);
  setSelectedMessage(null);
}

export function FolderTree(props: { views: { name: string; query: string }[] }) {
  const [data, { refetch }] = createResource(() => run("folder.list", {}));
  // Follow-up reminders: the Follow up entry shows while there are any, with
  // the number due. Rechecked on every store change, since a reply that
  // arrives answers one.
  const [followups, { refetch: refetchFollowups }] = createResource(() =>
    run("followup.list", {}).catch(() => ({ followups: [] })),
  );
  onMount(async () => {
    const stop = await onStoreChanged(() => {
      void refetch();
      void refetchFollowups();
    });
    onCleanup(stop);
  });
  const dueCount = () => (followups.latest?.followups ?? []).filter((f) => f.status === "due").length;
  // Folders keep their identity across refreshes (every sync and every move
  // changes counts), so <For> keeps their buttons: focus and a drag in progress
  // survive.
  const [folders, setFolders] = createStore<FolderRow[]>([]);
  createEffect(() => {
    const f = data.latest?.folders;
    if (f) setFolders(reconcile(f, { key: "id" }));
  });
  const accounts = createMemo(() => ordered(folders));
  const inboxUnread = createMemo(() =>
    folders
      .filter((f) => f.well_known === "inbox")
      .reduce((n, f) => n + f.unread, 0),
  );
  // A mail overlay on the taskbar button while any inbox has unread mail.
  // Only sent when it flips, not on every count change.
  const anyUnread = createMemo(() => inboxUnread() > 0);
  createEffect(() => {
    void setUnreadBadge(anyUnread() ? 1 : 0).catch(() => {});
  });
  // Pinned folders in the order set by dragging; the rest after, in tree order.
  const pinned = createMemo(() =>
    folders.filter((f) => f.pinned).sort((a, b) => (a.pin_order ?? Infinity) - (b.pin_order ?? Infinity)),
  );
  const saved = () => void refetch();
  // A pinned folder's name gets its account only when another pinned folder
  // has the same name; the full path and account show on hover.
  const sharedNames = createMemo(() => {
    const seen = new Map<string, number>();
    for (const f of pinned()) seen.set(f.name.toLowerCase(), (seen.get(f.name.toLowerCase()) ?? 0) + 1);
    return new Set([...seen].filter(([, n]) => n > 1).map(([k]) => k));
  });
  // The folder whose context menu is open, and where.
  const [menu, setMenu] = createSignal<{ folder: FolderRow; x: number; y: number } | null>(null);
  const openMenu = (f: FolderRow) => (e: MouseEvent) => {
    e.preventDefault();
    // From the keyboard (menu key, Shift+F10) there is no pointer: open by the row.
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const keyboard = e.clientX === 0 && e.clientY === 0;
    setMenu({ folder: { ...f }, x: keyboard ? r.left + 24 : e.clientX, y: keyboard ? r.bottom : e.clientY });
  };
  const error = () => (isCommandError(data.error) ? (data.error as CommandError).message : null);

  return (
    <div class="folder-tree" role="tree" aria-label="Folders">
      <Show when={error()}>{(e) => <p class="pane-note">{e()}</p>}</Show>
      <FolderItem
        name="All inboxes"
        unread={inboxUnread()}
        depth={0}
        selected={selectedFolder() === null && searchQuery() === null && !followUpView()}
        onSelect={() => choose(null)}
      />
      <Show when={(followups.latest?.followups.length ?? 0) > 0 || followUpView()}>
        <FolderItem
          name="Follow up"
          title="Mail you asked to be reminded about if nobody replies"
          unread={dueCount()}
          countLabel="due"
          depth={0}
          selected={followUpView() && searchQuery() === null}
          onSelect={() => {
            choose(null);
            setFollowUpView(true);
          }}
        />
      </Show>
      <Show when={pinned().length > 0}>
        <div role="group" aria-label="Pinned folders">
          <For each={pinned()}>
            {(f) => (
              <FolderItem
                name={sharedNames().has(f.name.toLowerCase()) ? `${f.name} · ${f.account}` : f.name}
                title={`${f.path} (${f.account})`}
                unread={f.unread}
                depth={0}
                color={f.color}
                selected={selectedFolder() === f.id && searchQuery() === null}
                onSelect={() => choose(f.id)}
                onContextMenu={openMenu(f)}
                canDrop={() => dragged()?.account === f.account}
                onDrop={() => dropOn(f)}
                move={{
                  id: f.id,
                  group: "pinned",
                  onReorder: (id, before) => void reorder(pinned(), id, f.id, before, true, saved),
                  onStep: (dir) => step(pinned(), f, dir, true, saved),
                }}
              />
            )}
          </For>
        </div>
      </Show>
      <Show when={props.views.length > 0}>
        <div role="group" aria-label="Saved searches">
          <div class="folder-account">Views</div>
          <For each={props.views}>
            {(v) => (
              <FolderItem
                name={v.name}
                unread={0}
                depth={0}
                selected={searchQuery() === v.query}
                onSelect={() => chooseView(v.query)}
              />
            )}
          </For>
        </div>
      </Show>
      <For each={[...accounts().keys()]}>
        {(account) => {
          const rows = () => accounts().get(account) ?? [];
          const siblings = (f: FolderRow) => rows().filter((r) => r.parent === f.parent);
          return (
            <div role="group" aria-label={account}>
              <button
                class="folder-account"
                role="treeitem"
                aria-expanded={!collapsed().has(`a:${account}`)}
                onClick={() => setOpen(`a:${account}`, collapsed().has(`a:${account}`))}
                onKeyDown={(e) => {
                  if (e.key === "ArrowRight" || e.key === "ArrowLeft") setOpen(`a:${account}`, e.key === "ArrowRight");
                }}
              >
                <span class="folder-twisty" aria-hidden="true">{collapsed().has(`a:${account}`) ? "▸" : "▾"}</span>
                {account}
              </button>
              <Show when={!collapsed().has(`a:${account}`)}>
                <For each={shown(rows())}>
                  {(f) => (
                    <FolderItem
                      name={f.name}
                      unread={f.unread}
                      depth={f.depth}
                      expanded={parents(rows()).has(f.id) ? !collapsed().has(`f:${f.id}`) : undefined}
                      onToggle={(open) => setOpen(`f:${f.id}`, open)}
                      selected={selectedFolder() === f.id && searchQuery() === null}
                      onSelect={() => choose(f.id)}
                      color={f.color}
                      onContextMenu={openMenu(f)}
                      canDrop={() => dragged()?.account === f.account}
                      onDrop={() => dropOn(f)}
                      move={{
                        id: f.id,
                        group: `${f.account}:${f.parent ?? ""}`,
                        onReorder: (id, before) => void reorder(siblings(f), id, f.id, before, false, saved),
                        onStep: (dir) => step(siblings(f), f, dir, false, saved),
                      }}
                    />
                  )}
                </For>
              </Show>
            </div>
          );
        }}
      </For>
      <Show when={menu()}>
        {(m) => <FolderMenu folder={m().folder} x={m().x} y={m().y} onClose={() => setMenu(null)} onChanged={() => void refetch()} />}
      </Show>
    </div>
  );
}

function FolderItem(props: {
  name: string;
  unread: number;
  depth: number;
  /** Set for a folder with subfolders: whether they show. */
  expanded?: boolean;
  onToggle?: (open: boolean) => void;
  selected: boolean;
  onSelect: () => void;
  /** Whether the message being dragged can be moved here (same account). */
  canDrop?: () => boolean;
  onDrop?: () => void;
  /** Its colour (`fam folder color`), shown as a dot before the name. */
  color?: string | null;
  onContextMenu?: (e: MouseEvent) => void;
  /** Hover text. */
  title?: string;
  /** What the count is, for screen readers (default "unread"). */
  countLabel?: string;
  /** Set for a folder that can be dragged to a new place among `group`. */
  move?: {
    id: number;
    group: string;
    /** Folder `id` was dropped just before or after this one. */
    onReorder: (id: number, before: boolean) => void;
    /** Alt+Up or Alt+Down. */
    onStep: (dir: -1 | 1) => void;
  };
}) {
  const [over, setOver] = createSignal(false);
  // Where a folder being moved would land: just above or below this one.
  const [edge, setEdge] = createSignal<"before" | "after" | null>(null);
  const movable = () => {
    const m = movingFolder();
    return m !== null && props.move !== undefined && m.group === props.move.group && m.id !== props.move.id;
  };
  return (
    <button
      class="folder-item"
      classList={{
        selected: props.selected,
        unread: props.unread > 0,
        "drop-target": over(),
        "drop-before": edge() === "before",
        "drop-after": edge() === "after",
      }}
      role="treeitem"
      aria-selected={props.selected}
      aria-expanded={props.expanded}
      style={{ "padding-left": `calc(var(--space-2) + ${props.depth} * var(--space-5))` }}
      onClick={props.onSelect}
      onContextMenu={props.onContextMenu}
      title={props.title}
      draggable={props.move !== undefined}
      onDragStart={(e) => {
        if (!props.move || !e.dataTransfer) return;
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", props.name);
        setMovingFolder({ id: props.move.id, group: props.move.group });
      }}
      onDragEnd={() => setMovingFolder(null)}
      onDragOver={(e) => {
        if (movable()) {
          e.preventDefault();
          if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
          const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
          setEdge(e.clientY < r.top + r.height / 2 ? "before" : "after");
          return;
        }
        if (!props.canDrop?.()) return;
        e.preventDefault();
        if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
        setOver(true);
      }}
      onDragLeave={() => {
        setOver(false);
        setEdge(null);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        const where = edge();
        setEdge(null);
        const m = movingFolder();
        if (where && m && movable()) {
          setMovingFolder(null);
          props.move?.onReorder(m.id, where === "before");
          return;
        }
        props.onDrop?.();
      }}
      onKeyDown={(e) => {
        if (props.move && e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
          e.preventDefault();
          props.move.onStep(e.key === "ArrowUp" ? -1 : 1);
          return;
        }
        if (props.expanded === undefined) return;
        if (e.key === "ArrowRight" || e.key === "ArrowLeft") props.onToggle?.(e.key === "ArrowRight");
      }}
    >
      <span
        class="folder-twisty"
        aria-hidden="true"
        onClick={(e) => {
          if (props.expanded === undefined) return;
          e.stopPropagation();
          props.onToggle?.(!props.expanded);
        }}
      >
        {props.expanded === undefined ? "" : props.expanded ? "▾" : "▸"}
      </span>
      <Show when={props.color}>
        {(c) => <span class="folder-dot" aria-hidden="true" style={{ background: `var(--folder-${c()})` }} />}
      </Show>
      <span class="folder-name">{props.name}</span>
      <Show when={props.unread > 0}>
        <span class="folder-count">
          {props.unread}
          <span class="visually-hidden"> {props.countLabel ?? "unread"}</span>
        </span>
      </Show>
    </button>
  );
}
