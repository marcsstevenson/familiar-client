// Move to folder (Ctrl+Shift+V, the message menu, the multiple selection's
// pane): a filtered list of folders, for one message or many. Messages from
// more than one account can only go to a folder path every one of them has
// (Inbox, Archive), since `fam mail move` looks the path up in each message's
// own account.
import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import type { FolderRow } from "../../../../../bindings/commands";
import { run } from "../api";
import { countLabel, removeFromList } from "../state";

export function MovePicker(props: { ids: number[]; accounts: string[]; onClose: () => void }) {
  // A copy: the picker closes (and its props go stale) before the move runs.
  const ids = [...props.ids];
  const accounts = [...props.accounts];
  const [folders] = createResource(() => run("folder.list", {}).then((o) => o.folders));
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);

  /** The folders the messages can go to, one row per path. */
  const targets = createMemo<FolderRow[]>(() => {
    const all = folders() ?? [];
    if (accounts.length === 1) return all.filter((f) => f.account === accounts[0]);
    const first = all.filter((f) => f.account === accounts[0]);
    return first.filter((f) => accounts.every((a) => all.some((g) => g.account === a && g.path === f.path)));
  });
  const shown = createMemo(() => {
    const words = query().toLowerCase().split(/\s+/).filter(Boolean);
    return targets().filter((f) => words.every((w) => f.path.toLowerCase().includes(w)));
  });

  function choose(f: FolderRow | undefined) {
    if (!f) return;
    props.onClose();
    const what = ids.length > 1 ? `${countLabel(ids.length)} to ${f.name}` : `to ${f.name}`;
    void removeFromList(ids, `Moved ${what}`, () => run("mail.move", { messages: ids, to: f.path }));
  }

  onMount(() => {
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      props.onClose();
    };
    window.addEventListener("keydown", esc);
    onCleanup(() => window.removeEventListener("keydown", esc));
  });

  const onKeyDown = (e: KeyboardEvent) => {
    const n = shown().length;
    if (e.key === "ArrowDown" && n > 0) {
      e.preventDefault();
      setActive((active() + 1) % n);
    } else if (e.key === "ArrowUp" && n > 0) {
      e.preventDefault();
      setActive((active() - 1 + n) % n);
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(shown()[active()]);
    }
    document.querySelector("#move-list [aria-selected=true]")?.scrollIntoView({ block: "nearest" });
  };

  return (
    <div class="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="palette" role="dialog" aria-label={`Move ${countLabel(ids.length)}`} onKeyDown={onKeyDown}>
        <input
          ref={(el) => queueMicrotask(() => el.focus())}
          class="palette-input"
          placeholder={`Move ${countLabel(ids.length)} to…`}
          aria-label="Folder"
          aria-controls="move-list"
          value={query()}
          onInput={(e) => {
            setQuery(e.currentTarget.value);
            setActive(0);
          }}
        />
        <ul class="palette-list" id="move-list" role="listbox">
          <For each={shown()}>
            {(f, i) => (
              <li
                role="option"
                aria-selected={i() === active()}
                classList={{ active: i() === active() }}
                onMouseMove={() => setActive(i())}
                onClick={() => choose(f)}
              >
                <span class="palette-title" style={{ "padding-left": query() ? 0 : `${f.depth * 16}px` }}>
                  {query() ? f.path : f.name}
                </span>
              </li>
            )}
          </For>
          <Show when={folders.loading}>
            <li class="palette-note">Loading folders…</li>
          </Show>
          <Show when={!folders.loading && shown().length === 0}>
            <li class="palette-note">
              {accounts.length > 1 && targets().length === 0
                ? "These messages are in different accounts with no folder in common"
                : "No folder matches"}
            </li>
          </Show>
        </ul>
        <Show when={accounts.length > 1 && targets().length > 0}>
          <p class="palette-note move-note">From {accounts.length} accounts: only folders all of them have</p>
        </Show>
      </div>
    </div>
  );
}
