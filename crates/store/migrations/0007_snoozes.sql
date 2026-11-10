-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 7: snoozed messages.
--
-- A snoozed message sits in the account's Snoozed folder until `until`
-- (Unix seconds), when famd moves it back to `return_folder_id` (the
-- Inbox when that folder is gone). The row goes with its message.
CREATE TABLE snoozes (
    message_id       INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    until            INTEGER NOT NULL,
    return_folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
    created_at       INTEGER NOT NULL
);
CREATE INDEX snoozes_until ON snoozes(until);
