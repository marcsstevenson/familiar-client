// The app's own title bar. On Windows and Linux the app draws the window
// buttons; on macOS the system's traffic lights sit over the left end.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Show } from "solid-js";
import { cycleTheme, theme } from "./theme";

const THEME_LABEL = { system: "Theme: system", light: "Theme: light", dark: "Theme: dark" } as const;

export function TitleBar(props: { platform: string }) {
  const win = getCurrentWindow();
  const mac = () => props.platform === "macos";
  return (
    <header class="titlebar" classList={{ mac: mac() }} data-tauri-drag-region>
      <span class="titlebar-name" data-tauri-drag-region>
        Familiar
      </span>
      <div class="titlebar-spacer" data-tauri-drag-region />
      <button
        class="titlebar-tool"
        title={THEME_LABEL[theme()]}
        aria-label={THEME_LABEL[theme()]}
        onClick={cycleTheme}
      >
        {theme() === "dark" ? "☾" : theme() === "light" ? "☀" : "◐"}
      </button>
      <Show when={!mac()}>
        <div class="window-buttons">
          <button aria-label="Minimise" onClick={() => void win.minimize()}>
            <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" /></svg>
          </button>
          <button aria-label="Maximise" onClick={() => void win.toggleMaximize()}>
            <svg viewBox="0 0 10 10" aria-hidden="true"><rect x="0.5" y="0.5" width="9" height="9" /></svg>
          </button>
          <button class="close" aria-label="Close" onClick={() => void win.close()}>
            <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" /></svg>
          </button>
        </div>
      </Show>
    </header>
  );
}
