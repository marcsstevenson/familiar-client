# Follow-up reminders

"Remind me if nobody replies in 3 days." Familiar works this out on this
device from the conversation threads it already has; nothing is set on the
server, and nothing runs in the background.

```sh
fam followup add 4182              # due in 3 days
fam followup add 4182 --days 7
fam draft send 9001 --remind-days 3
fam mail send --to bob@example.com --subject Quote --body "..." --remind-days 3
fam followup list                  # waiting, due or sending, soonest first
fam followup list --due
fam followup cancel 4182
```

In the desktop app: the compose window's **Remind me** menu (next to Format)
does the same as `--remind-days`; a message's context menu has **Remind me if
no reply** (3 days) and **Cancel reminder**. While there are reminders, the
folder pane shows **Follow up** below All inboxes, with the number due; it
lists the waiting messages soonest due first. Rows with a reminder show its
date, or **No reply** once it is due.

## How a reply is recognised

A reminder is answered when its conversation (Graph's `conversationId`, the
same thread `fam mail thread` shows) has a message newer than the waiting one
that is not in Sent Items or Drafts and is not from the account's own address
or one of its `aliases`. Answered reminders drop off the next time the
reminders are listed. Your own follow-up messages don't count as replies.

## Storage

`followups.json` in the data directory, one entry per reminder: the account,
the waiting message's Internet Message-ID (which the Sent Items copy keeps),
the subject and the due time. A draft sent with a reminder has no Message-ID
yet, so its entry holds the send op's ID until the op has read one (it does
so just before sending). If the op is gone without one, the send was undone
and the reminder goes too.
