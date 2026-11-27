import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import type { Contact } from "./ContactEditor";
import { mergeFields, title } from "./book";

export function MergeDialog(props: {
  cards: Contact[]; onClose: () => void; onMerge: (keep: Contact) => void;
}) {
  const [keep, setKeep] = createSignal(props.cards[0]!);
  const result = createMemo(() => mergeFields(keep(), props.cards));
  let dialog!: HTMLDialogElement;
  onMount(() => dialog.showModal());
  return <dialog ref={dialog} class="contact-editor" aria-labelledby="merge-title"
    onCancel={e => { e.preventDefault(); props.onClose(); }}>
    <h2 id="merge-title">Merge {props.cards.length} contacts</h2>
    <fieldset class="merge-choices">
      <legend>Choose the card to keep in {keep().account}</legend>
      <For each={props.cards}>{c => <label>
        <input type="radio" name="keeper" checked={keep() === c} onChange={() => setKeep(c)} />
        <span><strong>{title(c)}</strong><br />{[c.company, ...c.emails, ...c.phones].filter(Boolean).join(" · ")}</span>
      </label>}</For>
    </fieldset>
    <h3>Merged card</h3>
    <strong>{result().name || title(keep())}</strong>
    <Show when={result().starred}> ★</Show>
    <Show when={result().company}><div>{result().company}</div></Show>
    <For each={[...result().emails, ...result().phones]}>{value => <div>{value}</div>}</For>
    <p>The other {props.cards.length === 2 ? "card will" : `${props.cards.length - 1} cards will`} be deleted. Notes, photos, labels and other details on those cards will be lost.</p>
    <div class="contact-actions">
      <button type="button" autofocus onClick={props.onClose}>Cancel</button>
      <button type="button" class="contact-primary" onClick={() => props.onMerge(keep())}>Merge contacts</button>
    </div>
  </dialog>;
}
