// Turning `cal propose`'s proposals into editable drafts, describing repeats
// in words, and building the `cal add` input for a confirmed draft. No UI here.
import type { Commands } from "../api";

type ProposeOutput = Commands["cal.propose"]["output"];
export type Proposal = ProposeOutput["events"][number];
export type AddInput = Commands["cal.add"]["input"];
export type Calendar = Commands["cal.calendars"]["output"]["calendars"][number];

export type Every = "none" | "daily" | "weekly" | "monthly" | "yearly";
export const DAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;

/** One event as the dialog edits it. Dates are `YYYY-MM-DD`, times `HH:MM`. */
export type Draft = {
  add: boolean;
  subject: string;
  allDay: boolean;
  startDate: string;
  startTime: string;
  endDate: string;
  endTime: string;
  zone: string;
  location: string;
  notes: string;
  every: Every;
  interval: number;
  on: string[];
  /** How the repeat ends: a date, a number of times, or never. */
  ends: "until" | "count" | "never";
  until: string;
  count: number;
  evidence: string | null;
  already: number | null;
};

const pad = (n: number) => String(n).padStart(2, "0");
const ymd = (d: Date) => `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;

/** A `YYYY-MM-DD` date as a UTC midnight, for date arithmetic only. */
export function day(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return null;
  const d = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
  return Number.isNaN(d.getTime()) ? null : d;
}

function addMinutes(date: string, time: string, minutes: number): [string, string] {
  const d = day(date);
  const [h = NaN, m = NaN] = time.split(":").map(Number);
  if (!d || Number.isNaN(h) || Number.isNaN(m)) return [date, time];
  d.setUTCMinutes(h * 60 + m + minutes);
  return [ymd(d), `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`];
}

/** The draft for a proposal. An event already in the calendar starts unticked. */
export function draftOf(p: Proposal): Draft {
  const [startDate = "", startTime = "09:00"] = p.start.split(" ");
  let endDate = startDate;
  let endTime = "";
  if (p.end) {
    [endDate = startDate, endTime = ""] = p.end.split(" ");
  }
  if (!p.all_day && !endTime) [endDate, endTime] = addMinutes(startDate, startTime, 60);
  const r = p.repeat;
  return {
    add: p.already == null,
    subject: p.subject,
    allDay: p.all_day,
    startDate,
    startTime: p.all_day ? "09:00" : startTime,
    endDate,
    endTime: p.all_day ? "10:00" : endTime,
    zone: p.time_zone,
    location: p.location ?? "",
    notes: p.notes ?? "",
    every: (r?.every as Every | undefined) ?? "none",
    interval: r?.interval ?? 1,
    on: r?.on ?? [],
    ends: r?.until ? "until" : r?.count ? "count" : "never",
    until: r?.until ?? "",
    count: r?.count ?? 10,
    evidence: p.evidence ?? null,
    already: p.already ?? null,
  };
}

/** Why a draft can't be added as it stands, or null. */
export function problem(d: Draft): string | null {
  if (!d.subject.trim()) return "give it a subject";
  if (!day(d.startDate)) return "give it a date";
  const end = day(d.endDate);
  if (!end) return "give it an end date";
  if (d.allDay ? d.endDate < d.startDate : `${d.endDate} ${d.endTime}` <= `${d.startDate} ${d.startTime}`)
    return "it ends before it starts";
  if (d.every !== "none" && d.ends === "until" && (!day(d.until) || d.until < d.startDate))
    return "the repeat ends before it starts";
  return null;
}

/** `cal add`'s input for a draft, in `calendar` of `account`, from email `message`. */
export function addInput(d: Draft, message: number, cal: { account: string; name: string }): AddInput {
  const input: AddInput = {
    from_mail: message,
    account: cal.account,
    calendar: cal.name,
    subject: d.subject.trim(),
    all_day: d.allDay,
    start: d.allDay ? d.startDate : `${d.startDate} ${d.startTime}`,
    end: d.allDay ? d.endDate : `${d.endDate} ${d.endTime}`,
    tz: d.zone,
    location: d.location.trim(),
    body: d.notes.trim() || null,
  };
  if (d.every !== "none") {
    input.repeat = d.every;
    input.every = Math.max(1, d.interval);
    if (d.every === "weekly") input.on = d.on;
    if (d.ends === "until") input.until = d.until;
    if (d.ends === "count") input.count = Math.max(1, d.count);
  }
  return input;
}

const DAY_NAMES: Record<string, string> = {
  mon: "Mon",
  tue: "Tue",
  wed: "Wed",
  thu: "Thu",
  fri: "Fri",
  sat: "Sat",
  sun: "Sun",
};
const UNIT: Record<Exclude<Every, "none">, string> = { daily: "day", weekly: "week", monthly: "month", yearly: "year" };

/** A date in words: "Tue 2 Feb 2027". */
export function dateWords(s: string): string {
  const d = day(s);
  return d
    ? d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short", year: "numeric", timeZone: "UTC" })
    : s;
}

/** How many times a repeat happens, up to 1000; null when it has no end. */
export function occurrences(d: Draft): number | null {
  if (d.every === "none") return 1;
  if (d.ends === "count") return Math.max(1, d.count);
  if (d.ends === "never") return null;
  const start = day(d.startDate);
  const until = day(d.until);
  if (!start || !until || until < start) return 0;
  const n = Math.max(1, d.interval);
  const weekdays = d.every === "weekly" && d.on.length > 0 ? d.on : [DAYS[(start.getUTCDay() + 6) % 7]];
  let count = 0;
  for (let i = 0; count < 1000; i++) {
    if (d.every === "weekly") {
      // The week (Monday first) i*n weeks after the start's week.
      const monday = new Date(start);
      monday.setUTCDate(start.getUTCDate() - ((start.getUTCDay() + 6) % 7) + 7 * n * i);
      if (monday > until) break;
      for (const w of weekdays) {
        const x = new Date(monday);
        x.setUTCDate(monday.getUTCDate() + DAYS.indexOf(w as (typeof DAYS)[number]));
        if (x >= start && x <= until) count++;
      }
      continue;
    }
    const x = new Date(start);
    if (d.every === "daily") x.setUTCDate(start.getUTCDate() + n * i);
    if (d.every === "monthly") x.setUTCMonth(start.getUTCMonth() + n * i);
    if (d.every === "yearly") x.setUTCFullYear(start.getUTCFullYear() + n * i);
    if (x > until) break;
    // A 31st in a shorter month doesn't happen that month.
    if (d.every === "daily" || x.getUTCDate() === start.getUTCDate()) count++;
  }
  return count;
}

/** A repeat in words: "weekly on Tue, until Tue 13 Apr 2027 (11 times)". */
export function repeatWords(d: Draft): string {
  if (d.every === "none") return "Doesn't repeat";
  const n = Math.max(1, d.interval);
  let s = n === 1 ? d.every : `every ${n} ${UNIT[d.every]}s`;
  if (d.every === "weekly" && d.on.length > 0) s += ` on ${d.on.map((x) => DAY_NAMES[x] ?? x).join(", ")}`;
  if (d.ends === "until") s += `, until ${dateWords(d.until)}`;
  const times = occurrences(d);
  if (times === null) return `${s}, with no end date`;
  return `${s} (${times} ${times === 1 ? "time" : "times"})`;
}

/** Where a proposal came from, in words. */
export function sourceWords(source: string): string {
  switch (source) {
    case "ics":
      return "from the attached calendar file";
    case "ai:claude":
      return "found by Claude";
    case "ai:codex":
      return "found by Codex";
    case "heuristic":
      return "Familiar's guess";
    default:
      return "";
  }
}

/** The tool `[ai] tool` names, in words. */
export function toolWords(tool: string | null): string {
  if (tool === "claude") return "Claude";
  if (tool === "codex") return "Codex";
  return "your AI tool";
}

const LAST_KEY = "familiar.event-calendar";

/** The calendar to start with: the one last used for this sender, else the
 * receiving account's main calendar, else the first. */
export function defaultCalendar(cals: Calendar[], account: string, from: string): Calendar | undefined {
  let last: { account: string; name: string } | undefined;
  try {
    last = JSON.parse(localStorage.getItem(LAST_KEY) ?? "{}")[from.toLowerCase()];
  } catch {
    last = undefined;
  }
  return (
    cals.find((c) => last && c.account === last.account && c.name === last.name) ??
    cals.find((c) => c.account === account && c.is_default) ??
    cals.find((c) => c.account === account) ??
    cals[0]
  );
}

/** Remembers the calendar used for this sender. */
export function rememberCalendar(from: string, cal: Calendar): void {
  if (!from) return;
  try {
    const all = JSON.parse(localStorage.getItem(LAST_KEY) ?? "{}");
    all[from.toLowerCase()] = { account: cal.account, name: cal.name };
    localStorage.setItem(LAST_KEY, JSON.stringify(all));
  } catch {
    // Not remembering is fine.
  }
}

export const calendarKey = (c: { account: string; name: string }) => `${c.account}\u0000${c.name}`;
