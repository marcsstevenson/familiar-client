// The compose pane's public face: other panes and the command palette open
// compose windows with `openCompose`, e.g.
//   openCompose({ kind: "reply", message: id, all: true })
import { openCompose } from "./spec";

export { ComposeWindow } from "./ComposeWindow";
export { type ComposeSpec, composeSpec, openCompose } from "./spec";

/** Ctrl+N (Cmd+N) in the main window opens a new message. */
export function installComposeKeys(): void {
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      void openCompose({ kind: "new" });
    }
  });
}
