# Writes and the op queue

Familiar writes local-first (WP 2.1, 2.2). A write changes the store at once,
so `fam mail list` and the GUI show it straight away, and queues an **op**
for the server in the same transaction. `famd` sends ops and keeps trying
until the server has them.

## Commands

| Command | Does | Access |
| --- | --- | --- |
| `fam mail read <ids>... [--unread]` | Mark read or unread | write |
| `fam mail flag <ids>... [--clear]` | Flag, or clear the flag | write |
| `fam mail move <ids>... --to <folder>` | Move to a folder of the same account | write |
| `fam mail archive <ids>...` | Move to Archive | write |
| `fam mail report <ids>... [--phishing \| --not-junk]` | Report to the provider as junk (default), phishing or not junk; see below | write |
| `fam mail snooze <ids>... --until <when> [--tz]` | Move to Snoozed until a time; famd brings it back; see below | write |
| `fam mail unsnooze <ids>...` | Bring snoozed messages back now | write |
| `fam mail snoozed` | Snoozed messages and when each comes back | read |
| `fam mail delete <ids>... [--permanent --yes]` | Move to Deleted Items; `--permanent` deletes items already there for good | delete |
| `fam folder create <name> [--parent <folder>]` | Create a folder | write |
| `fam folder rename <folder> <name>` | Rename | write |
| `fam folder move <folder> (--to <folder> \| --top)` | Move under another folder, or to the top level | write |
| `fam folder delete <folder> [--permanent --yes]` | Move into Deleted Items, or delete for good from there | delete |
| `fam ops list [--all]` | Changes waiting for the server, and failed ones | read |
| `fam ops retry <id>` / `fam ops cancel <id>` | Try a failed change again / drop a queued or failed one and undo it here | write |

