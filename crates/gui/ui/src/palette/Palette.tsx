// The command palette (Ctrl+K): every app action and every registered command,
// each with its key and the `fam` command that does the same.
import { For, Match, Show, Switch, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import type { CommandDoc, CommandName, SchemaDocument } from "../../../../../bindings/commands";
import { COMMAND_SURFACE } from "../actions/surface";
import { type Action, allActions, keyOf } from "../actions/registry";
import { isCommandError, run } from "../api";
import { selectedMessage } from "../state";
import { type Field, type Values, cliLine, fieldsOf, inputOf } from "./schema";

type Item =
  | { type: "action"; action: Action; title: string; hint: string; key?: string }
  | { type: "command"; doc: CommandDoc; title: string; hint: string };

const [docs] = createResource(async () => {
  const out = (await run("schema", { format: "commands" })) as SchemaDocument;
  return out.commands.filter((c) => COMMAND_SURFACE[c.name as CommandName] === "palette");
});

/** Prefills a form from the selection: `messages`, `message` and `id`-like fields. */
function prefill(fields: Field[]): Values {
  const id = selectedMessage();
  const v: Values = {};
  if (id === null) return v;
  for (const f of fields) {
    if (f.name === "messages" || f.name === "message") v[f.name] = String(id);
  }
  return v;
}

function matches(query: string, text: string): boolean {
  // Every word of the query appears, in any order.
  const t = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => t.includes(w));
}

export function Palette(props: { initial?: string; onClose: () => void }) {
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const [form, setForm] = createSignal<CommandDoc | null>(null);

  const items = createMemo<Item[]>(() => {
    const acts: Item[] = allActions()
      .filter((a) => a.id !== "palette" && (!a.enabled || a.enabled()))
      .map((a) => ({ type: "action", action: a, title: a.title, hint: a.cli?.() ?? "", key: keyOf(a) }));
    const cmds: Item[] = (docs() ?? []).map((d) => ({
      type: "command",
      doc: d,
      title: d.summary,
      hint: d.cli,
    }));
    return [...acts, ...cmds].filter((i) => matches(query(), `${i.title} ${i.hint}`));
  });

  // Opened for one command (Reply before the compose window, for example):
  // show its form once the command list has loaded.
  let opened = false;
  createEffect(() => {
    const d = docs()?.find((c) => c.name === props.initial);
    if (d && !opened) {
      opened = true;
      setForm(d);
    }
  });
  // Escape works wherever focus is while the palette is open.
  onMount(() => {
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (form()) setForm(null);
      else props.onClose();
    };
    window.addEventListener("keydown", esc);
    onCleanup(() => window.removeEventListener("keydown", esc));
  });

  function choose(i: Item | undefined) {
    if (!i) return;
    if (i.type === "action") {
      props.onClose();
      void i.action.run();
    } else setForm(i.doc);
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      if (form()) setForm(null);
      else props.onClose();
    } else if (form()) {
      // Keys belong to the form's fields.
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(items().length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(items()[active()]);
    }
    e.stopPropagation();
  }

  return (
    <div class="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="palette" role="dialog" aria-label="Commands" onKeyDown={onKeyDown}>
        <Show
          when={form()}
          fallback={
            <>
              <input
                ref={(el) => queueMicrotask(() => el.focus())}
                class="palette-input"
                placeholder="Type a command"
                aria-label="Search commands"
                aria-controls="palette-list"
                value={query()}
                onInput={(e) => {
                  setQuery(e.currentTarget.value);
                  setActive(0);
                }}
              />
              <ul class="palette-list" id="palette-list" role="listbox">
                <For each={items()}>
                  {(item, i) => (
                    <li
                      role="option"
                      aria-selected={i() === active()}
                      classList={{ active: i() === active() }}
                      onMouseMove={() => setActive(i())}
                      onClick={() => choose(item)}
                    >
                      <span class="palette-title">{item.title}</span>
                      <span class="palette-cli">{item.hint}</span>
                      <Show when={item.type === "action" && item.key}>
                        <kbd>{(item as { key: string }).key}</kbd>
                      </Show>
                    </li>
                  )}
                </For>
                <Show when={docs.loading}>
                  <li class="palette-note">Loading commands…</li>
                </Show>
              </ul>
            </>
          }
        >
          {(doc) => (
            <CommandForm doc={doc()} onBack={() => setForm(null)} />
          )}
        </Show>
      </div>
    </div>
  );
}

