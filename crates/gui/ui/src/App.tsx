// The three-pane window: folders, message list, reading pane, and a status bar.
// Each pane is its own component folder; they share selection through state.ts.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { registerBuiltinActions } from "./actions/builtin";
import { onGlobalKey, setKeyOverrides } from "./actions/registry";
import { type DaemonState, type UiInfo, watchDaemon } from "./api";
import { ReadingPane } from "./reading/ReadingPane";
import { Palette } from "./palette/Palette";
import { EventDialog } from "./calendar/EventDialog";
import { aiSettings, eventDialog, movePicker, notice, processedFolders, setEventDialog, setFollowUpView, setMovePicker, setSearchQuery, setSelectedFolder, setSelectedMessage, setSpamDialog, spamDialog, spamSettings } from "./state";
import { SpamDialog } from "./spam/SpamDialog";
import { FolderTree } from "./folders/FolderTree";
import { MessageList } from "./list/MessageList";
import { MovePicker } from "./list/MovePicker";
import { Splitters, paneColumns } from "./Splitters";
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
  // config.toml changed since the app started: offer a restart.
  const [configChanged, setConfigChanged] = createSignal(false);
  const [palette, setPalette] = createSignal<{ initial?: string } | null>(null);
  setKeyOverrides(props.info.keys);
  Object.assign(processedFolders, props.info.processed);
  aiSettings.tool = props.info.ai_tool;
  aiSettings.accounts = props.info.ai_accounts;
  spamSettings.accounts = props.info.spam_accounts;
  registerBuiltinActions((initial) => setPalette({ initial }));
  onMount(async () => {
    window.addEventListener("keydown", onGlobalKey);
    const stopDaemon = await watchDaemon(setDaemon);
    const stopUi = await listen<UiCommand>("ui_command", (e) => {
      const c = e.payload;
      setPalette(null);
      setEventDialog(null);
      setSpamDialog(null);
      setSearchQuery(null);
      setFollowUpView(false);
      setSelectedFolder(c.folder);
      setSelectedMessage(c.kind === "open" ? c.message : null);
    });
    const stopConfig = await listen("config_changed", () => setConfigChanged(true));
    onCleanup(() => {
      window.removeEventListener("keydown", onGlobalKey);
      stopDaemon();
      stopUi();
      stopConfig();
    });
  });
  return (
    <div class="app">
      <TitleBar platform={props.info.platform} />
      <main class="panes" style={{ "grid-template-columns": paneColumns() }}>
        <Splitters />
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
        <Show when={configChanged()}>
          <span>
            config.toml changed.{" "}
            <button class="statusbar-button" onClick={() => void invoke("restart_app")}>
              Restart
            </button>
          </span>
        </Show>
        <Show when={notice()}>{(n) => <span class="statusbar-notice" classList={{ warn: n().error }}>{n().text}</span>}</Show>
        <span class="statusbar-spacer" />
        <span>Familiar {props.info.version}</span>
      </footer>
      <Show when={eventDialog()}>
        {(d) => <EventDialog message={d().message} from={d().from} onClose={() => setEventDialog(null)} />}
      </Show>
      <Show when={spamDialog()}>
        {(d) => <SpamDialog message={d().message} saved={d().saved} onClose={() => setSpamDialog(null)} />}
      </Show>
      <Show when={movePicker()}>
        {(m) => <MovePicker ids={m().ids} accounts={m().accounts} onClose={() => setMovePicker(null)} />}
      </Show>
      <Show when={palette()}>
        {(p) => <Palette initial={p().initial} onClose={() => setPalette(null)} />}
      </Show>
    </div>
  );
}
