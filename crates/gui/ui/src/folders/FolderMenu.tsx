// The folder context menu: pin, colour, rename and delete. Colour and pin are
// kept on this device (`fam folder color` / `pin`); rename and delete go to the
// server through the op queue, as `fam folder rename` / `delete` do.
import { For, Show, createSignal, onMount } from "solid-js";
import { isCommandError, run } from "../api";
import type { FolderRow } from "../../../../../bindings/commands";
import { notify } from "../state";

/** The colours `fam folder color` takes; each has a token in styles/tokens.css. */
export const FOLDER_COLOURS = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "grey"];

type Mode = "menu" | "rename" | "delete";

export function FolderMenu(props: { folder: FolderRow; x: number; y: number; onClose: () => void; onChanged: () => void }) {
  // A copy: the menu closes (and its props go stale) before a command runs.
  const folder = { ...props.folder };
  const [mode, setMode] = createSignal<Mode>("menu");
  const [name, setName] = createSignal(folder.name);
  const builtIn = () => folder.well_known != null;
  const target = () => ({ folder: folder.path, account: folder.account });
  let box!: HTMLElement;

  onMount(() => {
    // Keep the menu inside the window.
    const r = box.getBoundingClientRect();
    if (mode() === "menu") {
      box.style.left = `${Math.max(4, Math.min(props.x, window.innerWidth - r.width - 4))}px`;
      box.style.top = `${Math.max(4, Math.min(props.y, window.innerHeight - r.height - 4))}px`;
    }
    box.querySelector<HTMLElement>("button, input")?.focus();
  });

  async function act(label: string, job: () => Promise<unknown>) {
    props.onClose();
    try {
      await job();
      notify(label);
      props.onChanged();
    } catch (e) {
      notify(isCommandError(e) ? e.message : String(e), true);
    }
  }

  const togglePin = () =>
    act(folder.pinned ? `Unpinned ${folder.name}` : `Pinned ${folder.name}`, () =>
      run(folder.pinned ? "folder.unpin" : "folder.pin", target()),
    );
  const colour = (c: string | null) =>
    act(c ? `${folder.name} is ${c}` : `${folder.name} has no colour`, () =>
      run("folder.color", { ...target(), color: c ?? "none" }),
    );
  const rename = () => {
    const n = name().trim();
    if (!n || n === folder.name) return props.onClose();
    void act(`Renamed ${folder.name} to ${n}`, () => run("folder.rename", { ...target(), name: n }));
  };
  const remove = () =>
    act(`Moved ${folder.name} to Deleted Items`, () => run("folder.delete", target()));

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      props.onClose();
    } else if (mode() === "menu" && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      const items = [...box.querySelectorAll<HTMLElement>("[role=menuitem]")];
      const i = items.indexOf(document.activeElement as HTMLElement);
      const next = (i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }
  };

  return (
    <div
      class="folder-menu-backdrop"
      role="dialog"
      aria-label={`Folder ${folder.name}`}
      classList={{ modal: mode() !== "menu" }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        if (e.target === e.currentTarget) props.onClose();
      }}
      onKeyDown={onKeyDown}
    >
      <Show when={mode() === "menu"}>
        <div class="folder-menu" role="menu" ref={(el) => (box = el)} style={{ left: `${props.x}px`, top: `${props.y}px` }}>
          <button role="menuitem" onClick={togglePin}>
            {folder.pinned ? "Unpin" : "Pin below All inboxes"}
          </button>
          <div class="folder-menu-colours" role="group" aria-label="Colour">
            <For each={FOLDER_COLOURS}>
              {(c) => (
                <button
                  role="menuitem"
                  class="folder-swatch"
                  classList={{ current: folder.color === c }}
                  style={{ background: `var(--folder-${c})` }}
                  title={c}
                  aria-label={`Colour ${c}`}
                  onClick={() => colour(c)}
                />
              )}
            </For>
            <button
              role="menuitem"
              class="folder-swatch none"
              classList={{ current: !folder.color }}
              title="No colour"
              aria-label="No colour"
              onClick={() => colour(null)}
            />
          </div>
          <Show when={!builtIn()}>
            <button role="menuitem" onClick={() => setMode("rename")}>Rename…</button>
            <button role="menuitem" class="danger" onClick={() => setMode("delete")}>Delete…</button>
          </Show>
        </div>
      </Show>
      <Show when={mode() === "rename"}>
        <form
          class="folder-dialog"
          ref={(el) => (box = el)}
          onSubmit={(e) => {
            e.preventDefault();
            rename();
          }}
        >
          <label>
            Rename {folder.account} / {folder.path} to
            <input value={name()} onInput={(e) => setName(e.currentTarget.value)} ref={(el) => setTimeout(() => {
              el.focus();
              el.select();
            })} />
          </label>
          <div class="folder-dialog-buttons">
            <button type="button" onClick={props.onClose}>Cancel</button>
            <button type="submit" class="primary">Rename</button>
          </div>
        </form>
      </Show>
      <Show when={mode() === "delete"}>
        <div class="folder-dialog" role="alertdialog" aria-label="Delete folder" ref={(el) => (box = el)}>
          <p>
            Delete <strong>{folder.name}</strong> from {folder.account}? It moves to Deleted Items with
            everything in it{folder.total > 0 ? ` (${folder.total.toLocaleString()} messages here)` : ""}.
          </p>
          <div class="folder-dialog-buttons">
            <button type="button" onClick={props.onClose} ref={(el) => setTimeout(() => el.focus())}>
              Cancel
            </button>
            <button type="button" class="danger-fill" onClick={remove}>Delete</button>
          </div>
        </div>
      </Show>
    </div>
  );
}
