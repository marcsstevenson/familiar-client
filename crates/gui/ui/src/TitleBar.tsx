// The app's own title bar. On Windows and Linux the app draws the window
// buttons; on macOS the system's traffic lights sit over the left end.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type JSX, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { type Theme, isCommandError } from "./api";
import keaUrl from "./assets/kea.svg";
import { openCompose } from "./compose/spec";
import { notify } from "./state";
import { DEFAULT_ACCENT, accent, chooseAccent, chooseTheme, theme } from "./theme";

/** Opens config.toml in the default editor; errors go to the status bar. */
function openConfig() {
  invoke<string>("open_config").catch((e) => notify(isCommandError(e) ? e.message : String(e), true));
}

/** `title` names a compose window; without it this is the main window's bar. */
export function TitleBar(props: { platform: string; title?: string; navigation?: JSX.Element; children?: JSX.Element }) {
  let appearance!: HTMLDialogElement;
  const win = getCurrentWindow();
  const mac = () => props.platform === "macos";
  return (
    <header class="titlebar" classList={{ mac: mac() }} data-tauri-drag-region>
      <img class="titlebar-icon" src={keaUrl} alt="" data-tauri-drag-region />
      <span class="titlebar-name" data-tauri-drag-region>
        {props.title ?? "Familiar"}
      </span>
      {props.navigation}
      <Show when={props.title === undefined}>
        <button class="titlebar-tool" title="New message (Ctrl+N)" aria-label="New message" onClick={() => void openCompose({ kind: "new" })}>
          ✎
        </button>
        <button class="titlebar-tool" title="Open config.toml" aria-label="Open config.toml" onClick={openConfig}>
          ⚙
        </button>
      </Show>
      <div class="titlebar-spacer" data-tauri-drag-region />
      {props.children}
      <button
        class="titlebar-tool"
        title="Appearance"
        aria-label="Appearance"
        aria-haspopup="dialog"
        onClick={() => appearance.showModal()}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4" />
          <path d="M8 2a6 6 0 0 1 0 12Z" fill="currentColor" />
        </svg>
      </button>
      <dialog ref={appearance} class="appearance-dialog" aria-labelledby="appearance-title"
        onKeyDown={e => e.stopPropagation()}>
        <h2 id="appearance-title">Appearance</h2>
        <div class="appearance-row">
          <label for="appearance-theme">Theme</label>
          <select id="appearance-theme" value={theme()} onChange={e => chooseTheme(e.currentTarget.value as Theme)}>
            <option value="system">Use system setting</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </div>
        <div class="appearance-row">
          <label for="appearance-accent">Accent colour</label>
          <input id="appearance-accent" type="color" value={accent() ?? DEFAULT_ACCENT}
            onInput={e => chooseAccent(e.currentTarget.value)} />
        </div>
        <footer>
          <button type="button" disabled={!accent()} onClick={() => chooseAccent(null)}>Reset accent</button>
          <button type="button" class="primary" onClick={() => appearance.close()}>Done</button>
        </footer>
      </dialog>
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
