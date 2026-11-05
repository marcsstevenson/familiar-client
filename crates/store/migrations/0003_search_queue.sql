-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 3: the search index queue (WP 1.9).
--
-- The Tantivy index lives beside the store and is maintained by famd. Every
-- change to a message that the index cares about adds a row here, from a
-- trigger, so no writer can forget. famd reads the queue in `seq` order,
-- re-indexes those messages from their current rows (or drops them from the
-- index when the row is gone), commits the index and then deletes the rows
-- it handled, up to the highest `seq` it read. A change made while it works
-- gets a higher `seq` and is picked up next time.
--
-- Read and flag state are left out on purpose: they change often and search
-- filters them in SQLite, not in the index (WP 0.5).

CREATE TABLE search_queue (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL
);

CREATE TRIGGER search_queue_insert AFTER INSERT ON messages BEGIN
    INSERT INTO search_queue (message_id) VALUES (NEW.id);
END;

CREATE TRIGGER search_queue_delete AFTER DELETE ON messages BEGIN
    INSERT INTO search_queue (message_id) VALUES (OLD.id);
END;

CREATE TRIGGER search_queue_update AFTER UPDATE ON messages
WHEN OLD.folder_id IS NOT NEW.folder_id
  OR OLD.subject IS NOT NEW.subject
  OR OLD.from_name IS NOT NEW.from_name
  OR OLD.from_addr IS NOT NEW.from_addr
  OR OLD.to_json IS NOT NEW.to_json
  OR OLD.cc_json IS NOT NEW.cc_json
  OR OLD.received_at IS NOT NEW.received_at
  OR OLD.has_attachments IS NOT NEW.has_attachments
  OR OLD.body_hash IS NOT NEW.body_hash
BEGIN
    INSERT INTO search_queue (message_id) VALUES (NEW.id);
END;

-- is:unread and is:flagged are answered from these small partial indexes and
-- handed to the index as a set of IDs.
CREATE INDEX messages_unread  ON messages(id) WHERE is_read = 0;
CREATE INDEX messages_flagged ON messages(id) WHERE is_flagged = 1;

-- Every message that exists now needs indexing once.
INSERT INTO search_queue (message_id) SELECT id FROM messages;
