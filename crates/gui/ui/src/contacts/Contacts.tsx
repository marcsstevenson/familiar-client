import { For, Show, batch, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { isCommandError, run } from "../api";
import { openCompose } from "../compose/spec";
import { onStoreChanged } from "../list/api";
import { notify, setConfirmDialog } from "../state";
import { ContactEditor, type Contact } from "./ContactEditor";
import "./contacts.css";

const title = (c: Contact) => c.name || c.company || c.emails[0] || c.phones[0] || "Unnamed contact";
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
  const [deleting, setDeleting] = createSignal(false);
  let list!: HTMLDivElement;
  const accounts = () => data.error ? [] : data()?.accounts ?? [];
  const query = () => search().trim().toLocaleLowerCase();
  const ordered = createMemo(() => [...(data.error ? [] : data()?.contacts ?? [])]
    .sort((a, b) => title(a).localeCompare(title(b)) || a.account.localeCompare(b.account)));
  const rows = createMemo(() => ordered()
    .filter(c => (!account() || c.account === account()) && matches(c, query())));
  const current = createMemo(() => rows().find(c =>
    c.account === selection()?.account && c.remote_id === selection()?.remote_id) ?? rows()[0]);
  onMount(() => {
    const stop = onStoreChanged(() => void refetch());
    onCleanup(() => void stop.then(f => f()));
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
    document.getElementById("contact-" + c.id)?.scrollIntoView({ block: "nearest" });
  }

  async function saved(id: number, owner: string) {
    setEditor(undefined);
    notify("Contact saved");
    const result = await refetch();
    const contact = result?.contacts.find(c => c.id === id && c.account === owner);
    if (contact) batch(() => {
      if (account() && account() !== owner) setAccount(owner);
      if (!matches(contact, query())) setSearch("");
      select(contact);
    });
  }
  function confirmDelete(c: Contact) {
    setConfirmDialog({
      title: "Delete contact", text: `Delete ${title(c)} from ${c.account}?`, action: "Delete",
      onConfirm: async () => {
        setDeleting(true);
        try {
          await run("contacts.delete", { account: c.account, contact: c.id, remote_id: c.remote_id, yes: true });
          notify("Contact deleted");
          await refetch();
        } catch (e) { notify(errorText(e), true); }
        finally { setDeleting(false); }
      },
    });
  }

  return <div class="contacts-view">
    <section class="contacts-browser" aria-label="Contact browser">
      <header><h1>Contacts</h1>
        <button class="contact-primary" disabled={!accounts().length} onClick={() => setEditor({})}>New contact</button>
      </header>
      <div class="contacts-filters">
        <input type="search" aria-label="Search contacts" placeholder="Search contacts" value={search()}
          onInput={e => setSearch(e.currentTarget.value)} />
        <select aria-label="Filter contacts by account" value={account()} onChange={e => setAccount(e.currentTarget.value)}>
          <option value="">All accounts</option>
          <For each={accounts()}>{a => <option value={a} selected={a === account()}>{a}</option>}</For>
        </select>
      </div>
      <Show when={rows().length} fallback={
        <div class="contacts-empty" role="status">
          {data.error ? <><p>{errorText(data.error.cause ?? data.error)}</p><button onClick={() => void refetch()}>Retry</button></>
            : data.loading ? "Loading contacts…" : query() ? "No matching contacts." : "No saved contacts."}
        </div>
      }>
        <div class="contacts-list" role="listbox" aria-label="Saved contacts" tabIndex={0} ref={list}
          aria-activedescendant={current() && "contact-" + current()!.id} onKeyDown={move}>
          <For each={rows()}>{c =>
            <div id={"contact-" + c.id} role="option" aria-selected={c === current()}
              onClick={() => { select(c); list.focus(); }}>
              <strong>{title(c)}</strong><span>{c.account}</span>
            </div>
          }</For>
        </div>
      </Show>
    </section>
    <section class="contact-details" aria-label="Contact details">
      <Show when={current()}>{selected => <>
        <h2>{title(selected())}</h2>
        <p class="contact-owner">{selected().account}</p>
        <div class="contact-actions">
          <button disabled={deleting()} onClick={() => setEditor({ original: selected() })}>Edit</button>
          <button disabled={deleting()} onClick={() => confirmDelete(selected())}>Delete</button>
        </div>
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
    <Show when={editor()} keyed>{draft =>
      <ContactEditor original={draft.original} accounts={accounts()}
        initialAccount={account() || (accounts().length === 1 ? accounts()[0]! : "")}
        onClose={() => setEditor(undefined)} onSaved={(id, owner) => void saved(id, owner)} />
    }</Show>
  </div>;
}
