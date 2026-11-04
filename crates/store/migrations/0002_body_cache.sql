-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 2: the body cache and the sync engine's body queries (WP 1.7).
--
-- A body is one of three kinds:
--   window  the message is inside the sync window; kept while it stays there
--   pinned  body_pinned = 1; kept whatever its age
--   cached  fetched on demand (or unpinned) outside the window; kept in a
--           size-limited cache, oldest body_cached_at evicted first
-- body_cached_at is NULL for window and pinned bodies.

ALTER TABLE messages ADD COLUMN body_cached_at INTEGER;

-- Newest-first scans of one account: bodies still to fetch for the window,
-- and bodies that have aged out of it.
CREATE INDEX messages_account_time ON messages(account_id, received_at DESC);
CREATE INDEX messages_cached ON messages(body_cached_at) WHERE body_cached_at IS NOT NULL;
