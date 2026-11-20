// The three-pane window: folders, message list, reading pane, and a status bar.
// Each pane is its own component folder; they share selection through state.ts.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { registerBuiltinActions } from "./actions/builtin";
import { onGlobalKey, setKeyOverrides } from "./actions/registry";
import { type DaemonState, type UiInfo, watchDaemon } from "./api";
import { DayPanel } from "./calendar/DayPanel";
import { ReadingPane } from "./reading/ReadingPane";
import { Palette } from "./palette/Palette";
import { EventDialog } from "./calendar/EventDialog";
import { aiSettings, confirmDialog, eventDialog, movePicker, notice, processedFolders, setConfirmDialog, setEventDialog, setFollowUpView, setMovePicker, setSearchQuery, setSelectedFolder, setSelectedMessage, setSpamDialog, spamDialog, spamSettings } from "./state";
import { SpamDialog } from "./spam/SpamDialog";
import { ConfirmDialog } from "./ConfirmDialog";
import { FolderTree } from "./folders/FolderTree";
import { MessageList } from "./list/MessageList";
import { MovePicker } from "./list/MovePicker";
import { Splitters, paneColumns } from "./Splitters";
import { TitleBar } from "./TitleBar";

import { SyncStatus } from "./SyncStatus";
import { followUpView, notify, searchQuery, selectedFolder, selectedMessage } from "./state";
import { flushEditorWrites, readSession, writeSession } from "./session";

/** What `fam ui` asks the window to show (crates/gui/src/ui_socket.rs). */
type UiCommand =
  | { kind: "open"; folder: number; message: number }
  | { kind: "folder"; folder: number | null };

export function App(props: { info: UiInfo }) {
  const saved = readSession<{ folder: number | null; message: number | null; query: string | null;
    followUp: boolean; calendar: "mail" | "panel" | "full" }>("navigation");
  setSelectedFolder(saved?.folder ?? null);
  setSelectedMessage(saved?.message ?? null);
  setSearchQuery(saved?.query ?? null);
  setFollowUpView(saved?.followUp ?? false);
  const [calendarOpen, setCalendarOpen] = createSignal(saved?.calendar === "panel" || saved?.calendar === "full");
  const [fullCalendar, setFullCalendar] = createSignal(saved?.calendar === "full");
  createEffect(() => writeSession("navigation", {
    folder: selectedFolder(), message: selectedMessage(), query: searchQuery(), followUp: followUpView(),
    calendar: fullCalendar() ? "full" : calendarOpen() ? "panel" : "mail",
  }));
  let calendarToggle!: HTMLButtonElement;
  function toggleCalendar() {
    setFullCalendar(false);
    setCalendarOpen(!calendarOpen());
    if (!calendarOpen()) {
      writeSession("calendar", undefined);
      writeSession("calendarScroll", undefined);
      calendarToggle.focus();
    }
  }
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
  registerBuiltinActions((initial) => setPalette({ initial }), toggleCalendar);
  onMount(async () => {
    window.addEventListener("keydown", onGlobalKey);
    const stopDaemon = await watchDaemon(setDaemon);
    const stopUi = await listen<UiCommand>("ui_command", (e) => {
      const c = e.payload;
      setCalendarOpen(false);
      setFullCalendar(false);
      setPalette(null);
      setEventDialog(null);
      setSpamDialog(null);
      setConfirmDialog(null);
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
      <TitleBar platform={props.info.platform}>
        <button ref={calendarToggle} class="titlebar-tool calendar-toggle" title="Calendar" aria-label="Calendar"
          aria-expanded={calendarOpen()} onClick={toggleCalendar}>
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
            <rect x="3.5" y="5.5" width="17" height="15.5" rx="2.5" />
            <path d="M7.5 3v5m9-5v5m-13 2.5h17" />
          </svg>
        </button>
      </TitleBar>
      <main class="panes" classList={{ "reviewing-proposal": !!eventDialog(), "calendar-expanded": fullCalendar() }} style={{ "grid-template-columns": paneColumns() }}>
        <Splitters />
        <nav class="pane folders" aria-label="Folders">
          <FolderTree views={props.info.views} />
        </nav>
        <section class="pane list" aria-label="Messages">
          <MessageList />
        </section>
        <div class="reader-calendar">
          <ReadingPane />
          <Show when={calendarOpen() && !eventDialog()}><DayPanel full={fullCalendar()} expand={() => setFullCalendar(true)} onClose={toggleCalendar} /></Show>
          <Show when={eventDialog()} keyed>
            {d => <EventDialog message={d.message} from={d.from} onClose={() => setEventDialog(null)} />}
          </Show>
        </div>
      </main>
      <footer class="statusbar" role="status">
        <SyncStatus daemon={daemon()} />
        <Show when={props.info.config_error}>
          {(e) => <span class="warn">Config: {e()}</span>}
        </Show>
        <Show when={configChanged()}>
          <span>
            config.toml changed.{" "}
            <button class="statusbar-button" onClick={() => void flushEditorWrites().then(() => invoke("restart_app"))
              .catch(e => notify("Couldn't restart: " + e, true))}>
              Restart
            </button>
          </span>
        </Show>
        <Show when={notice()}>{(n) => <span class="statusbar-notice" classList={{ warn: n().error }}>{n().text}</span>}</Show>
        <span class="statusbar-spacer" />
        <span>Familiar {props.info.version}</span>
      </footer>
      <Show when={spamDialog()}>
        {(d) => <SpamDialog message={d().message} saved={d().saved} onClose={() => setSpamDialog(null)} />}
      </Show>
      <Show when={movePicker()}>
        {(m) => <MovePicker ids={m().ids} accounts={m().accounts} onClose={() => setMovePicker(null)} />}
      </Show>
      <Show when={confirmDialog()}>
        {(c) => <ConfirmDialog confirm={c()} onClose={() => setConfirmDialog(null)} />}
      </Show>
      <Show when={palette()}>
        {(p) => <Palette initial={p().initial} onClose={() => setPalette(null)} />}
      </Show>
    </div>
  );
}
