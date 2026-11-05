// App actions: things a key or the palette can do. Single-key bindings have
// defaults here and can be changed in config.toml's [keys] table, by action ID.
import { createSignal } from "solid-js";

export type Action = {
  /** The ID used in [keys], such as `archive`. */
  id: string;
  title: string;
  /** Default key: a single key such as `e`, or a chord such as `Ctrl+K`. */
  key?: string;
  /** The CLI command that does the same, for the current selection. */
  cli?: () => string | undefined;
  /** False when the action can't run now (for example, nothing selected). */
  enabled?: () => boolean;
  run: () => void | Promise<void>;
};

const actions = new Map<string, Action>();
const [overrides, setOverrides] = createSignal<Record<string, string>>({});
const [version, setVersion] = createSignal(0);

export function registerAction(a: Action): void {
  actions.set(a.id, a);
  setVersion((v) => v + 1);
}

/** Applies config.toml's [keys]. */
export function setKeyOverrides(keys: Record<string, string>): void {
  setOverrides(keys);
}

export function allActions(): Action[] {
  version();
  return [...actions.values()];
}

/** The key an action is bound to now. */
export function keyOf(a: Action): string | undefined {
  return overrides()[a.id] ?? a.key;
}

/** Normalises a key event to the form used in bindings: `Ctrl+K`, `e`, `#`. */
export function keyName(e: KeyboardEvent): string {
  const mods: string[] = [];
  if (e.ctrlKey || e.metaKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  // Shift is part of a printed character (`#`, `A`), so only named keys keep it.
  if (e.shiftKey && e.key.length > 1) mods.push("Shift");
  const base = mods.length > 0 && e.key.length === 1 ? e.key.toUpperCase() : e.key;
  return [...mods, base].join("+");
}

/** True while typing, where single keys must type rather than act. */
function typing(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.tagName === "SELECT"
  );
}

/** The window's key handler: runs the action bound to the key, if any. */
export function onGlobalKey(e: KeyboardEvent): void {
  // The palette and other dialogs handle their own keys.
  if (e.defaultPrevented || document.querySelector("[role=dialog]")) return;
  const name = keyName(e);
  const chord = name.includes("+");
  if (!chord && typing(e.target)) return;
  const bound = new Map<string, Action>();
  for (const a of allActions()) {
    const k = keyOf(a);
    if (k) bound.set(normalise(k), a);
  }
  const a = bound.get(normalise(name));
  if (!a || (a.enabled && !a.enabled())) return;
  e.preventDefault();
  void a.run();
}

/** Bindings compare without case for chords (`ctrl+k` = `Ctrl+K`) but keep it for single keys. */
function normalise(k: string): string {
  return k.includes("+") && k.length > 1 ? k.toLowerCase() : k;
}
