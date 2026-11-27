# Contacts

The desktop **Contacts** tab browses the saved address books of all synced Outlook and Gmail accounts. Search by name, organization, address or phone, filter by account, and select a card to view details or compose an email from that account. A contact with several addresses has one card per provider record.

Use Ctrl+F (Cmd+F on macOS) to search. Choose **New contact** or press Ctrl+N (Cmd+N on macOS) and a destination account to create a card. Edit changes the selected card in its owning account. Delete asks for confirmation before removing it from that provider. Browsing uses the local copy offline. Saving and deleting need a running daemon and a connection to the provider.

Existing accounts with read-only contact access can continue browsing. To grant contact editing, run `fam auth login <account>` and approve the additional permission. Managed accounts may need their administrator's consent.

The same operations are available through `fam contacts list`, `create`, `edit` and `delete`. Use `fam contacts list --json` to get local IDs and accounts. For example, `fam contacts create --account work --name "Jane Coach" --emails jane@example.org`. Deleting a card needs `--yes`.

## Select, merge and delete

Check a card to select it, or use the checkbox above the list to select the visible results. **Delete…** asks once for the selected contacts and names the affected accounts. If a provider rejects a deletion, Familiar stops and reports how many cards it removed.

**Likely duplicates** finds cards in the same account with a matching name, email address or phone number. Choose a card to see the matching value and select its matches. You can also select cards manually. **Merge…** lets you choose which card to keep and review the result. It keeps the first available name and company, starting with your chosen card, and combines email addresses, phone numbers and Google favourite status. The other cards are deleted after the result is saved and checked. Provider-only details such as photos, labels and notes stay with the card you keep. Details of that kind on the other cards are lost.

## Google favourites and links

The **Favourites** view shows starred cards from your synced Google accounts. Use the star on a Google card to change its favourite status in Google Contacts. Stars changed in Google appear on the next contact sync. **Open in Google Contacts** opens the selected person in your browser with the owning Google account.

## Recipient suggestions

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
  contact deleted in Outlook goes too. Uses `Contacts.ReadWrite`, consented at
  sign-in. A mailbox that refuses is logged and tried again next hour.
- **Google contacts.** For a Gmail account, famd lists the saved contacts
  (People API `people.connections`) on the same hourly schedule and
  replaces the store's copy the same way. Uses `contacts`, asked
  for at Google sign-in. Gmail's automatic "other contacts" aren't read:
  they are people the account has mailed, which past correspondents
  already cover.
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
