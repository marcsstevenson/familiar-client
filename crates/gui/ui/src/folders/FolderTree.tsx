// The folder pane: the unified Inbox, then each account's folder tree.
import { For, Show, createMemo, createResource, onCleanup, onMount } from "solid-js";
import { type CommandError, isCommandError, run } from "../api";
import type { FolderRow } from "../../../../../bindings/commands";
import { onStoreChanged } from "../list/api";
import { selectedFolder, setSelectedFolder, setSelectedMessage } from "../state";

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
  setSelectedFolder(id);
  setSelectedMessage(null);
}

export function FolderTree() {
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
        selected={selectedFolder() === null}
        onSelect={() => choose(null)}
      />
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
                  selected={selectedFolder() === f.id}
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
