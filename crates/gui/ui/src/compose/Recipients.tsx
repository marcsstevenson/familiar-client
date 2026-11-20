// A recipient line (To, Cc or Bcc): addresses as chips, with autocomplete
// from contacts and past correspondents (`contacts.search`).
import { For, Show, createSignal, onCleanup } from "solid-js";
import { type Commands, run } from "../api";

type Person = Commands["contacts.search"]["output"]["people"][number];

/** Splits typed or pasted text on commas and semicolons outside quotes and angle brackets. */
export function splitAddresses(text: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote = false;
  let angle = false;
  for (const c of text) {
    if (c === '"') quote = !quote;
    else if (c === "<") angle = true;
    else if (c === ">") angle = false;
    if ((c === "," || c === ";") && !quote && !angle) {
      if (cur.trim()) out.push(cur.trim());
      cur = "";
    } else cur += c;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

/** The address in `Name <addr>` or a bare address. */
export function addressOf(mailbox: string): string {
  const m = /<([^>]*)>\s*$/.exec(mailbox);
  return (m?.[1] ?? mailbox).trim();
}

export const looksValid = (mailbox: string) => /^[^\s@<>:]+@[^\s@<>:]+\.[^\s@<>:]+$/.test(addressOf(mailbox));

/** What a chip shows: the name when there is one. */
function chipText(mailbox: string): string {
  const m = /^\s*"?([^"<]*?)"?\s*<[^>]*>\s*$/.exec(mailbox);
  return m && m[1] ? m[1] : mailbox;
}

export type RecipientField = "to" | "cc" | "bcc";

