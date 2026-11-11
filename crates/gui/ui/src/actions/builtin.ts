// The built-in actions. The message actions run the same registry commands as
// `fam`, on the selected message.
import { isCommandError, run } from "../api";
import { listNav, multiPicked, notify, onPicked, picked, pickedMessages, processMessage, processedFolders, readingFrom, removeFromList, removePicked, selectedMessage, selectedRow, setEventDialog, setMovePicker, setPicked, setSelectedMessage } from "../state";
import { searchFocus } from "../list/SearchBar";
import { cycleTheme } from "../theme";
import { openCompose } from "../compose/spec";
import { registerAction } from "./registry";

const hasMessage = () => selectedMessage() !== null;
/** The open message, or a multiple selection in the list. */
const hasMessages = () => hasMessage() || multiPicked();
/** The IDs an action works on: the multiple selection, else the open message. */
const targets = (): number[] => {
  if (multiPicked()) return pickedMessages().ids;
  const id = selectedMessage();
  return id === null ? [] : [id];
};
/** For the palette: the IDs as `fam` takes them. */
const targetArgs = () => targets().join(" ");

async function onSelected(
  label: string,
  f: (ids: number[]) => Promise<unknown>,
): Promise<void> {
  if (multiPicked()) return onPicked(label, f);
  const id = selectedMessage();
  if (id === null) return;
  try {
    await f([id]);
    notify(label);
  } catch (e) {
    notify(isCommandError(e) ? e.message : String(e), true);
  }
}

function onSelectedLeaves(label: string, f: (ids: number[]) => Promise<unknown>) {
  if (multiPicked()) return removePicked(label, f);
  const id = selectedMessage();
  return id === null ? undefined : removeFromList(id, label, () => f([id]));
}

/** Opens the Move to folder picker for the selection. */
export function pickMoveTarget(): void {
  if (multiPicked()) {
    setMovePicker({ ids: pickedMessages().ids, accounts: [...new Set(picked().values())] });
    return;
  }
  const r = selectedRow();
  const id = selectedMessage();
  if (id !== null && r !== null && r.id === id) setMovePicker({ ids: [id], accounts: [r.account] });
}

