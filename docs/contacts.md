# Contacts

`fam contacts search <words>` (WP 2.5) finds people to write to, for
address autocomplete in the CLI, the GUI and agents. It reads the store, so
it works without famd.

```text
$ fam contacts search jan
Jane Coach <jane.coach@club.example>  (contact, Parklands Cricket, sent 12, received 40)
Jane (home) <jane@home.example>  (contact, Parklands Cricket)
```

`--json` gives each person's `name`, `address`, `mailbox` (`Name <addr>`,
ready for `--to`), `contact`, `company`, `accounts`, `sent`, `received`
and `last`. `--account` limits it to one account, `--limit` (1 to 200,
default 20) the number shown. An empty search lists the people written to
most.

## Where the people come from

- **Outlook contacts.** famd lists every contact (the default contacts
  folder and each contact folder in it) at start and then every hour
  (`SyncPolicy::contacts_interval`), replacing the store's copy, so a
  contact deleted in Outlook goes too. Uses `Contacts.Read`, consented at
  sign-in. A mailbox that refuses is logged and tried again next hour.
- **Past correspondents.** Everyone mail came from (outside Sent Items,
  Drafts and Outbox, and not the account itself) and everyone mail in Sent
  Items went to (To and Cc), per account, with counts and the last date.
  Triggers on `messages` keep them as mail arrives (schema version 6); the
  migration fills them in from mail already stored. Counts only grow:
  deleting mail doesn't forget who you wrote to.

## Matching and order

A person matches when their name, address or company starts with the
words, or a word of their name does (`coach` finds Jane Coach). Case
doesn't matter and `%`/`_` are plain characters. Contacts come first, then
people written to most (each sent message counts 20 received ones), then
the most recent. The same address from several accounts or from both
sources is one person.
