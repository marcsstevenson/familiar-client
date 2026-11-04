-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Familiar store, schema version 1.
--
-- Migrations are append-only: once a file has shipped on main it is never
-- edited. Change the schema by adding the next numbered file.
--
-- Times are Unix seconds (UTC). Remote IDs are Graph immutable IDs.
-- Folder counters, `threads` and `folder_threads` are derived from `messages`
-- by the triggers at the end of this file, so the writer can never forget to
-- update them and readers never have to count.

CREATE TABLE accounts (
    id           INTEGER PRIMARY KEY,
    -- The account's key in config.toml, e.g. "hotmail".
    key          TEXT NOT NULL UNIQUE,
    email        TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    provider     TEXT NOT NULL DEFAULT 'graph',
    -- Directory tenant for work accounts; token refresh must use it.
    tenant_id    TEXT,
    created_at   INTEGER NOT NULL
);

CREATE TABLE folders (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    remote_id  TEXT NOT NULL,
    parent_id  INTEGER REFERENCES folders(id) ON DELETE SET NULL,
    name       TEXT NOT NULL,
    -- Graph well-known name (inbox, sentitems, deleteditems, ...), if any.
    well_known TEXT,
    -- Stored counters, maintained by triggers (WP 0.5: a recount takes 0.86 s).
    total      INTEGER NOT NULL DEFAULT 0,
    unread     INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, remote_id)
);
CREATE INDEX folders_parent ON folders(parent_id);

-- One row per conversation, across folders. Graph's 76-character
-- conversationId is stored once here and messages refer to it by integer.
CREATE TABLE threads (
    id               INTEGER PRIMARY KEY,
    account_id       INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    conversation_id  TEXT NOT NULL,
    message_count    INTEGER NOT NULL DEFAULT 0,
    unread_count     INTEGER NOT NULL DEFAULT 0,
    last_received_at INTEGER,
    last_message_id  INTEGER,
    UNIQUE (account_id, conversation_id)
);

-- Message bodies, keyed by the BLAKE3 hash of the raw (uncompressed) bytes.
-- Small bodies are stored inline as zstd; large ones live in a file named by
-- the hash under bodies/, and `data` is NULL.
CREATE TABLE bodies (
    hash        BLOB PRIMARY KEY,
    size        INTEGER NOT NULL,
    stored_size INTEGER NOT NULL,
    data        BLOB
) WITHOUT ROWID;

CREATE TABLE messages (
    id                  INTEGER PRIMARY KEY,
    account_id          INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id           INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    thread_id           INTEGER NOT NULL REFERENCES threads(id),
    remote_id           TEXT NOT NULL,
    internet_message_id TEXT,
    subject             TEXT NOT NULL DEFAULT '',
    from_name           TEXT NOT NULL DEFAULT '',
    from_addr           TEXT NOT NULL DEFAULT '',
    -- Recipient lists as JSON arrays of {"name", "addr"}.
    to_json             TEXT NOT NULL DEFAULT '[]',
    cc_json             TEXT NOT NULL DEFAULT '[]',
    received_at         INTEGER NOT NULL,
    sent_at             INTEGER,
    is_read             INTEGER NOT NULL DEFAULT 0 CHECK (is_read IN (0, 1)),
    is_flagged          INTEGER NOT NULL DEFAULT 0 CHECK (is_flagged IN (0, 1)),
    importance          INTEGER NOT NULL DEFAULT 1,
    has_attachments     INTEGER NOT NULL DEFAULT 0 CHECK (has_attachments IN (0, 1)),
    preview             TEXT NOT NULL DEFAULT '',
    -- NULL while only the header is local (older than the body window).
    body_hash           BLOB REFERENCES bodies(hash),
    body_is_html        INTEGER NOT NULL DEFAULT 0 CHECK (body_is_html IN (0, 1)),
    -- Pinned bodies are kept even outside the body window.
    body_pinned         INTEGER NOT NULL DEFAULT 0 CHECK (body_pinned IN (0, 1)),
    UNIQUE (account_id, remote_id)
);
-- Keyset paging: (received_at, id) cursor, never OFFSET.
CREATE INDEX messages_folder_time  ON messages(folder_id, received_at DESC, id DESC);
CREATE INDEX messages_thread       ON messages(thread_id, folder_id, received_at DESC);
CREATE INDEX messages_imid         ON messages(internet_message_id);
CREATE INDEX messages_body         ON messages(body_hash) WHERE body_hash IS NOT NULL;

