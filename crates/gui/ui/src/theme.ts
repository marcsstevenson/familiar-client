// Light, dark or follow the OS. config.toml's `[app] theme` is the default;
// choosing one in the app lasts until the app is restarted. The choice is
// kept in localStorage, which every window of the app shares, so compose
// windows open in the same theme and follow later changes.
import { createSignal } from "solid-js";
import type { Theme } from "./api";

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
}

export function setTheme(t: Theme): void {
  setThemeSignal(t);
  apply();
}

const ORDER: Theme[] = ["system", "light", "dark"];
const KEY = "familiar.theme";

function storedChoice(): Theme | null {
  try {
    const t = localStorage.getItem(KEY) as Theme | null;
    return t && ORDER.includes(t) ? t : null;
  } catch {
    return null;
  }
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
  setTheme(storedChoice() ?? config);
  window.addEventListener("storage", (e) => {
    if (e.key === KEY) setTheme(storedChoice() ?? config);
  });
}

export function cycleTheme(): void {
  const next = ORDER[(ORDER.indexOf(theme()) + 1) % ORDER.length] ?? "system";
  setTheme(next);
  try {
    localStorage.setItem(KEY, next);
  } catch {
    // Not shared with other windows, but this one still changes.
  }
}
