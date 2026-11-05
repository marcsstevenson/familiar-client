// The app's own title bar. On Windows and Linux the app draws the window
// buttons; on macOS the system's traffic lights sit over the left end.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Show } from "solid-js";
import { openCompose } from "./compose/spec";
import { cycleTheme, theme } from "./theme";

const THEME_LABEL = { system: "Theme: system", light: "Theme: light", dark: "Theme: dark" } as const;

/** `title` names a compose window; without it this is the main window's bar. */
export function TitleBar(props: { platform: string; title?: string }) {
  const win = getCurrentWindow();
  const mac = () => props.platform === "macos";
  return (
    <header class="titlebar" classList={{ mac: mac() }} data-tauri-drag-region>
      <span class="titlebar-name" data-tauri-drag-region>
        {props.title ?? "Familiar"}
      </span>
      <Show when={props.title === undefined}>
        <button class="titlebar-tool" title="New message (Ctrl+N)" aria-label="New message" onClick={() => void openCompose({ kind: "new" })}>
          ✎
        </button>
      </Show>
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