-- One row per conversation per folder, so a threaded folder view is an index
-- range scan rather than a GROUP BY.
CREATE TABLE folder_threads (
    folder_id        INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    thread_id        INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    last_received_at INTEGER NOT NULL,
    last_message_id  INTEGER NOT NULL,
    message_count    INTEGER NOT NULL,
    unread_count     INTEGER NOT NULL,
    PRIMARY KEY (folder_id, thread_id)
) WITHOUT ROWID;
CREATE INDEX folder_threads_time ON folder_threads(folder_id, last_received_at DESC, thread_id DESC);

-- Sync bookmarks: Graph delta links per account and resource, for example
-- "folders" for the folder tree or "messages:<folder remote id>".
CREATE TABLE sync_state (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    resource   TEXT NOT NULL,
    -- Set when the resource belongs to one folder, so it goes with the folder.
    folder_id  INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    delta_link TEXT,
    synced_at  INTEGER,
    last_error TEXT,
    PRIMARY KEY (account_id, resource)
) WITHOUT ROWID;

-- Durable queue of local writes waiting to reach the server. An action
-- updates the store and adds a row here in the same transaction.
CREATE TABLE ops (
    id              INTEGER PRIMARY KEY,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- What the op does, e.g. "mark_read", "move", "send".
    kind            TEXT NOT NULL,
    -- What it acts on, e.g. a message remote ID. Ops on the same target run
    -- in order, one at a time.
    target          TEXT NOT NULL DEFAULT '',
    -- JSON arguments.
    payload         TEXT NOT NULL DEFAULT '{}',
    -- Sent to the server so a retried op is not applied twice.
    idempotency_key TEXT NOT NULL UNIQUE,
    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending', 'in_flight', 'done', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    not_before      INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX ops_ready  ON ops(state, not_before, id);
CREATE INDEX ops_target ON ops(account_id, target, id);

-- Derived data -------------------------------------------------------------
--
-- Each trigger recomputes the folder_threads row and the threads row that a
-- message change touches, from `messages` via messages_thread. That is a
-- small index range scan, and it keeps the derived rows exactly equal to a
-- full recount (tested). Folder counters are adjusted incrementally.

CREATE TRIGGER messages_after_insert AFTER INSERT ON messages BEGIN
    UPDATE folders SET total = total + 1, unread = unread + (NEW.is_read = 0)
     WHERE id = NEW.folder_id;

    DELETE FROM folder_threads WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), id, count(*), sum(is_read = 0)
      FROM messages WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id
    HAVING count(*) > 0;

    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = NEW.thread_id)
     WHERE id = NEW.thread_id;
END;

CREATE TRIGGER messages_after_delete AFTER DELETE ON messages BEGIN
    UPDATE folders SET total = total - 1, unread = unread - (OLD.is_read = 0)
     WHERE id = OLD.folder_id;

    DELETE FROM folder_threads WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), id, count(*), sum(is_read = 0)
      FROM messages WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id
    HAVING count(*) > 0;

    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = OLD.thread_id)
     WHERE id = OLD.thread_id;
    DELETE FROM threads WHERE id = OLD.thread_id AND message_count = 0;
END;

CREATE TRIGGER messages_after_update
AFTER UPDATE OF folder_id, thread_id, is_read, received_at ON messages BEGIN
    UPDATE folders SET total = total - 1, unread = unread - (OLD.is_read = 0)
     WHERE id = OLD.folder_id;
    UPDATE folders SET total = total + 1, unread = unread + (NEW.is_read = 0)
     WHERE id = NEW.folder_id;

    DELETE FROM folder_threads WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), id, count(*), sum(is_read = 0)
      FROM messages WHERE folder_id = OLD.folder_id AND thread_id = OLD.thread_id
    HAVING count(*) > 0;

    DELETE FROM folder_threads WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id;
    INSERT INTO folder_threads
    SELECT folder_id, thread_id, max(received_at), id, count(*), sum(is_read = 0)
      FROM messages WHERE folder_id = NEW.folder_id AND thread_id = NEW.thread_id
    HAVING count(*) > 0;

    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = OLD.thread_id)
     WHERE id = OLD.thread_id;
    UPDATE threads SET (message_count, unread_count, last_received_at, last_message_id) =
        (SELECT count(*), coalesce(sum(is_read = 0), 0), max(received_at), id
           FROM messages WHERE thread_id = NEW.thread_id)
     WHERE id = NEW.thread_id;
    DELETE FROM threads WHERE id = OLD.thread_id AND message_count = 0;
END;
