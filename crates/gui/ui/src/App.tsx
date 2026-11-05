// The three-pane window: folders, message list, reading pane, and a status bar.
// Each pane is its own component folder; they share selection through state.ts.
import { listen } from "@tauri-apps/api/event";
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { registerBuiltinActions } from "./actions/builtin";
import { onGlobalKey, setKeyOverrides } from "./actions/registry";
import { type DaemonState, type UiInfo, watchDaemon } from "./api";
import { ReadingPane } from "./reading/ReadingPane";
import { Palette } from "./palette/Palette";
import { notice, setSearchQuery, setSelectedFolder, setSelectedMessage } from "./state";
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

/** What `fam ui` asks the window to show (crates/gui/src/ui_socket.rs). */
type UiCommand =
  | { kind: "open"; folder: number; message: number }
  | { kind: "folder"; folder: number | null };

export function App(props: { info: UiInfo }) {
  const [daemon, setDaemon] = createSignal<DaemonState>({ state: "starting" });
  // The palette: closed, open on the list, or open on one command's form.
  const [palette, setPalette] = createSignal<{ initial?: string } | null>(null);
  setKeyOverrides(props.info.keys);
  registerBuiltinActions((initial) => setPalette({ initial }));
  onMount(async () => {
    window.addEventListener("keydown", onGlobalKey);
    const stopDaemon = await watchDaemon(setDaemon);
    const stopUi = await listen<UiCommand>("ui_command", (e) => {
      const c = e.payload;
      setPalette(null);
      setSearchQuery(null);
      setSelectedFolder(c.folder);
      setSelectedMessage(c.kind === "open" ? c.message : null);
    });
    onCleanup(() => {
      window.removeEventListener("keydown", onGlobalKey);
      stopDaemon();
      stopUi();
    });
  });
  return (
    <div class="app">
      <TitleBar platform={props.info.platform} />
      <main class="panes">
        <nav class="pane folders" aria-label="Folders">
          <FolderTree views={props.info.views} />
        </nav>
        <section class="pane list" aria-label="Messages">
          <MessageList />
        </section>
        <ReadingPane />
      </main>
      <footer class="statusbar" role="status">
        <span classList={{ warn: daemon().state === "unavailable" }}>{daemonText(daemon())}</span>
        <Show when={props.info.config_error}>
          {(e) => <span class="warn">Config: {e()}</span>}
        </Show>
        <Show when={notice()}>{(n) => <span classList={{ warn: n().error }}>{n().text}</span>}</Show>
        <span class="statusbar-spacer" />
        <span>Familiar {props.info.version}</span>
      </footer>
      <Show when={palette()}>
        {(p) => <Palette initial={p().initial} onClose={() => setPalette(null)} />}
      </Show>
    </div>
  );
}
