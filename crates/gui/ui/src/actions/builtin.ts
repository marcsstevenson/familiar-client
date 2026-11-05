// The built-in actions. The message actions run the same registry commands as
// `fam`, on the selected message.
import { isCommandError, run } from "../api";
import { listNav, notify, selectedMessage, selectedRow } from "../state";
import { cycleTheme } from "../theme";
import { registerAction } from "./registry";

const hasMessage = () => selectedMessage() !== null;

async function onSelected(
  label: string,
  f: (id: number) => Promise<unknown>,
): Promise<void> {
  const id = selectedMessage();
  if (id === null) return;
  try {
    await f(id);
    notify(label);
  } catch (e) {
    notify(isCommandError(e) ? e.message : String(e), true);
  }
}

export function registerBuiltinActions(openPalette: (prefill?: string, values?: Record<string, string | boolean>) => void): void {
  registerAction({ id: "palette", title: "Show all commands", key: "Ctrl+K", run: () => openPalette() });
  registerAction({ id: "next", title: "Next message", key: "j", run: () => listNav.move(1) });
  registerAction({ id: "previous", title: "Previous message", key: "k", run: () => listNav.move(-1) });
  registerAction({
    id: "archive",
    title: "Archive",
    key: "e",
    enabled: hasMessage,
    cli: () => `fam mail archive ${selectedMessage()}`,
    run: () => onSelected("Archived", (id) => run("mail.archive", { messages: [id] })),
  });
  registerAction({
    id: "delete",
    title: "Delete",
    key: "#",
    enabled: hasMessage,
    cli: () => `fam mail delete ${selectedMessage()}`,
    run: () => onSelected("Moved to Deleted Items", (id) => run("mail.delete", { messages: [id] })),
  });
  registerAction({
    id: "flag",
    title: "Flag or clear flag",
    key: "s",
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
  // Until the compose window lands, these open the command's form in the palette.
  registerAction({
    id: "reply",
    title: "Reply",
    key: "r",
    enabled: hasMessage,
    cli: () => `fam draft reply ${selectedMessage()}`,
    run: () => openPalette("draft.reply"),
  });
  registerAction({
    id: "reply_all",
    title: "Reply all",
    key: "a",
    enabled: hasMessage,
    cli: () => `fam draft reply ${selectedMessage()} --all`,
    run: () => openPalette("draft.reply", { all: true }),
  });
  registerAction({
    id: "forward",
    title: "Forward",
    key: "f",
    enabled: hasMessage,
    cli: () => `fam draft forward ${selectedMessage()}`,
    run: () => openPalette("draft.forward"),
  });
  registerAction({
    id: "compose",
    title: "New message",
    key: "c",
    cli: () => "fam draft create",
    run: () => openPalette("draft.create"),
  });
  registerAction({ id: "theme", title: "Change theme (system, light, dark)", run: cycleTheme });
}
