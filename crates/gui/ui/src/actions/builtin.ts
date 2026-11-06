// The built-in actions. The message actions run the same registry commands as
// `fam`, on the selected message.
import { isCommandError, run } from "../api";
import { listNav, notify, selectedMessage, selectedRow } from "../state";
import { searchFocus } from "../list/SearchBar";
import { cycleTheme } from "../theme";
import { openCompose } from "../compose/spec";
import { registerAction } from "./registry";

const hasMessage = () => selectedMessage() !== null;

async function onSelected(
  label: string,
  f: (id: number) => Promise<unknown>,
  leaves = false,
): Promise<void> {
  const id = selectedMessage();
  if (id === null) return;
  if (leaves) listNav.leaving = id;
  try {
    await f(id);
    notify(label);
  } catch (e) {
    notify(isCommandError(e) ? e.message : String(e), true);
  }
}

export function registerBuiltinActions(openPalette: (prefill?: string) => void): void {
  registerAction({ id: "palette", title: "Show all commands", key: "Ctrl+K", run: () => openPalette() });
  registerAction({ id: "next", title: "Next message", key: "j", run: () => listNav.move(1) });
  registerAction({ id: "previous", title: "Previous message", key: "k", run: () => listNav.move(-1) });
  registerAction({
    id: "archive",
    title: "Archive",
    key: "e",
    also: ["Backspace"],
    enabled: hasMessage,
    cli: () => `fam mail archive ${selectedMessage()}`,
    run: () => onSelected("Archived", (id) => run("mail.archive", { messages: [id] }), true),
  });
  registerAction({
    id: "delete",
    title: "Delete",
    key: "#",
    also: ["Delete", "Ctrl+D"],
    enabled: hasMessage,
    cli: () => `fam mail delete ${selectedMessage()}`,
    run: () => onSelected("Moved to Deleted Items", (id) => run("mail.delete", { messages: [id] }), true),
  });
  registerAction({
    id: "flag",
    title: "Flag or clear flag",
    key: "s",
    also: ["Insert"],
    enabled: hasMessage,
    cli: () => `fam mail flag ${selectedMessage()}${selectedRow()?.is_flagged ? " --clear" : ""}`,
    run: () => {
      const clear = selectedRow()?.is_flagged ?? false;
      return onSelected(clear ? "Flag cleared" : "Flagged", (id) =>
        run("mail.flag", { messages: [id], clear }),
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
      return onSelected(unread ? "Marked unread" : "Marked read", (id) =>
        run("mail.read", { messages: [id], unread }),
      );
    },
  });
  // Outlook's separate read and unread keys.
  registerAction({
    id: "mark_read",
    title: "Mark read",
    also: ["Ctrl+Q"],
    enabled: hasMessage,
    cli: () => `fam mail read ${selectedMessage()}`,
    run: () => onSelected("Marked read", (id) => run("mail.read", { messages: [id], unread: false })),
  });
  registerAction({
    id: "mark_unread",
    title: "Mark unread",
    also: ["Ctrl+U"],
    enabled: hasMessage,
    cli: () => `fam mail read ${selectedMessage()} --unread`,
    run: () => onSelected("Marked unread", (id) => run("mail.read", { messages: [id], unread: true })),
  });
  registerAction({
    id: "move",
    title: "Move to folder",
    also: ["Ctrl+Shift+V"],
    enabled: hasMessage,
    run: () => openPalette("mail.move"),
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
