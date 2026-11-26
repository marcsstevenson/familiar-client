import { choose, preferences } from "../preferences";
import type { Calendar } from "./proposals";
import type { CalendarView } from "./timeline";

export const calendarKey = (c: Calendar) => c.account + ":" + c.remote_id;
export const calendarView = () => preferences().calendar_view ?? "Week";
export const calendarVisible = (c: Calendar) => !preferences().hidden_calendars?.includes(calendarKey(c));
export const writableCalendars = (calendars: Calendar[]) => calendars.filter(c => c.can_edit && calendarVisible(c));
export const chooseView = (calendar_view: CalendarView) => choose({ calendar_view });

export function setCalendarVisible(c: Calendar, visible: boolean) {
  const hidden = new Set(preferences().hidden_calendars);
  if (visible) hidden.delete(calendarKey(c));
  else hidden.add(calendarKey(c));
  choose({ hidden_calendars: [...hidden] });
}
