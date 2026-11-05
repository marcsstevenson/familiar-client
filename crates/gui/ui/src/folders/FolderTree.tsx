// The folder pane: the unified Inbox, then each account's folder tree.
import { For, Show, createMemo, createResource, onCleanup, onMount } from "solid-js";
import { type CommandError, isCommandError, run } from "../api";
import type { FolderRow } from "../../../../../bindings/commands";
import { onStoreChanged } from "../list/api";
import { searchQuery, selectedFolder, setSearchQuery, setSelectedFolder, setSelectedMessage } from "../state";

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

function choose(id: number | null) {
  setSearchQuery(null);
  setSelectedFolder(id);
  setSelectedMessage(null);
}

function chooseView(query: string) {
  setSearchQuery(query);
  setSelectedMessage(null);
}

export function FolderTree(props: { views: { name: string; query: string }[] }) {
  const [data, { refetch }] = createResource(() => run("folder.list", {}));
  onMount(async () => {
    const stop = await onStoreChanged(() => void refetch());
    onCleanup(stop);
  });
  const accounts = createMemo(() => ordered(data.latest?.folders ?? []));
  const inboxUnread = createMemo(() =>
    (data.latest?.folders ?? [])
      .filter((f) => f.well_known === "inbox")
      .reduce((n, f) => n + f.unread, 0),
  );
  const error = () => (isCommandError(data.error) ? (data.error as CommandError).message : null);

  return (
    <div class="folder-tree" role="tree" aria-label="Folders">
      <Show when={error()}>{(e) => <p class="pane-note">{e()}</p>}</Show>
      <FolderItem
        name="All inboxes"
        unread={inboxUnread()}
        depth={0}
        selected={selectedFolder() === null && searchQuery() === null}
        onSelect={() => choose(null)}
      />
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
      <For each={[...accounts()]}>
        {([account, rows]) => (
          <div role="group" aria-label={account}>
            <div class="folder-account">{account}</div>
            <For each={rows}>
              {(f) => (
                <FolderItem
                  name={f.name}
                  unread={f.unread}
                  depth={f.depth}
                  selected={selectedFolder() === f.id && searchQuery() === null}
                  onSelect={() => choose(f.id)}
                />
              )}
            </For>
          </div>
        )}
      </For>
    </div>
  );
}

function FolderItem(props: {
  name: string;
  unread: number;
  depth: number;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      class="folder-item"
      classList={{ selected: props.selected, unread: props.unread > 0 }}
      role="treeitem"
      aria-selected={props.selected}
      aria-label={props.unread > 0 ? `${props.name}, ${props.unread} unread` : props.name}
      style={{ "padding-left": `calc(var(--space-4) + ${props.depth} * var(--space-5))` }}
      onClick={props.onSelect}
    >
      <span class="folder-name">{props.name}</span>
      <Show when={props.unread > 0}>
        <span class="folder-count" aria-hidden="true">
          {props.unread}
        </span>
      </Show>
    </button>
  );
}
