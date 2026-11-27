import { For, Show, batch, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { keyName } from "../actions/registry";
import { isCommandError, run } from "../api";
import { openCompose } from "../compose/spec";
import { onStoreChanged } from "../list/api";
import { notify, setConfirmDialog } from "../state";
import { ContactEditor, type Contact } from "./ContactEditor";
import { openLink } from "../reading/api";
import { duplicateGroups, googleLink, includesMerge, key, mergeFields, snapshot, target, title } from "./book";
import { MergeDialog } from "./MergeDialog";
import "./contacts.css";

const errorText = (e: unknown) => isCommandError(e) ? e.message : String(e);

function matches(c: Contact, query: string) {
  if (!query) return true;
  const text = [c.name, c.company, ...c.emails, ...c.phones];
  if (text.some(value => value.toLocaleLowerCase().includes(query))) return true;
  const digits = query.replace(/\D/g, "");
  return !!digits && /^[\d\s()+.\-]+$/.test(query) && c.phones.some(p => p.replace(/\D/g, "").includes(digits));
}

export function Contacts() {
  const [data, { refetch }] = createResource(() => run("contacts.list", {}));
  const [account, setAccount] = createSignal("");
  const [search, setSearch] = createSignal("");
  const [selection, select] = createSignal<Pick<Contact, "account" | "remote_id">>();
  const [editor, setEditor] = createSignal<{ original?: Contact }>();
  const [view, setView] = createSignal("all");
  const [checked, setChecked] = createSignal(new Set<string>());
  const [mergeCards, setMergeCards] = createSignal<Contact[]>();
  const [busy, setBusy] = createSignal(false);
  const [status, setStatus] = createSignal("");
  const clear = () => setChecked(new Set<string>());
  function toggle(c: Contact) {
    const next = new Set(checked());
    if (!next.delete(key(c))) next.add(key(c));
    setChecked(next);
  }
  let searchInput!: HTMLInputElement;
  let newButton!: HTMLButtonElement;
  const accounts = () => data.error ? [] : data()?.accounts ?? [];
  const query = () => search().trim().toLocaleLowerCase();
  const ordered = createMemo(() => [...(data.error ? [] : data()?.contacts ?? [])]
    .sort((a, b) => title(a).localeCompare(title(b)) || a.account.localeCompare(b.account)));
  const groups = createMemo(() => duplicateGroups(ordered()));
  const duplicates = createMemo(() => new Set(groups().flatMap(g => g.cards.map(key))));
  const rows = createMemo(() => ordered().filter(c => (!account() || c.account === account())
    && matches(c, query()) && (view() !== "favourites" || c.starred)
    && (view() !== "duplicates" || duplicates().has(key(c)))));
  // A store refresh returns new objects even for unchanged contacts. Key the DOM rows by card identity.
  const visibleByKey = createMemo(() => new Map(rows().map(c => [key(c), c])));
  const selected = createMemo(() => rows().filter(c => checked().has(key(c))));
  const sameAccount = () => selected().every(c => c.account === selected()[0]?.account);
  const current = createMemo(() => rows().find(c =>
    c.account === selection()?.account && c.remote_id === selection()?.remote_id) ?? rows()[0]);
  onMount(() => {
    const stop = onStoreChanged(() => void refetch());
    function shortcut(event: KeyboardEvent) {
      if (event.defaultPrevented || document.querySelector('dialog[open], [role="dialog"], [role="alertdialog"]')) return;
      switch (keyName(event)) {
        case "Ctrl+N": event.preventDefault(); newButton.click(); break;
        case "Ctrl+F": event.preventDefault(); searchInput.focus(); searchInput.select(); break;
      }
    }
    window.addEventListener("keydown", shortcut);
    onCleanup(() => {
      window.removeEventListener("keydown", shortcut);
      void stop.then(f => f());
    });
  });

  function move(event: KeyboardEvent) {
    const last = rows().length - 1;
    if (last < 0) return;
    const at = rows().findIndex(c => c === current());
    let next: number;
    switch (event.key) {
      case "ArrowDown": next = Math.min(at + 1, last); break;
      case "ArrowUp": next = Math.max(at - 1, 0); break;
      case "Home": next = 0; break;
      case "End": next = last; break;
      default: return;
    }
    event.preventDefault();
    const c = rows()[next]!;
    select(c);
    document.getElementById("contact-" + c.id)?.focus();
  }

  async function saved(id: number, owner: string) {
    setEditor(undefined);
    notify("Contact saved");
    const result = await refetch();
    const contact = result?.contacts.find(c => c.id === id && c.account === owner);
    if (contact) batch(() => {
      clear(); setView("all");
      if (account() && account() !== owner) setAccount(owner);
      if (!matches(contact, query())) setSearch("");
      select(contact);
    });
  }
  async function write(action: () => Promise<void>) {
    if (busy()) return;
    setBusy(true);
    setStatus("");
    try { await action(); }
    catch (e) { setStatus(errorText(e)); }
    finally {
      try { await refetch(); }
      catch (e) { notify(errorText(e), true); }
      setBusy(false);
    }
  }
  async function remove(cards: Contact[], merging: boolean) {
    let done = 0;
    for (const c of cards) {
      try {
        await run("contacts.delete", { ...target(c), yes: true,
          expected_json: merging ? JSON.stringify(snapshot(c)) : undefined });
      } catch (e) {
        throw new Error(`${merging ? "Merged card saved. " : ""}Deleted ${done} of ${cards.length}. Stopped at ${title(c)}. ${errorText(e)}`);
      }
      done++;
      setChecked(prev => { const next = new Set(prev); next.delete(key(c)); return next; });
    }
  }
  function confirmDelete(cards: Contact[]) {
    const owners = [...new Set(cards.map(c => c.account))].join(", ");
    const count = cards.length;
    setConfirmDialog({
      title: count === 1 ? "Delete contact" : "Delete contacts",
      text: `Delete ${count === 1 ? title(cards[0]!) : count + " selected contacts"} from ${owners}?`,
      action: count === 1 ? "Delete" : `Delete ${count} contacts`,
      onConfirm: () => void write(async () => {
        await remove(cards, false);
        notify(count === 1 ? "Contact deleted" : `Deleted ${count} contacts`);
      }),
    });
  }
  async function merge(keep: Contact, cards: Contact[]) {
    setMergeCards(undefined);
    const fields = mergeFields(keep, cards);
    await write(async () => {
      const saved = await run("contacts.edit", { ...target(keep), expected_json: JSON.stringify(snapshot(keep)),
        // A name equal to the current name preserves Google's structured name.
        name: fields.name, company: fields.company !== keep.company ? fields.company : undefined,
        emails: JSON.stringify(fields.emails) !== JSON.stringify(keep.emails) ? fields.emails : undefined,
        phones: JSON.stringify(fields.phones) !== JSON.stringify(keep.phones) ? fields.phones : undefined,
        starred: fields.starred && !keep.starred ? true : undefined,
      });
      const book = await run("contacts.list", {});
      const updated = book.contacts.find(c => c.id === saved.contact && c.account === keep.account);
      if (!updated || !includesMerge(updated, fields)) {
        throw new Error("The saved card is missing a previewed field. All source cards have been kept.");
      }
      select(updated);
      const others = cards.filter(c => key(c) !== key(keep));
      if (others.some(c => key(c) === key(updated))) {
        throw new Error("The provider changed the kept card's identity. Review the cards again.");
      }
      setChecked(new Set([updated, ...others].map(key)));
      await remove(others, true);
      batch(() => { clear(); setView("all"); if (!matches(updated, query())) setSearch(""); });
      notify(`Merged ${cards.length} contacts`);
    });
  }

  return <div class="contacts-view">
    <section class="contacts-browser" aria-label="Contact browser">
      <header><h1>Contacts</h1>
        <button ref={newButton} class="contact-primary" aria-keyshortcuts="Control+N Meta+N"
          disabled={busy() || !accounts().length} onClick={() => setEditor({})}>New contact</button>
      </header>
      <div class="contacts-filters">
        <input ref={searchInput} type="search" aria-label="Search contacts" aria-keyshortcuts="Control+F Meta+F"
          placeholder="Search contacts" value={search()}
          disabled={busy()} onInput={e => { clear(); setSearch(e.currentTarget.value); }} />
        <select aria-label="Filter contacts by account" value={account()} disabled={busy()} onChange={e => { clear(); setAccount(e.currentTarget.value); }}>
          <option value="">All accounts</option>
          <For each={accounts()}>{a => <option value={a} selected={a === account()}>{a}</option>}</For>
        </select>
      </div>
      <div class="contact-views" aria-label="Contact views">
        <For each={[["all", "All contacts"], ["favourites", "Favourites"], ["duplicates", "Likely duplicates"]] as const}>{([id, label]) =>
          <button disabled={busy()} aria-pressed={view() === id}
            onClick={() => { clear(); setView(id); }}>{label}</button>
        }</For>
      </div>
      <Show when={status()}><div class="contacts-result" role="alert">{status()}</div></Show>
      <Show when={busy()}><div class="contacts-result" role="status">Updating contacts…</div></Show>
      <Show when={rows().length} fallback={
        <div class="contacts-empty" role="status">
          {data.error ? <><p>{errorText(data.error.cause ?? data.error)}</p><button onClick={() => void refetch()}>Retry</button></>
            : data.loading ? "Loading contacts…" : query() ? "No matching contacts." : view() === "favourites" ? "No Google favourites."
              : view() === "duplicates" ? "No likely duplicates in this account view." : "No saved contacts."}
        </div>
      }>
        <div class="contact-selection">
          <label><input type="checkbox" disabled={busy()} aria-label="Select all visible contacts"
            checked={selected().length === rows().length} ref={el => createEffect(() => { el.indeterminate = selected().length > 0 && selected().length < rows().length; })}
            onChange={e => setChecked(new Set(e.currentTarget.checked ? rows().map(key) : []))} />
            {selected().length ? selected().length + " selected" : rows().length + " contacts"}</label>
          <Show when={selected().length}>
            <button disabled={busy() || selected().length < 2 || !sameAccount()}
              onClick={() => setMergeCards(selected())}>Merge…</button>
            <button disabled={busy()} onClick={() => confirmDelete(selected())}>Delete…</button>
            <Show when={!sameAccount()}><small>Choose cards in one account to merge.</small></Show>
          </Show>
        </div>
        <div class="contacts-list" aria-label="Saved contacts">
          <For each={[...visibleByKey().keys()]}>{id => {
            const card = () => visibleByKey().get(id)!;
            return <div class="contact-row" classList={{ active: card() === current() }}>
              <input type="checkbox" aria-label={"Select " + title(card()) + " in " + card().account}
                disabled={busy()} checked={checked().has(id)} onChange={() => toggle(card())} />
              <button id={"contact-" + card().id} aria-current={card() === current() ? "true" : undefined}
                onFocus={() => select(card())} onClick={() => select(card())} onKeyDown={move}>
                <strong>{card().starred ? "★ " : ""}{title(card())}</strong>
                <span>{[card().emails[0], card().phones[0]].filter(Boolean).join(" · ")}</span><small>{card().account}</small>
              </button>
            </div>;
          }}</For>
        </div>
      </Show>
    </section>
    <section class="contact-details" aria-label="Contact details">
      <Show when={current()}>{selected => <>
        <h2>{title(selected())}</h2>
        <p class="contact-owner">{selected().account}</p>
        <div class="contact-actions">
          <button disabled={busy()} onClick={() => setEditor({ original: selected() })}>Edit</button>
          <button disabled={busy()} onClick={() => confirmDelete([selected()])}>Delete</button>
        </div>
        <Show when={selected().google_account}>
          <div class="contact-actions contact-google">
            <button disabled={busy()} aria-pressed={selected().starred} onClick={() => {
              const c = selected();
              void write(async () => {
                await run("contacts.edit", { ...target(c), starred: !c.starred });
                notify(c.starred ? "Removed from Google favourites" : "Added to Google favourites");
              });
            }}>{selected().starred ? "★ Favourited" : "☆ Favourite"}</button>
            <Show when={googleLink(selected())}>{url =>
              <button onClick={() => void openLink(url()).catch(e => notify(errorText(e), true))}>Open in Google Contacts ↗</button>
            }</Show>
          </div>
        </Show>
        <Show when={view() === "duplicates"}>
          <For each={groups().filter(g => g.cards.includes(selected()))}>{group =>
            <button class="contact-match" disabled={busy()} onClick={() => {
              setSearch(""); setChecked(new Set(group.cards.map(key)));
            }}>Select {group.cards.length} matching “{group.reason}”</button>
          }</For>
        </Show>
        <dl>
          <Show when={selected().company}><dt>Company</dt><dd>{selected().company}</dd></Show>
          <Show when={selected().emails.length}><dt>Email addresses</dt>
            <For each={selected().emails}>{email =>
              <dd><button class="contact-email" title={"Compose to " + email} aria-label={"Compose to " + email}
                onClick={() => void openCompose({ kind: "new", account: selected().account, to: [email] })
                  .catch(e => notify(errorText(e), true))}>{email}</button></dd>
            }</For>
          </Show>
          <Show when={selected().phones.length}><dt>Phone numbers</dt>
            <For each={selected().phones}>{phone => <dd>{phone}</dd>}</For>
          </Show>
        </dl>
      </>}</Show>
    </section>
    <Show when={mergeCards()} keyed>{cards =>
      <MergeDialog cards={cards} onClose={() => setMergeCards(undefined)} onMerge={keep => void merge(keep, cards)} />
    }</Show>
    <Show when={editor()} keyed>{draft =>
      <ContactEditor original={draft.original} accounts={accounts()}
        initialAccount={account() || (accounts().length === 1 ? accounts()[0]! : "")}
        onClose={() => setEditor(undefined)} onSaved={(id, owner) => void saved(id, owner)} />
    }</Show>
  </div>;
}
