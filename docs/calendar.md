# Calendar read

WP 1.11. `famd` keeps each account's calendars and their events on this
device; `fam cal list` and `fam cal calendars` read them from the store, so
they work without the daemon and offline.

## What is kept

- Every calendar Graph lists for the account (`/me/calendars`), including
  shared calendars you have added.
- Events from `[calendar] past` before today to `[calendar] future` after it
  (defaults `3m` and `1y`):

  ```toml
  [calendar]
  past = "3m"
  future = "1y"
  ```

- Recurring series as the server expands them: each occurrence and exception
  is its own event. Series masters are not stored. calendarView delta gives
  an occurrence only its ID, type, times and series master, so its subject,
  location, organiser, all-day flag and zone are copied from the master
  (fetched once per series per sync). Any other event that arrives without
  a subject is fetched in full by its ID.

## How it syncs

The engine polls calendars every 5 minutes, before mail folders, so a long
first mail sync doesn't hold them up. For each calendar:

1. `GET /me/calendars/{id}/calendarView/delta?startDateTime=..&endDateTime=..`
   with `Prefer: outlook.timezone="UTC"`. The window is midnight UTC
   `past` days ago to midnight after `future` days from today.
2. The `nextLink` is saved after each page (resource `events:<calendar id>`
   in `sync_state`), the `deltaLink` at the end. Later polls bring only
   changes.
3. Once a day the window moves on. The saved link belongs to the old window,
   so a new *full round* runs; events it doesn't list are removed at its
   end. A 410 also starts a full round.
4. Graph can report changed events just outside the window; they are not
   stored. A calendar that fails (a shared calendar that refuses delta, say)
   records the error and the others carry on; `fam cal calendars` shows it.

Times are stored as Unix seconds, UTC. All-day events are stored as midnight
UTC of their dates and are matched by date, so they show on their own days in
any zone.

## Time zones

Familiar uses IANA zone names (`Pacific/Auckland`) everywhere. Graph and
Outlook often use Windows names (`New Zealand Standard Time`), so
`familiar_core::tz` maps between them with CLDR's `windowsZones.xml`
(generated into `crates/core/src/tz_table.rs` by
`scripts/gen-windows-zones.py`). Reads accept either name; the organiser's
zone (`originalStartTimeZone`) is stored as IANA. Writes (Phase 2) will send
Windows names, which the consumer service handled more reliably in one past
session.

## Commands

```text
fam cal list [--calendar <name or purpose>] [--account <name>]
             [--from today|tomorrow|yesterday|YYYY-MM-DD] [--days N] [--tz <zone>]
fam cal calendars [--account <name>]
```

`--calendar` takes a calendar's name, case-insensitive, or a purpose from
`[calendar.purposes]` (`--calendar family`). `--tz` takes an IANA or Windows
name; the default is this computer's zone. `--json` gives RFC 3339 times in
that zone, and dates for all-day events (end exclusive). A range outside the
synced window, or a calendar that hasn't synced yet, adds a note. In the
text output, all-day events come first each day and a multi-day all-day event
is listed on every day it covers (`(day 2 of 4)`); a timed event that ends on
a later day shows `(+1d)`.

```text
$ fam cal list --calendar family --days 14
Thu 5 Nov 2026
  17:00-18:00  Junior practice @ Club grounds

Thu 12 Nov 2026
  17:00-18:00  Junior practice @ Club grounds

Times in Pacific/Auckland.
```

## Events an email suggests

`fam cal propose --from-mail <id>` (AI events plan, WP AI.1) says which
events an email suggests, without adding anything:

- **A meeting request** comes back as an `invitation` with the meeting's
  local event ID, to answer with `fam cal rsvp`; a cancellation, to remove
  with `fam cal delete`. Adding it again would make a second copy. If the
  meeting hasn't synced to this device yet, there is a note to try again
  in a few minutes and no events.
- **`.ics` attachments** give each of their events: subject, start, end,
  zone (`TZID`, IANA or Windows), location, description, UID and a simple
  repeat (`FREQ`, `INTERVAL`, `BYDAY`, `UNTIL`, `COUNT`). Repeats Familiar
  can't set (`BYMONTHDAY`, `2TU`) give the first date with a note.
- **Otherwise** the subject, date, time and place `fam cal add --from-mail`
  would guess.

Each event's `start` and `end` are in `fam cal add`'s form
(`2026-11-21 18:30`, or a date for all day, the end being the last day) in
its `time_zone`, and `repeat` maps to `--repeat`, `--every`, `--on`,
`--until` and `--count`. `already` is the ID of an event in the account's
calendars with the same UID, or the same subject starting within a minute.
Invitations and attachments are read through famd; without it there is a
note and only the text is used.

### Add to calendar in the app

The reading pane's **Add to calendar** button (Ctrl+Shift+A) runs
`fam cal propose` on the open message and shows what it found:

- Each event can be changed before it is added: subject, date, times, all
  day, zone, location and repeat. The repeat is shown in words with how many
  times it happens; **change** opens the repeat editor. Events already in a
  calendar start unticked.
- One calendar for all of them, picked from every writable calendar on
  every account. It starts on the calendar last used for that sender, else
  the receiving account's main calendar. Under it are the calendar's other
  events on each of those days.
- **Add** runs `fam cal add --from-mail` once per ticked event, and the
  dialog then offers **Undo**, which deletes them again.
- A meeting request shows the meeting with **Accept**, **Tentative** and
  **Decline** (`fam cal rsvp`, with `--no-reply` when "Tell the organiser"
  is unticked) instead.

When `[ai]` is on (see [ai.md](ai.md)), the dialog says which tool the email
may go to while it waits. Closing it then leaves the tool to finish or time
out on its own.

