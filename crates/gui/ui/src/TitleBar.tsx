// The app's own title bar. On Windows and Linux the app draws the window
// buttons; on macOS the system's traffic lights sit over the left end.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type JSX, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { isCommandError } from "./api";
import { iconUrl } from "./icon";
import { resolvedTheme } from "./theme";
import { openCompose } from "./compose/spec";
import { notify } from "./state";
import { Appearance } from "./Appearance";

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
      <img class="titlebar-icon" src={iconUrl(resolvedTheme() === "light" ? "transparent" : "contrast")} alt="" data-tauri-drag-region />
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
      <Appearance ref={el => { appearance = el; }} />
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
