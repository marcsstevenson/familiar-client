-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Schema version 6: Outlook contacts and past correspondents (WP 2.5), for
-- address autocomplete and `fam contacts search`.

-- Contacts from the server, refreshed in full rounds like calendars.
CREATE TABLE contacts (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    remote_id    TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    company      TEXT NOT NULL DEFAULT '',
    -- JSON array of {"name", "addr"}.
    emails_json  TEXT NOT NULL DEFAULT '[]',
    -- JSON array of strings.
    phones_json  TEXT NOT NULL DEFAULT '[]',
    round        INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, remote_id)
);

-- One row per address a contact has, for search by address.
CREATE TABLE contact_addresses (
    contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    addr       TEXT NOT NULL COLLATE NOCASE
);
CREATE INDEX contact_addresses_addr ON contact_addresses(addr);
CREATE INDEX contact_addresses_contact ON contact_addresses(contact_id);

-- Everyone mail came from (outside Sent Items and Drafts) or went to (from
-- Sent Items), per account. Kept by the triggers below as messages arrive;
-- counts only grow.
CREATE TABLE correspondents (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    addr       TEXT NOT NULL COLLATE NOCASE,
    name       TEXT NOT NULL DEFAULT '',
    -- Messages received from them.
    received   INTEGER NOT NULL DEFAULT 0,
    -- Messages sent to them (To or Cc).
    sent       INTEGER NOT NULL DEFAULT 0,
    last_at    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, addr)
) WITHOUT ROWID;

CREATE TRIGGER correspondents_from AFTER INSERT ON messages
WHEN NEW.from_addr <> ''
 AND NOT EXISTS (SELECT 1 FROM folders WHERE id = NEW.folder_id
                  AND well_known IN ('sentitems', 'drafts', 'outbox'))
 AND NOT EXISTS (SELECT 1 FROM accounts WHERE id = NEW.account_id
                  AND lower(email) = lower(NEW.from_addr))
BEGIN
    INSERT INTO correspondents (account_id, addr, name, received, sent, last_at)
    VALUES (NEW.account_id, lower(NEW.from_addr), NEW.from_name, 1, 0, NEW.received_at)
    ON CONFLICT (account_id, addr) DO UPDATE SET
        received = received + 1,
        name = CASE WHEN excluded.name <> '' AND (name = '' OR excluded.last_at >= last_at)
                    THEN excluded.name ELSE name END,
        last_at = max(last_at, excluded.last_at);
END;

CREATE TRIGGER correspondents_to AFTER INSERT ON messages
WHEN EXISTS (SELECT 1 FROM folders WHERE id = NEW.folder_id AND well_known = 'sentitems')
BEGIN
    INSERT INTO correspondents (account_id, addr, name, received, sent, last_at)
    SELECT NEW.account_id, lower(json_extract(value, '$.addr')),
           coalesce(json_extract(value, '$.name'), ''), 0, 1, NEW.received_at
      FROM (SELECT value FROM json_each(NEW.to_json)
            UNION ALL SELECT value FROM json_each(NEW.cc_json))
     WHERE coalesce(json_extract(value, '$.addr'), '') <> ''
    ON CONFLICT (account_id, addr) DO UPDATE SET
        sent = sent + 1,
        name = CASE WHEN excluded.name <> '' AND (name = '' OR excluded.last_at >= last_at)
                    THEN excluded.name ELSE name END,
        last_at = max(last_at, excluded.last_at);
END;

-- Mail already stored.
INSERT INTO correspondents (account_id, addr, name, received, sent, last_at)
SELECT m.account_id, lower(m.from_addr), max(m.from_name), count(*), 0, max(m.received_at)
  FROM messages m
 WHERE m.from_addr <> ''
   AND NOT EXISTS (SELECT 1 FROM folders f WHERE f.id = m.folder_id
                    AND f.well_known IN ('sentitems', 'drafts', 'outbox'))
   AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.id = m.account_id
                    AND lower(a.email) = lower(m.from_addr))
 GROUP BY m.account_id, lower(m.from_addr);

INSERT INTO correspondents (account_id, addr, name, received, sent, last_at)
SELECT m.account_id, lower(json_extract(j.value, '$.addr')),
       max(coalesce(json_extract(j.value, '$.name'), '')), 0, count(*), max(m.received_at)
  FROM messages m
  JOIN folders f ON f.id = m.folder_id AND f.well_known = 'sentitems',
       json_each(m.to_json) j
 WHERE coalesce(json_extract(j.value, '$.addr'), '') <> ''
 GROUP BY m.account_id, lower(json_extract(j.value, '$.addr'))
ON CONFLICT (account_id, addr) DO UPDATE SET
    sent = sent + excluded.sent,
    last_at = max(last_at, excluded.last_at),
    name = CASE WHEN name = '' THEN excluded.name ELSE name END;

INSERT INTO correspondents (account_id, addr, name, received, sent, last_at)
SELECT m.account_id, lower(json_extract(j.value, '$.addr')),
       max(coalesce(json_extract(j.value, '$.name'), '')), 0, count(*), max(m.received_at)
  FROM messages m
  JOIN folders f ON f.id = m.folder_id AND f.well_known = 'sentitems',
       json_each(m.cc_json) j
 WHERE coalesce(json_extract(j.value, '$.addr'), '') <> ''
 GROUP BY m.account_id, lower(json_extract(j.value, '$.addr'))
ON CONFLICT (account_id, addr) DO UPDATE SET
    sent = sent + excluded.sent,
    last_at = max(last_at, excluded.last_at),
    name = CASE WHEN name = '' THEN excluded.name ELSE name END;
