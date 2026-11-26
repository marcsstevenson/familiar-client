import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createStore, reconcile } from "solid-js/store";
import { uiInfo, type DesktopPreferences, type Theme, type UiInfo } from "./api";
import { notify } from "./state";

type Preferences = Partial<DesktopPreferences> & { theme: Theme };
const [settings, setPreferences] = createStore<Preferences>({ theme: "system" });
export const preferences = () => settings;
let pending = Promise.resolve();
let revision = 0;
let channel: BroadcastChannel;

function apply(info: UiInfo) {
  setPreferences(reconcile({ ...info.desktop, theme: info.theme }));
}

async function reload() {
  const version = ++revision;
  await pending;
  const info = await uiInfo();
  if (revision === version) apply(info);
}

export async function initPreferences() {
  channel = new BroadcastChannel("familiar.preferences");
  const refresh = () => void reload().catch(e => notify("Couldn't read preferences: " + e, true));
  channel.onmessage = refresh;
  await listen("preferences_changed", refresh);
  await reload();
}

export function choose(changes: Partial<Preferences>) {
  ++revision;
  setPreferences(changes);
  const patch = Object.fromEntries(Object.entries(changes).map(([k, v]) => [k, v ?? null]));
  pending = pending.then(async () => {
    await invoke("save_preferences", { changes: patch, import: false });
    channel.postMessage(null);
  }).catch(e => {
    notify("Couldn't save preferences: " + e, true);
    void reload().catch(console.warn);
  });
}

export const flushPreferences = () => pending;
