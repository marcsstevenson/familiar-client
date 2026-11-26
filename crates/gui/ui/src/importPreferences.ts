// One-time upgrade from older webview settings. Import only absent TOML keys,
// and retire each browser value only after its config write succeeds.
import { invoke } from "@tauri-apps/api/core";
import { run, type DesktopPreferences, type Theme } from "./api";
import { views, type CalendarView } from "./calendar/timeline";
import { readSession, writeSession } from "./session";
import { notify } from "./state";

export async function importPreferences() {
  type Patch = Partial<DesktopPreferences> & { theme?: Theme };
  async function migrate(key: string, convert: (raw: string) => Patch | Promise<Patch>) {
    const raw = localStorage.getItem(key);
    if (raw === null) return;
    try {
      const changes = await convert(raw);
      await invoke("save_preferences", { changes, import: true });
      localStorage.removeItem(key);
    } catch (e) { notify("Couldn't import " + key + ": " + e, true); }
  }
  await migrate("familiar.accent", raw => ({ accent: raw }));
  await migrate("familiar.theme", raw => ({ theme: raw as Theme }));
  await migrate("familiar.neon", raw => ({ neon: raw === "true" }));
  await migrate("familiar.panes", raw => ({ panes: JSON.parse(raw) }));
  await migrate("familiar.folders.collapsed", async raw => {
    const old = JSON.parse(raw) as string[];
    const collapsed = old.filter(key => key.startsWith("a:"));
    if (old.some(key => key.startsWith("f:"))) {
      const { folders } = await run("folder.list", {});
      if (!folders.length) throw new Error("folder list is empty; will retry next launch");
      for (const f of folders) if (old.includes("f:" + f.id)) collapsed.push("f:" + f.account + ":" + f.remote_id);
    }
    return { collapsed };
  });
  await migrate("familiar.calendar-view", async raw => {
    const old = JSON.parse(raw) as { view?: CalendarView; hidden?: number[] };
    const changes: Patch = {};
    if (old.view && views.includes(old.view)) changes.calendar_view = old.view;
    if (old.hidden?.length) {
      const { calendars } = await run("cal.calendars", {});
      if (!calendars.length) throw new Error("calendar list is empty; will retry next launch");
      changes.hidden_calendars = calendars.filter(c => old.hidden!.includes(c.id)).map(c => c.account + ":" + c.remote_id);
    }
    return changes;
  });
  await migrate("familiar.event-calendar", async raw => {
    const old = JSON.parse(raw) as Record<string, { account: string; name: string; id?: number }>;
    const { calendars } = await run("cal.calendars", {});
    const sender_calendars = Object.fromEntries(Object.entries(old).map(([sender, { account, name, id }]) => {
      const match = calendars.find(c => c.id === id && c.account === account);
      return [sender, { account, name, remote_id: match?.remote_id }];
    }));
    return { sender_calendars };
  });
  const list = readSession<{ sort: DesktopPreferences["sort"]; unread: boolean }>("list");
  if (list) {
    await invoke("save_preferences", { changes: { sort: list.sort, unread_only: list.unread }, import: true });
    writeSession("list", undefined);
  }
  const nav = readSession<{ dayPanelOpen?: boolean; calendar?: string }>("navigation");
  if (nav?.dayPanelOpen !== undefined || nav?.calendar === "panel") {
    await invoke("save_preferences", { changes: { day_panel_open: nav.dayPanelOpen ?? true }, import: true });
  }
}
