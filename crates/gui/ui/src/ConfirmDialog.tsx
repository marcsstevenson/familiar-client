// A question before an action that can't be undone, such as deleting mail for
// good. Enter (on the focused action button) confirms; Esc or a click outside
// cancels.
import { onMount } from "solid-js";
import type { Confirm } from "./state";

export function ConfirmDialog(props: { confirm: Confirm; onClose: () => void }) {
  // A copy: the dialog closes (and its props go stale) before the action runs.
  const c = { ...props.confirm };
  let action!: HTMLButtonElement;
  onMount(() => action.focus());
  const confirm = () => {
    props.onClose();
    c.onConfirm();
  };
  return (
    <div
      class="folder-menu-backdrop modal"
      role="dialog"
      aria-label={c.title}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          props.onClose();
        }
      }}
    >
      <div class="folder-dialog" role="alertdialog" aria-label={c.title}>
        <p>{c.text}</p>
        <div class="folder-dialog-buttons">
          <button type="button" onClick={props.onClose}>
            Cancel
          </button>
          <button type="button" class="danger-fill" ref={(el) => (action = el)} onClick={confirm}>
            {c.action}
          </button>
        </div>
      </div>
    </div>
  );
}
