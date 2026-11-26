// Light, dark or follow the OS. config.toml's `[app] theme` is the default;
// choosing one in the app lasts until the app is restarted. The choice is
// kept in localStorage, which every window of the app shares, so compose
// windows open in the same theme and follow later changes.
import { createSignal } from "solid-js";
import { luminance } from "./colour";
import type { Theme } from "./api";

const ACCENT_KEY = "familiar.accent";
export const DEFAULT_ACCENT = "#1f5fc7";
const [accent, setAccentSignal] = createSignal<string | null>(null);
const NEON_KEY = "familiar.neon";
const [neon, setNeon] = createSignal(false);
export { accent, neon };

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function storedAccent(): string | null {
  const value = stored(ACCENT_KEY);
  return value && /^#[0-9a-f]{6}$/i.test(value) ? value : null;
}

export function chooseNeon(enabled: boolean): void {
  setNeon(enabled);
  apply();
  try {
    if (enabled) localStorage.setItem(NEON_KEY, "true");
    else localStorage.removeItem(NEON_KEY);
  } catch {
    // The preference still applies in this window.
  }
}

// Move the chosen colour towards white or black just far enough to keep
// accent text readable on the selected row (the strongest neutral surface).
export function applyAccent(root: HTMLElement, value: string | null, dark: boolean): void {
  if (!value) {
    root.style.removeProperty("--accent-source");
    root.style.removeProperty("--accent");
    return;
  }
  root.style.setProperty("--accent-source", value);
  const rgb = [1, 3, 5].map(i => parseInt(value.slice(i, i + 2), 16));
  const end = dark ? 255 : 0;
  for (let step = 0; step <= 100; step++) {
    const channels = rgb.map(n => Math.round(n + (end - n) * step / 100));
    const light = luminance(channels[0]!, channels[1]!, channels[2]!);
    if (dark ? light >= 0.38 : light <= 0.12) {
      root.style.setProperty("--accent", `rgb(${channels.join(", ")})`);
      return;
    }
  }
}

export function chooseAccent(value: string | null): void {
  setAccentSignal(value);
  apply();
  try {
    if (value) localStorage.setItem(ACCENT_KEY, value);
    else localStorage.removeItem(ACCENT_KEY);
  } catch {
    // Storage may be unavailable; the current window still updates.
  }
}

const media = window.matchMedia("(prefers-color-scheme: dark)");
const [theme, setThemeSignal] = createSignal<Theme>("system");
const [systemDark, setSystemDark] = createSignal(media.matches);
media.addEventListener("change", (e) => {
  setSystemDark(e.matches);
  apply();
});

export { theme };

/** The theme actually showing. */
export const resolvedTheme = (): "light" | "dark" =>
  theme() === "system" ? (systemDark() ? "dark" : "light") : (theme() as "light" | "dark");

// The page always carries the resolved theme, so tokens.css has exactly one
// light and one dark set.
function apply(): void {
  document.documentElement.dataset.theme = resolvedTheme();
  document.documentElement.toggleAttribute("data-neon", neon());
  applyAccent(document.documentElement, accent(), resolvedTheme() === "dark");
}

export function setTheme(t: Theme): void {
  setThemeSignal(t);
  apply();
}

const ORDER: Theme[] = ["system", "light", "dark"];
const KEY = "familiar.theme";

function storedChoice(): Theme | null {
  const t = stored(KEY) as Theme | null;
  return t && ORDER.includes(t) ? t : null;
}

/**
 * Starts from the config's theme. The main window forgets an earlier run's
 * choice; a compose window takes the main window's current one.
 */
export function initTheme(config: Theme, main: boolean): void {
  if (main) {
    try {
      localStorage.removeItem(KEY);
    } catch {
      // No storage: each window just follows the config.
    }
  }
  setAccentSignal(storedAccent());
  setNeon(stored(NEON_KEY) === "true");
  setTheme(storedChoice() ?? config);
  window.addEventListener("storage", (e) => {
    if (e.key === ACCENT_KEY || e.key === null) {
      setAccentSignal(storedAccent());
      apply();
    }
    if (e.key === NEON_KEY || e.key === null) {
      setNeon(stored(NEON_KEY) === "true");
      apply();
    }
    if (e.key === KEY) setTheme(storedChoice() ?? config);
  });
}

export function cycleTheme(): void {
  const next = ORDER[(ORDER.indexOf(theme()) + 1) % ORDER.length] ?? "system";
  chooseTheme(next);
}

export function chooseTheme(next: Theme): void {
  setTheme(next);
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // Not shared with other windows, but this one still changes.
  }
}
