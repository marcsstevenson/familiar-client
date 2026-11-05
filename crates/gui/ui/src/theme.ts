// Light, dark or follow the OS. config.toml's `[app] theme` is the default;
// choosing one in the app lasts until it is restarted with a different config.
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

export function cycleTheme(): void {
  setTheme(ORDER[(ORDER.indexOf(theme()) + 1) % ORDER.length] ?? "system");
}
