import { For, Show, createSignal, onMount } from "solid-js";
import { type Commands, isCommandError, run } from "../api";

export type Contact = Commands["contacts.list"]["output"]["contacts"][number];
type Fields = Pick<Commands["contacts.edit"]["input"], "name" | "company" | "emails" | "phones">;

export function ContactEditor(props: {
  original?: Contact; accounts: string[]; initialAccount: string;
  onClose: () => void; onSaved: (id: number, account: string) => void;
}) {
  const original = props.original;
  const [account, setAccount] = createSignal(original?.account ?? props.initialAccount);
  const [name, setName] = createSignal(original?.name ?? "");
  const [company, setCompany] = createSignal(original?.company ?? "");
  const [emails, setEmails] = createSignal(original?.emails.join("\n") ?? "");
  const [phones, setPhones] = createSignal(original?.phones.join("\n") ?? "");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let dialog!: HTMLDialogElement;
  onMount(() => dialog.showModal());

  const lines = (value: string) => value.split("\n").map(s => s.trim()).filter(Boolean);
  function changes(): Fields {
    const fields: Fields = {};
    if (name() !== (original?.name ?? "")) fields.name = name().trim();
    if (company() !== (original?.company ?? "")) fields.company = company().trim();
    if (emails() !== (original?.emails.join("\n") ?? "")) fields.emails = lines(emails());
    if (phones() !== (original?.phones.join("\n") ?? "")) fields.phones = lines(phones());
    return fields;
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (busy()) return;
    setError("");
    setBusy(true);
    try {
      const fields = changes();
      const result = original
        ? await run("contacts.edit", { account: original.account, contact: original.id, remote_id: original.remote_id, ...fields })
        : await run("contacts.create", { account: account(), ...fields });
      props.onSaved(result.contact, account());
    } catch (e) {
      setError(isCommandError(e) ? e.message : String(e));
    } finally { setBusy(false); }
  }
  return <dialog ref={dialog} class="contact-editor" aria-labelledby="contact-editor-title"
    onCancel={e => { e.preventDefault(); if (!busy()) props.onClose(); }}>
    <form onSubmit={save}>
      <h2 id="contact-editor-title">{original ? "Edit contact" : "New contact"}</h2>
      <fieldset disabled={busy()}>
        <label>Account
          <Show when={!original} fallback={<span>{original?.account}</span>}>
            <select required value={account()} onChange={e => setAccount(e.currentTarget.value)}>
              <option value="" disabled>Choose an account</option>
              <For each={props.accounts}>{a => <option value={a} selected={a === account()}>{a}</option>}</For>
            </select>
          </Show>
        </label>
        <label>Name<input value={name()} onInput={e => setName(e.currentTarget.value)} /></label>
        <label>Company<input value={company()} onInput={e => setCompany(e.currentTarget.value)} /></label>
        <label>Email addresses <small>One per line</small>
          <textarea rows="3" spellcheck={false} value={emails()} onInput={e => setEmails(e.currentTarget.value)} />
        </label>
        <label>Phone numbers <small>One per line</small>
          <textarea rows="3" value={phones()} onInput={e => setPhones(e.currentTarget.value)} />
        </label>
      </fieldset>
      <Show when={error()}><p class="contact-error" role="alert">{error()}</p></Show>
      <div class="contact-actions">
        <button type="button" disabled={busy()} onClick={props.onClose}>Cancel</button>
        <button class="contact-primary" type="submit"
          disabled={busy() || !account() || Object.keys(changes()).length === 0}>{busy() ? "Saving…" : "Save"}</button>
      </div>
    </form>
  </dialog>;
}
