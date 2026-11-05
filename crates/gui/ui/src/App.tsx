// The three-pane window: folders, message list, reading pane, and a status bar.
// Each pane is its own component folder; they share selection through state.ts.
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { type DaemonState, type UiInfo, watchDaemon } from "./api";
import { FolderTree } from "./folders/FolderTree";
import { MessageList } from "./list/MessageList";
import { TitleBar } from "./TitleBar";

function daemonText(s: DaemonState): string {
  switch (s.state) {
    case "starting":
      return "Starting sync…";
    case "running":
      return "Sync running";
    case "unavailable":
      return `Sync not running: ${s.message}`;
  }
}

export function App(props: { info: UiInfo }) {
  const [daemon, setDaemon] = createSignal<DaemonState>({ state: "starting" });
  onMount(async () => {
    const stop = await watchDaemon(setDaemon);
    onCleanup(stop);
  });
  return (
    <div class="app">
      <TitleBar platform={props.info.platform} />
      <main class="panes">
        <nav class="pane folders" aria-label="Folders">
          <FolderTree />
        </nav>
        <section class="pane list" aria-label="Messages">
          <MessageList />
        </section>
        <section class="pane reading" aria-label="Reading pane">
          <div class="pane-empty">Select a message to read it</div>
        </section>
      </main>
      <footer class="statusbar" role="status">
        <span classList={{ warn: daemon().state === "unavailable" }}>{daemonText(daemon())}</span>
        <Show when={props.info.config_error}>
          {(e) => <span class="warn">Config: {e()}</span>}
        </Show>
        <span class="statusbar-spacer" />
        <span>Familiar {props.info.version}</span>
      </footer>
    </div>
  );
}
