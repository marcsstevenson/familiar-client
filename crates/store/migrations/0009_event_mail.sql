-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Calendar sync replaces occurrences as its date window moves. Keep the
-- source against the provider's event or series ID, independent of that cache.
CREATE TABLE event_mail (
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    remote_id TEXT NOT NULL,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    PRIMARY KEY (calendar_id, remote_id)
);
CREATE INDEX event_mail_message ON event_mail(message_id);
CREATE INDEX events_series_start ON events(calendar_id, coalesce(series_master_id, remote_id), start_at);

-- A queued create starts with a local ID. Single events and Google series
-- receive their provider identity in place. Graph expands series separately.
CREATE TRIGGER event_mail_confirm AFTER UPDATE OF remote_id ON events
WHEN OLD.remote_id LIKE 'local:%' AND NEW.remote_id <> OLD.remote_id
BEGIN
    UPDATE event_mail SET remote_id = coalesce(NEW.series_master_id, NEW.remote_id)
      WHERE calendar_id = OLD.calendar_id AND remote_id = OLD.remote_id;
END;
CREATE TRIGGER event_mail_undo AFTER DELETE ON events WHEN OLD.remote_id LIKE 'local:%'
BEGIN
    DELETE FROM event_mail WHERE calendar_id = OLD.calendar_id AND remote_id = OLD.remote_id;
END;
