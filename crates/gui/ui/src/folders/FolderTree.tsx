// The folder pane: the unified Inbox, then each account's folder tree.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { type CommandError, isCommandError, run } from "../api";
import type { FolderRow } from "../../../../../bindings/commands";
import { onStoreChanged } from "../list/api";
import { FolderMenu } from "./FolderMenu";
import { dragged, followUpView, removeFromList, searchQuery, selectedFolder, setDragged, setFollowUpView, setSearchQuery, setSelectedFolder, setSelectedMessage } from "../state";

/** Well-known folders first, in this order; the rest keep the tree order. */
const WELL_KNOWN = ["inbox", "drafts", "sentitems", "archive", "deleteditems", "junkemail"];

/** Each account's folders with the well-known top-level ones first. */
function ordered(rows: FolderRow[]): Map<string, FolderRow[]> {
  const byAccount = new Map<string, FolderRow[]>();
  for (const r of rows) {
    const list = byAccount.get(r.account) ?? [];
    list.push(r);
    byAccount.set(r.account, list);
  }
  for (const [account, list] of byAccount) {
    // Split into top-level subtrees, then put the well-known ones first.
    const trees: FolderRow[][] = [];
    for (const r of list) {
      if (r.depth === 0 || trees.length === 0) trees.push([r]);
      else trees[trees.length - 1]!.push(r);
    }
    const rank = (t: FolderRow[]) => {
      const i = WELL_KNOWN.indexOf(t[0]?.well_known ?? "");
      return i < 0 ? WELL_KNOWN.length : i;
    };
    trees.sort((a, b) => rank(a) - rank(b));
    byAccount.set(account, trees.flat());
  }
  return byAccount;
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

/** Moves the message dragged from the list into `f`. */
function dropOn(f: FolderRow) {
  const d = dragged();
  setDragged(null);
  if (!d) return;
  void removeFromList(d.id, `Moved to ${f.name}`, () => run("mail.move", { messages: [d.id], to: f.path }));
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
  const pinned = createMemo(() => folders.filter((f) => f.pinned));
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
}) {
  const [over, setOver] = createSignal(false);
  return (
    <button
      class="folder-item"
      classList={{ selected: props.selected, unread: props.unread > 0, "drop-target": over() }}
      role="treeitem"
      aria-selected={props.selected}
      aria-expanded={props.expanded}
      style={{ "padding-left": `calc(var(--space-2) + ${props.depth} * var(--space-5))` }}
      onClick={props.onSelect}
      onContextMenu={props.onContextMenu}
      title={props.title}
      onDragOver={(e) => {
        if (!props.canDrop?.()) return;
        e.preventDefault();
        if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        props.onDrop?.();
      }}
      onKeyDown={(e) => {
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
