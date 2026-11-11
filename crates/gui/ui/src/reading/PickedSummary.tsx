// What the reading pane shows while several messages are picked in the list:
// how many, and the actions that work on them all (as Outlook and Apple Mail
// do). The same actions are on the keys (Delete, e, Ctrl+Shift+V) and the
// list's context menu.
import { run } from "../api";
import { pickMoveTarget } from "../actions/builtin";
import { countLabel, onPicked, picked, removePicked, setPicked } from "../state";

export function PickedSummary() {
  const accounts = () => new Set(picked().values()).size;
  return (
    <div class="picked-summary">
      <span class="reading-empty-kea" aria-hidden="true" />
      <h2>{countLabel(picked().size)} picked</h2>
      <p class="picked-note">
        {accounts() > 1 ? `From ${accounts()} accounts. ` : ""}Ctrl+click to add or take out one, Shift+click for a run.
      </p>
      <div class="picked-actions">
        <button onClick={pickMoveTarget}>Move to folder…</button>
        <button onClick={() => void removePicked("Archived", (ids) => run("mail.archive", { messages: ids }))}>Archive</button>
        <button class="danger" onClick={() => void removePicked("Moved to Deleted Items", (ids) => run("mail.delete", { messages: ids }))}>
          Delete
        </button>
      </div>
      <div class="picked-actions">
        <button onClick={() => void onPicked("Marked read", (ids) => run("mail.read", { messages: ids, unread: false }))}>Mark read</button>
        <button onClick={() => void onPicked("Marked unread", (ids) => run("mail.read", { messages: ids, unread: true }))}>Mark unread</button>
        <button onClick={() => void onPicked("Flagged", (ids) => run("mail.flag", { messages: ids, clear: false }))}>Flag</button>
        <button onClick={() => void onPicked("Flag cleared", (ids) => run("mail.flag", { messages: ids, clear: true }))}>Clear flag</button>
      </div>
      <button class="picked-clear" onClick={() => setPicked(new Map())}>
        Clear selection (Esc)
      </button>
    </div>
  );
}