export function Recipients(props: {
  label: "To" | "Cc" | "Bcc";
  onMove: (mailbox: string, destination: RecipientField) => void;
  value: string[];
  onChange: (v: string[]) => void;
  /** Search only this account's people. */
  account?: string;
  autofocus?: boolean;
}) {
  let input!: HTMLInputElement;
  const [text, setText] = createSignal("");
  const [people, setPeople] = createSignal<Person[]>([]);
  const [active, setActive] = createSignal(0);
  let timer: number | undefined;
  let asked = 0;
  onCleanup(() => clearTimeout(timer));

  const add = (items: string[]) => {
    const have = new Set(props.value.map((v) => addressOf(v).toLowerCase()));
    const fresh = items.filter((i) => !have.has(addressOf(i).toLowerCase()));
    if (fresh.length) props.onChange([...props.value, ...fresh]);
  };
  const commit = () => {
    const items = splitAddresses(text());
    if (items.length) add(items);
    setText("");
    setPeople([]);
  };
  const choose = (p: Person) => {
    add([p.mailbox]);
    setText("");
    setPeople([]);
    input.focus();
  };
  const search = (q: string) => {
    clearTimeout(timer);
    if (!q.trim()) {
      setPeople([]);
      return;
    }
    const n = ++asked;
    timer = window.setTimeout(async () => {
      try {
        const out = await run("contacts.search", { query: q.trim(), limit: 8, account: props.account ?? null });
        if (n === asked) {
          setPeople(out.people);
          setActive(0);
        }
      } catch {
        // No store yet, or famd's store is busy: typing still works.
        if (n === asked) setPeople([]);
      }
    }, 120);
  };

  // Pointer capture keeps chip moves separate from native attachment drops.
  let highlighted: HTMLElement | null = null;
  const highlight = (target: HTMLElement | null) => {
    highlighted?.classList.remove("recipient-drop-target");
    highlighted = target;
    highlighted?.classList.add("recipient-drop-target");
  };
  onCleanup(() => highlight(null));

  // While a chip is dragged, a faded copy of it follows the pointer and the
  // original dims. The copy appears only once the pointer has moved a few
  // pixels, so a plain click on a chip shows nothing.
  let drag: { chip: HTMLElement; x: number; y: number; dx: number; dy: number; ghost?: HTMLElement } | null = null;
  const endDrag = () => {
    if (!drag) return;
    drag.ghost?.remove();
    drag.chip.classList.remove("chip-dragging");
    document.body.classList.remove("chip-drag-active");
    drag = null;
  };
  onCleanup(endDrag);
  const moveGhost = (e: PointerEvent) => {
    if (!drag) return;
    if (!drag.ghost) {
      if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 4) return;
      const ghost = drag.chip.cloneNode(true) as HTMLElement;
      ghost.querySelector(".chip-remove")?.remove();
      ghost.className = "chip chip-ghost";
      ghost.removeAttribute("title");
      ghost.setAttribute("aria-hidden", "true");
      document.body.append(ghost);
      drag.ghost = ghost;
      drag.chip.classList.add("chip-dragging");
      document.body.classList.add("chip-drag-active");
    }
    drag.ghost.style.transform = `translate(${e.clientX - drag.dx}px, ${e.clientY - drag.dy}px)`;
  };
  const dropTarget = (e: PointerEvent) => {
    const target = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-recipient-field]");
    return target?.dataset.recipientField !== props.label.toLowerCase() ? target ?? null : null;
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const list = people();
    if (list.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      setActive((a) => (a + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
    } else if (list.length && (e.key === "Enter" || e.key === "Tab") && !e.shiftKey) {
      e.preventDefault();
      choose(list[active()] ?? list[0]!);
    } else if (e.key === "Enter" || e.key === "," || e.key === ";") {
      if (text().trim()) {
        e.preventDefault();
        commit();
      }
    } else if (e.key === "Escape" && list.length) {
      e.preventDefault();
      e.stopPropagation();
      setPeople([]);
    } else if (e.key === "Backspace" && text() === "" && props.value.length) {
      props.onChange(props.value.slice(0, -1));
    }
  };

  return (
    <div class="compose-field recipients" data-recipient-field={props.label.toLowerCase()} onClick={() => input.focus()}>
      <label class="compose-label" for={`rcpt-${props.label}`}>
        {props.label}
      </label>
      <div class="chips">
        <For each={props.value}>
          {(r, i) => (
            <span
              class="chip"
              classList={{ invalid: !looksValid(r) }}
              title={r}
              onPointerDown={(e) => {
                if (e.isPrimary && e.button === 0 && !(e.target as Element).closest("button")) {
                  e.currentTarget.setPointerCapture(e.pointerId);
                  const box = e.currentTarget.getBoundingClientRect();
                  endDrag();
                  drag = { chip: e.currentTarget, x: e.clientX, y: e.clientY, dx: e.clientX - box.left, dy: e.clientY - box.top };
                }
              }}
              onPointerMove={(e) => {
                if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
                moveGhost(e);
                highlight(dropTarget(e));
              }}
              onPointerUp={(e) => {
                if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
                const field = dropTarget(e)?.dataset.recipientField;
                highlight(null);
                endDrag();
                if (field === "to" || field === "cc" || field === "bcc") props.onMove(r, field);
              }}
              onLostPointerCapture={() => {
                highlight(null);
                endDrag();
              }}
            >
              {chipText(r)}
              <button
                type="button"
                class="chip-remove"
                aria-label={`Remove ${r}`}
                onClick={(e) => {
                  e.stopPropagation();
                  props.onChange(props.value.filter((_, j) => j !== i()));
                }}
              >
                x
              </button>
            </span>
          )}
        </For>
        <input
          ref={input}
          id={`rcpt-${props.label}`}
          class="chip-input"
          autocomplete="off"
          spellcheck={false}
          autofocus={props.autofocus}
          value={text()}
          role="combobox"
          aria-expanded={people().length > 0}
          aria-controls={`rcpt-${props.label}-list`}
          onInput={(e) => {
            const v = e.currentTarget.value;
            if (/[,;]\s*$/.test(v) || splitAddresses(v).length > 1) {
              const parts = splitAddresses(v);
              const last = /[,;]\s*$/.test(v) ? "" : parts.pop()!;
              add(parts);
              setText(last);
              search(last);
            } else {
              setText(v);
              search(v);
            }
          }}
          onKeyDown={onKeyDown}
          // Suggestions are picked on mousedown without taking focus, so a
          // blur is always the end of typing (Send included): commit now.
          onBlur={commit}
        />
      </div>
      <Show when={people().length > 0}>
        <ul class="suggestions" id={`rcpt-${props.label}-list`} role="listbox">
          <For each={people()}>
            {(p, i) => (
              <li
                role="option"
                aria-selected={i() === active()}
                classList={{ active: i() === active() }}
                onMouseDown={(e) => {
                  e.preventDefault();
                  choose(p);
                }}
                onMouseEnter={() => setActive(i())}
              >
                <span class="suggestion-name">{p.name || p.address}</span>
                <Show when={p.name}>
                  <span class="suggestion-addr">{p.address}</span>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}
