-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 4: calendars and events (WP 1.11).
--
-- Events are kept for a rolling window around today, synced per calendar
-- by calendarView delta. Recurring series arrive expanded by the server:
-- each occurrence and exception is its own row; series masters are not
-- stored. Times are Unix seconds, UTC. An all-day event's start and end
-- are midnight UTC of its first day and of the day after its last, and
-- stand for those dates wherever you are.

CREATE TABLE calendars (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    remote_id    TEXT NOT NULL,
    name         TEXT NOT NULL,
    color        TEXT,
    is_default   INTEGER NOT NULL DEFAULT 0,
    can_edit     INTEGER NOT NULL DEFAULT 0,
    owner_name   TEXT NOT NULL DEFAULT '',
    owner_addr   TEXT NOT NULL DEFAULT '',
    -- The window the current delta link (or round in progress) covers.
    window_start INTEGER,
    window_end   INTEGER,
    -- Bumped when a full round starts; rows not seen in it are removed at its end.
    round        INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, remote_id)
);

CREATE TABLE events (
    id               INTEGER PRIMARY KEY,
    account_id       INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    calendar_id      INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    remote_id        TEXT NOT NULL,
    ical_uid         TEXT,
    series_master_id TEXT,
    -- singleInstance, occurrence or exception.
    kind             TEXT NOT NULL DEFAULT 'singleInstance',
    subject          TEXT NOT NULL DEFAULT '',
    location         TEXT NOT NULL DEFAULT '',
    start_at         INTEGER NOT NULL,
    end_at           INTEGER NOT NULL,
    is_all_day       INTEGER NOT NULL DEFAULT 0,
    -- IANA zones the organiser used, when Graph says.
    start_tz         TEXT,
    end_tz           TEXT,
    show_as          TEXT NOT NULL DEFAULT 'busy',
    is_cancelled     INTEGER NOT NULL DEFAULT 0,
    is_organizer     INTEGER NOT NULL DEFAULT 0,
    organizer_name   TEXT NOT NULL DEFAULT '',
    organizer_addr   TEXT NOT NULL DEFAULT '',
    -- My response: none, organizer, accepted, tentativelyAccepted, declined, notResponded.
    response         TEXT,
    attendees_json   TEXT NOT NULL DEFAULT '[]',
    categories_json  TEXT NOT NULL DEFAULT '[]',
    preview          TEXT NOT NULL DEFAULT '',
    web_link         TEXT,
    reminder_minutes INTEGER,
    change_key       TEXT,
    round            INTEGER NOT NULL DEFAULT 0,
    UNIQUE (calendar_id, remote_id)
);
CREATE INDEX events_time ON events(calendar_id, start_at);
CREATE INDEX events_account_time ON events(account_id, start_at);
