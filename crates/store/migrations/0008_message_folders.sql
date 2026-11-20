-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 8: a message can sit in more than one folder (WP G.2).
--
-- Gmail labels are folders, and one message can carry several. Every
-- message has one row here per folder it is in. `messages.folder_id` stays
-- as the message's primary location and always has a row here too; a Graph
-- message has exactly that one row, so nothing changes for Graph.
--
-- `thread_id`, `received_at` and `is_read` are copies of the message's own
-- columns, kept equal by the triggers below. With them, folder counters and
-- `folder_threads` are derived from this table alone, and a folder list is
-- an index range scan here, keyset paged on (received_at, message_id) as
-- before.
--
-- How rows get here:
-- - inserting a message adds the row for its `folder_id`;
-- - changing `folder_id` (a move) moves that row;
-- - `WriteTx::set_message_folders` adds and removes the other rows;
-- - deleting a message deletes all its rows;
-- - deleting a folder deletes its rows (and, as before, the messages whose
--   primary location it was).

CREATE TABLE message_folders (
    message_id  INTEGER NOT NULL,
    folder_id   INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    thread_id   INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    is_read     INTEGER NOT NULL,
    PRIMARY KEY (message_id, folder_id)
) WITHOUT ROWID;
-- Folder lists, newest first. is_read is in it so that ordering a whole
-- folder, all or unread only, never leaves the index (a lookup per row into
-- this WITHOUT ROWID table doubled the time at 150k messages).
CREATE INDEX message_folders_time
    ON message_folders(folder_id, received_at DESC, message_id DESC, is_read);
-- Recomputing one conversation's row in one folder.
CREATE INDEX message_folders_thread ON message_folders(thread_id, folder_id);

INSERT INTO message_folders (message_id, folder_id, thread_id, received_at, is_read)
SELECT id, folder_id, thread_id, received_at, is_read FROM messages;

-- The folder side of version 1's triggers moves to message_folders; the
-- thread side stays on messages.
DROP TRIGGER messages_after_insert;
DROP TRIGGER messages_after_delete;
DROP TRIGGER messages_after_update;
-- messages_folder_time stays: lists no longer read it, but the foreign key
-- from messages to folders needs an index on folder_id, or deleting a folder
-- scans every message.

-- messages: keep message_folders in step, and recompute the thread. ---------

CREATE TRIGGER messages_after_insert AFTER INSERT ON messages BEGIN
    INSERT INTO message_folders (message_id, folder_id, thread_id, received_at, is_read)
    VALUES (NEW.id, NEW.folder_id, NEW.thread_id, NEW.received_at, NEW.is_read);

    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = NEW.thread_id)
     WHERE id = NEW.thread_id;
END;

CREATE TRIGGER messages_after_delete AFTER DELETE ON messages BEGIN
    DELETE FROM message_folders WHERE message_id = OLD.id;

    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = OLD.thread_id)
     WHERE id = OLD.thread_id;
    DELETE FROM threads WHERE id = OLD.thread_id AND message_count = 0;
END;

CREATE TRIGGER messages_after_update
AFTER UPDATE OF folder_id, thread_id, is_read, received_at ON messages
WHEN OLD.folder_id IS NOT NEW.folder_id
  OR OLD.thread_id IS NOT NEW.thread_id
  OR OLD.is_read IS NOT NEW.is_read
  OR OLD.received_at IS NOT NEW.received_at
BEGIN
    -- Copy the changed columns to every folder the message is in.
    UPDATE message_folders
       SET thread_id = NEW.thread_id, received_at = NEW.received_at, is_read = NEW.is_read
     WHERE message_id = NEW.id
       AND (thread_id IS NOT NEW.thread_id OR received_at IS NOT NEW.received_at
            OR is_read IS NOT NEW.is_read);

    -- A move: the old primary location's row goes to the new one.
    DELETE FROM message_folders
     WHERE message_id = NEW.id AND folder_id = OLD.folder_id
       AND OLD.folder_id IS NOT NEW.folder_id;
    INSERT OR IGNORE INTO message_folders (message_id, folder_id, thread_id, received_at, is_read)
    SELECT NEW.id, NEW.folder_id, NEW.thread_id, NEW.received_at, NEW.is_read
     WHERE OLD.folder_id IS NOT NEW.folder_id;

    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = OLD.thread_id)
     WHERE id = OLD.thread_id;
    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = NEW.thread_id)
     WHERE id = NEW.thread_id AND NEW.thread_id IS NOT OLD.thread_id;
    DELETE FROM threads WHERE id = OLD.thread_id AND message_count = 0;
END;

-- message_folders: folder counters and folder_threads. ---------------------
--
-- Each trigger recomputes the folder_threads row that a change touches from
-- message_folders via message_folders_thread, a small index range scan, and
-- adjusts the folder counters incrementally, as version 1 did from messages.

CREATE TRIGGER message_folders_after_insert AFTER INSERT ON message_folders BEGIN
    UPDATE folders SET total = total + 1, unread = unread + (NEW.is_read = 0)
     WHERE id = NEW.folder_id;

    DELETE FROM folder_threads WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), message_id, count(*), sum(is_read = 0)
      FROM message_folders WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id
    HAVING count(*) > 0;
END;

CREATE TRIGGER message_folders_after_delete AFTER DELETE ON message_folders BEGIN
    UPDATE folders SET total = total - 1, unread = unread - (OLD.is_read = 0)
     WHERE id = OLD.folder_id;

    DELETE FROM folder_threads WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), message_id, count(*), sum(is_read = 0)
      FROM message_folders WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id
    HAVING count(*) > 0;
END;

CREATE TRIGGER message_folders_after_update AFTER UPDATE ON message_folders BEGIN
    UPDATE folders SET total = total - 1, unread = unread - (OLD.is_read = 0)
     WHERE id = OLD.folder_id;
    UPDATE folders SET total = total + 1, unread = unread + (NEW.is_read = 0)
     WHERE id = NEW.folder_id;

    DELETE FROM folder_threads WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), message_id, count(*), sum(is_read = 0)
      FROM message_folders WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id
    HAVING count(*) > 0;

    DELETE FROM folder_threads WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), message_id, count(*), sum(is_read = 0)
      FROM message_folders WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id
    HAVING count(*) > 0;
END;

-- Search indexes every folder a message is in. A change to the primary
-- location is already queued by version 3's triggers on messages; these
-- queue the other folders' rows coming and going.
CREATE TRIGGER search_queue_folder_insert AFTER INSERT ON message_folders
WHEN NEW.folder_id IS NOT (SELECT folder_id FROM messages WHERE id = NEW.message_id)
BEGIN
    INSERT INTO search_queue (message_id) VALUES (NEW.message_id);
END;

CREATE TRIGGER search_queue_folder_delete AFTER DELETE ON message_folders
WHEN OLD.folder_id IS NOT (SELECT folder_id FROM messages WHERE id = OLD.message_id)
BEGIN
    INSERT INTO search_queue (message_id) VALUES (OLD.message_id);
END;
