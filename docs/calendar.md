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
  is its own event. Series masters are not stored.

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
synced window, or a calendar that hasn't synced yet, adds a note.

```text
$ fam cal list --calendar family --days 14
Thu 5 Nov 2026
  17:00-18:00  Junior practice @ Club grounds

Thu 12 Nov 2026
  17:00-18:00  Junior practice @ Club grounds

Times in Pacific/Auckland.
```