export function registerBuiltinActions(openPalette: (prefill?: string) => void): void {
  registerAction({ id: "palette", title: "Show all commands", key: "Ctrl+K", run: () => openPalette() });
  registerAction({ id: "next", title: "Next message", key: "j", run: () => listNav.move(1) });
  registerAction({ id: "previous", title: "Previous message", key: "k", run: () => listNav.move(-1) });
  // Dialogs, menus, search and other inputs keep Esc for themselves
  // (onGlobalKey skips them), so here it only closes the open message.
  registerAction({
    id: "close",
    title: "Close the message, or clear a multiple selection",
    key: "Escape",
    enabled: hasMessages,
    run: () => {
      setPicked(new Map());
      setSelectedMessage(null);
    },
  });
  registerAction({
    id: "archive",
    title: "Archive",
    key: "e",
    also: ["Backspace"],
    enabled: hasMessages,
    cli: () => `fam mail archive ${targetArgs()}`,
    run: () => onSelectedLeaves("Archived", (ids) => run("mail.archive", { messages: ids })),
  });
  registerAction({
    id: "process",
    title: "Processed (move to the account's processed folder)",
    enabled: () => {
      const r = selectedRow();
      return r !== null && r.id === selectedMessage() && processedFolders[r.account] !== undefined;
    },
    cli: () => `fam mail process ${selectedMessage()}`,
    run: () => {
      const r = selectedRow();
      return r === null ? undefined : processMessage(r.id, r.account);
    },
  });
  registerAction({
    id: "report_junk",
    title: "Report junk (spam)",
    enabled: hasMessages,
    cli: () => `fam mail report ${targetArgs()}`,
    run: () => onSelectedLeaves("Reported as junk", (ids) => run("mail.report", { messages: ids })),
  });
  registerAction({
    id: "report_phishing",
    title: "Report phishing (scam)",
    enabled: hasMessages,
    cli: () => `fam mail report ${targetArgs()} --phishing`,
    run: () =>
      onSelectedLeaves("Reported as phishing", (ids) => run("mail.report", { messages: ids, phishing: true })),
  });
  registerAction({
    id: "delete",
    title: "Delete",
    key: "#",
    also: ["Delete", "Ctrl+D"],
    enabled: hasMessages,
    cli: () => `fam mail delete ${targetArgs()}`,
    run: () => onSelectedLeaves("Moved to Deleted Items", (ids) => run("mail.delete", { messages: ids })),
  });
  registerAction({
    id: "flag",
    title: "Flag or clear flag",
    key: "s",
    also: ["Insert"],
    enabled: hasMessages,
    cli: () => `fam mail flag ${targetArgs()}${!multiPicked() && selectedRow()?.is_flagged ? " --clear" : ""}`,
    run: () => {
      // A multiple selection is flagged; its menu also offers Clear flag.
      const clear = !multiPicked() && (selectedRow()?.is_flagged ?? false);
      return onSelected(clear ? "Flag cleared" : "Flagged", (ids) =>
        run("mail.flag", { messages: ids, clear }),
      );
    },
  });
  registerAction({
    id: "toggle_read",
    title: "Mark read or unread",
    key: "u",
    enabled: hasMessage,
    cli: () => `fam mail read ${selectedMessage()}${(selectedRow()?.is_read ?? true) ? " --unread" : ""}`,
    run: () => {
      const unread = selectedRow()?.is_read ?? true;
      return onSelected(unread ? "Marked unread" : "Marked read", (ids) =>
        run("mail.read", { messages: ids, unread }),
      );
    },
  });
  // Outlook's separate read and unread keys.
  registerAction({
    id: "mark_read",
    title: "Mark read",
    also: ["Ctrl+Q"],
    enabled: hasMessages,
    cli: () => `fam mail read ${targetArgs()}`,
    run: () => onSelected("Marked read", (ids) => run("mail.read", { messages: ids, unread: false })),
  });
  registerAction({
    id: "mark_unread",
    title: "Mark unread",
    also: ["Ctrl+U"],
    enabled: hasMessages,
    cli: () => `fam mail read ${targetArgs()} --unread`,
    run: () => onSelected("Marked unread", (ids) => run("mail.read", { messages: ids, unread: true })),
  });
  registerAction({
    id: "add_to_calendar",
    title: "Add to calendar (events in this email)",
    key: "Ctrl+Shift+A",
    enabled: hasMessage,
    cli: () => `fam cal propose --from-mail ${selectedMessage()}`,
    run: () => {
      const id = selectedMessage();
      if (id !== null) setEventDialog({ message: id, from: readingFrom() });
    },
  });
  registerAction({
    id: "move",
    title: "Move to folder",
    also: ["Ctrl+Shift+V"],
    enabled: hasMessages,
    cli: () => `fam mail move ${targetArgs()} --to <folder>`,
    run: pickMoveTarget,
  });
  // These open a compose window.
  const composeOn = (f: (id: number) => Parameters<typeof openCompose>[0]) => async () => {
    const id = selectedMessage();
    if (id !== null) await openCompose(f(id));
  };
  registerAction({
    id: "reply",
    title: "Reply",
    key: "r",
    also: ["Ctrl+R"],
    enabled: hasMessage,
    cli: () => `fam draft reply ${selectedMessage()}`,
    run: composeOn((message) => ({ kind: "reply", message })),
  });
  registerAction({
    id: "reply_all",
    title: "Reply all",
    key: "a",
    also: ["Ctrl+Shift+R"],
    enabled: hasMessage,
    cli: () => `fam draft reply ${selectedMessage()} --all`,
    run: composeOn((message) => ({ kind: "reply", message, all: true })),
  });
  registerAction({
    id: "forward",
    title: "Forward",
    key: "f",
    also: ["Ctrl+F"],
    enabled: hasMessage,
    cli: () => `fam draft forward ${selectedMessage()}`,
    run: composeOn((message) => ({ kind: "forward", message })),
  });
  registerAction({
    id: "compose",
    title: "New message",
    key: "c",
    cli: () => "fam draft create",
    run: () => openCompose({ kind: "new" }),
  });
  registerAction({ id: "search", title: "Search mail", key: "/", also: ["Ctrl+E", "F3"], cli: () => "fam mail search", run: () => searchFocus.focus() });
  registerAction({ id: "theme", title: "Change theme (system, light, dark)", run: cycleTheme });
}
