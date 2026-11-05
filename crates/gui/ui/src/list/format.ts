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
