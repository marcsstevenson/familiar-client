// Apply the configured colours in every Familiar window.
import { createEffect, createSignal } from "solid-js";
import { luminance } from "./colour";
import type { Theme } from "./api";
import { choose, preferences } from "./preferences";

export const DEFAULT_ACCENT = "#1f5fc7";
export const accent = () => {
  const colour = preferences().accent;
  return colour?.toLowerCase() === DEFAULT_ACCENT ? null : colour ?? null;
};
export const theme = () => preferences().theme;
export const neon = () => preferences().neon ?? false;
export const chooseAccent = (value: string | null) => choose({ accent: value ?? undefined });
export const chooseTheme = (theme: Theme) => choose({ theme });
export const chooseNeon = (neon: boolean) => choose({ neon });

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

const media = window.matchMedia("(prefers-color-scheme: dark)");
const [systemDark, setSystemDark] = createSignal(media.matches);
media.addEventListener("change", (e) => {
  setSystemDark(e.matches);
});

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

export function initTheme() {
  createEffect(apply);
}

const ORDER: Theme[] = ["system", "light", "dark"];
export function cycleTheme(): void {
  chooseTheme(ORDER[(ORDER.indexOf(theme()) + 1) % ORDER.length] ?? "system");
}
