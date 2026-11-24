import { createSignal } from "solid-js";
import type { Calendar } from "./proposals";
import { views, type CalendarView } from "./timeline";

const key = "familiar.calendar-view";
type Preferences = { view: CalendarView; hidden: number[] };

function read(): Preferences {
  try {
    const saved = JSON.parse(localStorage.getItem(key) ?? "{}");
    return {
      view: views.includes(saved.view) ? saved.view : "Week",
      hidden: Array.isArray(saved.hidden) ? saved.hidden.filter(Number.isSafeInteger) : [],
    };
  } catch { return { view: "Week", hidden: [] }; }
}

const [preferences, setPreferences] = createSignal(read());
export const calendarView = () => preferences().view;
export const calendarVisible = (id: number) => !preferences().hidden.includes(id);
export const writableCalendars = (calendars: Calendar[]) => calendars.filter(c => c.can_edit && calendarVisible(c.id));

function save(next: Preferences) {
  setPreferences(next);
  try { localStorage.setItem(key, JSON.stringify(next)); }
  catch { /* Keep the current choices in memory. */ }
}

export function chooseView(view: CalendarView) { save({ ...preferences(), view }); }

export function setCalendarVisible(id: number, visible: boolean) {
  const current = preferences();
  const hidden = visible ? current.hidden.filter(other => other !== id) : [...current.hidden, id];
  save({ ...current, hidden });
}
