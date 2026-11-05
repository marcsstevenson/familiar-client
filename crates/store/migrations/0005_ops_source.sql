-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 5: who queued each op, and the audit log (WP 2.1, 2.6).
--
-- `source` is the caller that asked for the write: "cli", "gui", "mcp",
-- or an agent name given with --source. `summary` is a one-line description
-- for `fam ops list` and the audit log.
ALTER TABLE ops ADD COLUMN source  TEXT NOT NULL DEFAULT 'cli';
ALTER TABLE ops ADD COLUMN summary TEXT NOT NULL DEFAULT '';

-- Every automated write, and every write refused by agent permissions.
-- Rows are only added; `fam audit list` reads them.
CREATE TABLE audit (
    id         INTEGER PRIMARY KEY,
    at         INTEGER NOT NULL,
    account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    source     TEXT NOT NULL,
    -- The command, e.g. "mail.move".
    command    TEXT NOT NULL,
    -- "queued", "denied", "done" or "failed".
    outcome    TEXT NOT NULL,
    op_id      INTEGER,
    detail     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX audit_at ON audit(at);