function CommandForm(props: { doc: CommandDoc; onBack: () => void }) {
  const fields = fieldsOf(props.doc.input);
  const [values, setValues] = createSignal<Values>(prefill(fields));
  const [result, setResult] = createSignal<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  const set = (name: string, v: string | boolean) => setValues((o) => ({ ...o, [name]: v }));

  const parsed = createMemo(() => {
    try {
      return { input: inputOf(fields, values()), error: null };
    } catch (e) {
      return { input: null, error: String(e) };
    }
  });
  const cli = () => {
    const p = parsed();
    return p.input ? cliLine(props.doc.cli, fields, p.input) : props.doc.cli;
  };

  async function submit(e: Event) {
    e.preventDefault();
    const p = parsed();
    if (!p.input) return;
    setBusy(true);
    try {
      const out = await run(props.doc.name as CommandName, p.input as never);
      setResult({ ok: true, text: JSON.stringify(out, null, 2) });
    } catch (err) {
      setResult({ ok: false, text: isCommandError(err) ? `${err.kind}: ${err.message}` : String(err) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <form class="command-form" onSubmit={submit}>
      <div class="command-head">
        <button type="button" class="link" onClick={props.onBack} aria-label="Back to all commands">
          ‹
        </button>
        <div>
          <div class="palette-title">{props.doc.summary}</div>
          <div class="command-desc">{props.doc.description}</div>
        </div>
      </div>
      <div class="command-fields">
        <For each={fields}>
          {(f, i) => (
            <label class="command-field">
              <span class="command-name">
                {f.name.replace(/_/g, " ")}
                {f.required ? " *" : ""}
              </span>
              <Switch
                fallback={
                  <input
                    autofocus={i() === 0}
                    required={f.required}
                    inputmode={f.kind === "integer" || f.kind === "number" || f.numericItems ? "numeric" : undefined}
                    placeholder={f.kind === "list" ? "comma-separated" : f.kind === "json" ? "JSON" : ""}
                    value={(values()[f.name] as string | undefined) ?? ""}
                    onInput={(e) => set(f.name, e.currentTarget.value)}
                  />
                }
              >
                <Match when={f.name === "body"}>
                  <textarea
                    rows={6}
                    required={f.required}
                    value={(values()[f.name] as string | undefined) ?? ""}
                    onInput={(e) => set(f.name, e.currentTarget.value)}
                  />
                </Match>
                <Match when={f.kind === "boolean"}>
                  <input
                    type="checkbox"
                    checked={values()[f.name] === true}
                    onChange={(e) => set(f.name, e.currentTarget.checked)}
                  />
                </Match>
                <Match when={f.kind === "enum"}>
                  <select value={(values()[f.name] as string | undefined) ?? ""} onChange={(e) => set(f.name, e.currentTarget.value)}>
                    <option value="">{f.required ? "Choose…" : "(default)"}</option>
                    <For each={f.options}>{(o) => <option value={o}>{o}</option>}</For>
                  </select>
                </Match>
              </Switch>
              <span class="command-help">{f.description}</span>
            </label>
          )}
        </For>
      </div>
      <div class="command-cli" aria-label="Command line">
        <code>{cli()}</code>
      </div>
      <Show when={parsed().error}>{(e) => <p class="warn">{e()}</p>}</Show>
      <div class="command-actions">
        <button type="submit" class="primary" disabled={busy() || !parsed().input}>
          {busy() ? "Running…" : "Run"}
        </button>
      </div>
      <Show when={result()}>
        {(r) => <pre class="command-result" classList={{ warn: !r().ok }}>{r().text}</pre>}
      </Show>
    </form>
  );
}