| `fam draft create [--to ...] [--subject] [--body \| --body-file] [--html] [--attach f] [--inline img]` | Save a new draft with the account's `new` signature | draft |
| `fam draft reply <id> [--all] [--body ...]` | Save a reply (threaded via Graph's `createReply`), your text and `reply` signature above the quoted original | draft |
| `fam draft forward <id> --to ... [--body ...]` | Save a forward with the original's attachments | draft |
| `fam draft update <draft> [--to/--cc/--bcc] [--subject] [--body] [--attach] [--remove-attachment]` | Change a draft; a recipient flag replaces that line (`--cc ""` empties it), `--body` the whole body. Images the body embeds become inline attachments with content IDs taken from their bytes, so saving the same body again adds nothing, and inline images the new body no longer shows are taken off. `--remove-attachment` takes a file off by name or Graph attachment ID | draft |
| `fam draft edit [<draft>] [--account] [--no-signature]` | Write a draft (or, without an ID, a new message) in your editor, in Markdown; see below | draft |
| `fam draft send <draft> [--now] [--yes] [--at <when>]` | Send after the undo-send delay, or later with `--at` | send |
| `fam mail send --to ... --subject ... --body ... [--now] [--yes] [--at <when>]` | Draft and send in one go | send |
| `fam cal add <subject> --start <when> [--end \| --minutes] [--calendar \| --purpose] [--attendee ...] [--repeat ...] [--from-mail <id>]` | Add an event (invitations go to attendees) | write |
| `fam cal edit <event> [--subject] [--start] [--end \| --minutes] [--location] [--series]` | Change an event, or with `--series` every event of its series | write |
| `fam cal delete <event> [--series] [--comment] [--yes]` | Delete; a meeting you organised is cancelled and attendees told (needs `--yes`) | delete |
| `fam cal rsvp <event> accept\|tentative\|decline [--no-reply] [--series]` | Answer an invitation; a declined meeting leaves the calendar | write |

The compose commands also take `--markdown`: the body is Markdown and is
saved as HTML. A single line break stays a line break, as people expect in
mail; tables, strikethrough and task lists work too.

Every write takes `--dry-run` (say what would change, change nothing) and
`--wait` (wait until the server has it, up to `--wait-secs`, default 60).
Folders are named by path (`Inbox/Club`), well-known name (`inbox`,
`archive`, `deleteditems`) or plain name. Folder commands need `--account`
when more than one account is synced.

Exit codes: a refused change under `--wait` is `server` (12); a `--wait`
that times out is `offline` (7) and the change stays queued; `--wait`
without famd is `daemon_unavailable` (11) and nothing changes; a permanent
delete without `--yes` is `confirmation_required` (10).

## Reporting junk and phishing

`fam mail report` (and the message list's right-click menu) follows the
provider's own report process, so its filter learns from it:

- **Outlook (Graph).** Graph's `reportMessage` action with `ReportAction`
  `junk`, `phish` or `notJunk` and `IsMessageMoveRequested: true`. Graph has
  it only on the beta endpoint, so the runner posts to `/beta` (everything
  else stays on v1.0). The server moves the message: junk into Junk Email,
  not junk back to the Inbox. Outlook.com deletes a message reported as
  phishing (it goes to the hidden Recoverable Items; checked live on Hotmail
  2026-11-09). Locally it moves to Junk Email (Inbox for not junk) at once,
  and the next sync shows where the server put it. Graph's answer is not a
  whole message, so it is not read. Not junk is only for messages in Junk
  Email; junk on a message already there does nothing.
- **Gmail (later).** The op payload (`report_message` with a provider-neutral
  `Report`) is the seam. Gmail has no report API beyond its labels: junk and
  phishing add `SPAM` and remove `INBOX`, not junk does the reverse.

## Snooze

`fam mail snooze` (and Snooze… in the message list's right-click menu)
moves messages to the account's **Snoozed** folder, made at the top level
the first time like any other folder, so Outlook on the web and phones see
them out of the Inbox too. The store's `snoozes` table (schema 7) keeps when
each comes back and the folder it came from.

- **Times** (`--until`, read in `--tz`, default this computer's zone):
  `3h`, `2d`, `1w`, `90m` from now; `tonight` (6 pm); `tomorrow`,
  `weekend` (Saturday) and `next week` (Monday) at 9 am; or anything
  `fam cal add --start` takes (`mon 9am`, `fri 2pm`, `21 nov 8am`,
  `2026-11-21 08:30`; a date alone is 9 am). A day name whose time today
  has passed is next week's. A time gone by is refused.
- **Coming back.** famd looks every 30 s (and at start) and moves each due
  message back to the folder it came from (the Inbox if that folder is gone
  or it was put in Snoozed by hand), marked unread, through the op queue.
  **famd has to be running**: with the computer off nothing comes back
  until famd next starts, and then everything due comes back at once.
  Graph keeps the message's received time, so it comes back in its old
  place in the list; unread is what makes it stand out.
- **Changing your mind.** Snoozing a snoozed message again only changes
  the time. `fam mail unsnooze` brings it back now (read state kept). A
  message moved out of Snoozed by hand, here or in another app, stays where
  it was put when its time comes; the snooze just ends.

## Compose and send (WP 2.3)

- **Signatures** come from `[accounts.<name>.signature]` in `config.toml`:
  `new` for new messages, `reply` for replies and forwards. `--no-signature`
  leaves it off. Bodies are plain text unless `--html`.
- **Aliases.** `--from <address>` sends from one of the account's
  `aliases` in `config.toml` (an address not listed is refused). Replies and
  forwards default to the alias the original was sent to; `fam draft update
  --from` changes it, back to the account's own address too. Graph gets it as
  the draft's `from`.
- **Attachments** are read by `fam` and travel in the op, up to 3 MB each
  (Graph's single-request limit; upload sessions for bigger files are not
  done yet). `--inline img.png` adds an inline image referenced as
  `cid:img.png` in an HTML body.
- **Undo send.** A send waits `[app] send_delay_seconds` (default 10) in the
  queue. The output names the op; `fam ops cancel <op>` before then keeps
  the draft. `--now` skips the delay.
- **Send later.** `--at <when>` (`+2h`, `+1d`, `tomorrow 9am`, `fri 8:30`,
  `2026-11-21 18:00` in local time, or RFC 3339; a date alone is 9am) and the
  compose window's *Later…* button. After the undo-send delay the op stamps
  the draft with MAPI `PidTagDeferredSendTime` (Graph extended property
  `SystemTime 0x3FEF`) and calls `/send`; Exchange then holds the message
  and sends it at that time itself, so it goes with the PC off. The op is
  done once the server has it; `send_at` on the op output gives the time.
- **Outbox.** Offline, or with famd stopped, the draft and the send wait in
  the queue and go when they can. A send whose answer was lost is checked
  against Sent Items by Internet message ID before it is called failed.
- **Many outside recipients.** More than 10 To/Cc recipients outside the
  account's own domain, who would all see each other, needs `--yes`
  (exit 10 otherwise); Bcc doesn't count.
- A draft is made on the server in steps (create, then body and recipients
  for responses, then each attachment), and the op records each step, so a
  retry carries on. A lost answer to the very first create can leave one
  extra copy in Drafts on the server.

## Calendar (WP 2.4)

- **Times** are read in `--tz` (IANA or Windows name; default this
  computer's zone): `"2026-11-21 18:00"`, `"sat 6pm"`, `"tomorrow 9:30am"`,
  `21/11 18:00` (day first), `"21 Nov 6:30pm"`. A date alone makes an
  all-day event (`--days` or an `--end` date for longer; the end date is the
  last day). A timed event lasts `--minutes` (default 60) unless `--end`;
  `--end 20:00` is on the start's day. `fam cal edit --start` keeps the
  length.
- **Where.** `--calendar` takes a calendar name or a purpose from
  `[calendar.purposes]`; `--purpose family` takes only a purpose (exit 3 if
  there's no such purpose). Without either, the account's main calendar;
  `--account` picks the account when more than one is synced.
- **Recurring.** `--repeat daily|weekly|monthly|yearly`, `--every N`,
  `--on mon,wed` (weekly), `--until <date>` or `--count N`. The server
  expands the series; until it has, the first occurrence shows locally,
  and its occurrences then replace it. `--series` on edit, delete and rsvp
  acts on the whole series.
- **From an email.** `fam cal add --from-mail <message id>` takes the
  subject (without `RE:`/`FW:`), the first date with a time or a month
  (`Saturday 21 November at 6:30 pm`, `12/1/2027`, `2026-11-21`) and a
  `Where:`/`Location:`/`Venue:` line, unless given; the event's notes end
  with a link back to the email (Outlook on the web). With no date found,
  `--start` is needed (exit 2).
- **Graph.** Times go as wall-clock times in the event's zone (Windows
  name), so a weekly event keeps its local time over daylight saving. A
  new event carries a `transactionId`, so a POST retried after a lost answer
  makes one event. Only the organiser can change subject, times, place or
  attendees (Familiar refuses first). Deleting a meeting you organised with
  `--comment` uses `/cancel`, otherwise `DELETE` (which also tells
  attendees). More than 10 invitees needs `--yes`.
- **Locally.** A deleted event is hidden while its op is queued and comes
  back if the server refuses; edits and answers are undone the same way.
  Sync leaves events with queued changes alone.

## How the queue behaves

- **Order.** Each account's ops run one at a time in queue order, so a move
  into a folder created a moment before waits for the folder.
- **Without famd.** `fam` takes the store's writer lock itself, makes the
  change and queues it. famd sends it when it starts.
- **Restarts and kills.** An op famd had claimed when it stopped is put back
  at the front of the queue at start. Every op is safe to send twice:
  read/flag/move are idempotent with immutable IDs, folder create looks for
  an existing folder of that name, and deletes treat "not found" as done.
- **Passing failures** (offline, throttled, 5xx, expired sign-in) wait 2 s,
  doubling to 5 min, and go again, for as long as it takes.
- **Refusals** (4xx, item gone) fail the op and undo its local change. A
  failed folder create also fails and undoes later ops that needed the
  folder.
- **Sync and pending changes.** While an item has an open op, sync leaves it
  alone, so a slow server can't flip a message back. The round after the
  op lands brings the server's view.
- **Audit log.** Every queued, done and failed op is recorded with its
  source (`cli`, `mcp`, an agent name) in the store's `audit` table.

Code: `familiar_core::ops` (planning, local apply/undo), `familiar_daemon::ops`
(the runner), `familiar_provider_graph` `write.rs` (Graph calls), mock routes
in `crates/mock-graph/src/writes.rs`. Tests: `crates/daemon/tests/writes.rs`.

## Writing in your editor

`fam draft edit <draft>` opens the draft in `$VISUAL` or `$EDITOR`
(Notepad on Windows and `vi` elsewhere when neither is set). The file
looks like this:

```
To: Ana <ana@example.com>, bo@example.com
Cc:
Bcc:
Subject: Saturday

Hi **Ana**,

- bring the roster
- and the keys

Thanks,
Marc
```

The header lines come first, then a blank line, then the body in Markdown.
When the editor closes, whatever changed is saved as `fam draft update`
would, with the Markdown turned into HTML. Closing it unchanged saves
nothing. The signature and, in a reply or forward, the quoted message
below your text are not in the file and are kept as they were. A draft
written elsewhere with fonts or colours comes back as plain Markdown, so
saving a change to its text drops that formatting. Bcc isn't kept on this
device, so the Bcc line starts empty and only a filled-in line changes it.

Without an ID, `fam draft edit` writes a new message, which gets the
account's `new` signature (`--no-signature` leaves it off; `--account`
picks the account when there is more than one). If the file can't be
saved (an unknown header line, say), the error names the file, which is
kept so nothing you wrote is lost.

The editor command runs through the shell, so arguments work. Editors
that return at once need their wait flag: `EDITOR="code --wait"`. On
Windows set it with `setx EDITOR "code --wait"` and open a new terminal.
`fam draft edit` isn't offered over MCP or in the desktop app, since it
needs a person at a terminal; agents use `--markdown` on the other
compose commands instead.
