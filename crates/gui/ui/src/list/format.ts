// Short dates for the list: the time today, the day and month this year,
// otherwise the full date.
export function shortDate(unixSeconds: number, now = new Date()): string {
  const d = new Date(unixSeconds * 1000);
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  }
  if (d.getFullYear() === now.getFullYear()) {
    return d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
  }
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** When a snoozed message comes back, short: the time today, otherwise the
 * weekday and time this week, otherwise the date and time. */
export function snoozeLabel(unixSeconds: number, now = new Date()): string {
  const d = new Date(unixSeconds * 1000);
  const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return time;
  const days = (d.getTime() - now.getTime()) / 86_400_000;
  if (days < 6) return `${d.toLocaleDateString(undefined, { weekday: "short" })} ${time}`;
  return `${d.toLocaleDateString(undefined, { day: "numeric", month: "short" })} ${time}`;
}

/** The snooze menu's ready-made times, as `fam mail snooze --until` reads
 * them, with when each one is for people. */
export function snoozeChoices(now = new Date()): { label: string; until: string; at: string }[] {
  const at = (days: number, hour: number) => {
    const d = new Date(now);
    d.setDate(d.getDate() + days);
    d.setHours(hour, 0, 0, 0);
    return snoozeLabel(d.getTime() / 1000, now);
  };
  const ahead = (weekday: number) => ((weekday - now.getDay() + 7) % 7) || 7;
  const out = [];
  out.push({ label: "In 3 hours", until: "3h", at: snoozeLabel(now.getTime() / 1000 + 3 * 3600, now) });
  if (now.getHours() < 17) out.push({ label: "This evening", until: "tonight", at: at(0, 18) });
  out.push({ label: "Tomorrow", until: "tomorrow", at: at(1, 9) });
  // Saturday, unless it is already the weekend.
  if (now.getDay() >= 1 && now.getDay() <= 4) {
    out.push({ label: "This weekend", until: "weekend", at: at(ahead(6), 9) });
  }
  out.push({ label: "Next week", until: "next week", at: at(ahead(1), 9) });
  return out;
}
