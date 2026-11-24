// The three-pane window: folders, message list, reading pane, and a status bar.
// Each pane is its own component folder; they share selection through state.ts.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { For, Show, batch, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { registerBuiltinActions } from "./actions/builtin";
import { onGlobalKey, setKeyOverrides } from "./actions/registry";
import { type DaemonState, type UiInfo, watchDaemon } from "./api";
import { DayPanel, type CalendarLocation } from "./calendar/DayPanel";
import { localDate } from "./calendar/timeline";
import { Contacts } from "./contacts/Contacts";
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
  | { kind: "event"; calendar: number; event: number; start_at: number; all_day: boolean }
  | { kind: "open"; folder: number; message: number }
  | { kind: "folder"; folder: number | null };

const mainViews = ["Mail", "Calendar", "Contacts"] as const;
type MainView = typeof mainViews[number];

export function App(props: { info: UiInfo }) {
  const saved = readSession<{ folder: number | null; message: number | null; query: string | null;
    followUp: boolean; view?: MainView; dayPanelOpen?: boolean; calendar?: "mail" | "panel" | "full" }>("navigation");
  setSelectedFolder(saved?.folder ?? null);
  setSelectedMessage(saved?.message ?? null);
  setSearchQuery(saved?.query ?? null);
  setFollowUpView(saved?.followUp ?? false);
  const [view, setView] = createSignal<MainView>(saved?.view ?? (saved?.calendar === "full" ? "Calendar" : "Mail"));
  const [dayPanelOpen, setDayPanelOpen] = createSignal(saved?.dayPanelOpen ?? saved?.calendar === "panel");
  const calendarMode = (): keyof typeof locations | undefined =>
    view() === "Calendar" ? "calendar" : view() === "Mail" && dayPanelOpen() && !eventDialog() ? "dayPanel" : undefined;
  const locations = {
    calendar: readSession<CalendarLocation>("calendar"),
    dayPanel: readSession<CalendarLocation>(saved?.calendar === "panel" ? "calendar" : "dayPanel"),
  };
  const [eventTarget, setEventTarget] = createSignal<{ calendar: number; event: number; date: string }>();
  createEffect(() => writeSession("navigation", {
    folder: selectedFolder(), message: selectedMessage(), query: searchQuery(), followUp: followUpView(),
    view: view(), dayPanelOpen: dayPanelOpen(),
  }));
  let dayPanelToggle!: HTMLButtonElement;
  let viewTabs!: HTMLDivElement;
  function expandDayPanel() {
    locations.calendar = locations.dayPanel;
    if (locations.dayPanel?.event !== undefined) {
      locations.dayPanel = { date: locations.dayPanel.date };
      writeSession("dayPanel", locations.dayPanel);
    }
    setView("Calendar");
  }
  function toggleDayPanel() {
    setDayPanelOpen(open => !open);
    if (!dayPanelOpen()) {
      locations.dayPanel = undefined;
      writeSession("dayPanel", undefined);
      writeSession("dayPanelScroll", undefined);
      dayPanelToggle.focus();
    }
  }
  function onTabKey(event: KeyboardEvent) {
    const tabs = Array.from(viewTabs.querySelectorAll<HTMLButtonElement>("button:enabled"));
    const index = tabs.indexOf(event.target as HTMLButtonElement);
    let next;
    switch (event.key) {
      case "ArrowLeft": next = (index + tabs.length - 1) % tabs.length; break;
      case "ArrowRight": next = (index + 1) % tabs.length; break;
      case "Home": next = 0; break;
      case "End": next = tabs.length - 1; break;
      default: return;
    }
    event.preventDefault();
    tabs[next]?.focus();
    tabs[next]?.click();
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
  registerBuiltinActions((initial) => setPalette({ initial }), toggleDayPanel);
  onMount(async () => {
    window.addEventListener("keydown", onGlobalKey);
    const stopDaemon = await watchDaemon(setDaemon);
    const stopUi = await listen<UiCommand>("ui_command", (e) => {
      const c = e.payload;
      setPalette(null);
      setEventDialog(null);
      setSpamDialog(null);
      setConfirmDialog(null);
      if (c.kind === "event") {
        const start = new Date(c.start_at * 1000);
        batch(() => {
          setView("Calendar");
          setEventTarget({ calendar: c.calendar, event: c.event, date: c.all_day ? start.toISOString().slice(0, 10) : localDate(start) });
        });
        return;
      }
      setDayPanelOpen(false);
      setView("Mail");
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
      <TitleBar platform={props.info.platform} navigation={
        <div ref={viewTabs} class="view-tabs" role="tablist" aria-label="Main view" onKeyDown={onTabKey}>
          <For each={mainViews}>{name =>
            <button id={"tab-" + name} role="tab" aria-selected={view() === name} aria-controls="main-view"
              tabIndex={view() === name ? 0 : -1} disabled={name !== view() && !!eventDialog()}
              onClick={() => setView(name)}>{name}</button>
          }</For>
        </div>
      }>
        <Show when={view() === "Mail"}><button ref={dayPanelToggle} class="titlebar-tool calendar-toggle"
          title={dayPanelOpen() ? "Hide day panel" : "Show day panel"}
          aria-expanded={dayPanelOpen() && !eventDialog()} disabled={!!eventDialog()} onClick={toggleDayPanel}>
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
            <rect x="3.5" y="5.5" width="17" height="15.5" rx="2.5" />
            <path d="M14.5 5.5v15.5" />
          </svg>
        </button></Show>
      </TitleBar>
      <main class="main-view"><div class="panes" id="main-view" role="tabpanel" aria-labelledby={"tab-" + view()}
        classList={{ "reviewing-proposal": !!eventDialog(), "calendar-expanded": view() === "Calendar", "contacts-expanded": view() === "Contacts" }} style={{ "grid-template-columns": paneColumns() }}>
        <Splitters />
        <nav class="pane folders" aria-label="Folders">
          <FolderTree views={props.info.views} />
        </nav>
        <section class="pane list" aria-label="Messages">
          <MessageList />
        </section>
        <div class="reader-calendar">
          <ReadingPane />
          <Show when={calendarMode()} keyed>{mode => <DayPanel full={mode === "calendar"} location={locations[mode]}
            remember={location => { locations[mode] = location; writeSession(mode, location); }}
            expand={expandDayPanel}
            onClose={() => {
              if (mode === "dayPanel") toggleDayPanel();
              else { setView("Mail"); viewTabs.querySelector<HTMLButtonElement>("#tab-Mail")?.focus(); }
            }}
            target={mode === "calendar" ? eventTarget() : undefined} onTargetOpened={() => setEventTarget(undefined)} />}</Show>
          <Show when={eventDialog()} keyed>
            {d => <EventDialog message={d.message} from={d.from} onClose={() => setEventDialog(null)} />}
          </Show>
        </div>
        <Show when={view() === "Contacts"}><Contacts /></Show>
      </div></main>
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
