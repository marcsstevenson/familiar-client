# Request: approve the "Familiar" mail app for Marc's iM3 Dental account

**From:** Marc Stevenson
**What I'm asking:** admin consent for one desktop app so that I, and only I, can use it with my iM3 mailbox and calendar.

## What Familiar is

Familiar is an open source desktop email and calendar client that I'm building for my own use, as a replacement for Outlook. It talks to Microsoft 365 through Microsoft Graph, the same official API that Outlook mobile and other approved clients use.

- **No server.** It runs only on my computer. Mail is synced to a local database on that computer and goes nowhere else. There is no cloud service, and no third party receives iM3 data.
- **No secrets.** It's a public client app (like the Outlook desktop app). It signs in through the normal Microsoft sign-in page, so MFA and your Conditional Access policies still apply. Sign-in tokens are stored in the Windows credential store.
- **Revocable at any time.** You can remove it in Entra under Enterprise applications, and access stops at once.

## App details

| | |
| --- | --- |
| App name | Familiar |
| Application (client) ID | `CLIENT_ID` |
| Registered in | Marc's own Azure directory (not iM3's) |
| Publisher verification | Not yet verified |
| Source code | https://github.com/marcsstevenson/familiar-client |

## Permissions requested (delegated, so only my own data)

| Permission | Why |
| --- | --- |
| `User.Read` | Sign in and read my profile |
| `offline_access` | Stay signed in without prompting every hour |
| `Mail.ReadWrite` | Read and organise my mail |
| `Mail.Send` | Send mail as me |
| `Calendars.ReadWrite` | Read and update my calendar |
| `Contacts.Read` | Address suggestions from my contacts |
| `MailboxSettings.ReadWrite` | Time zone, automatic replies |

All of these are delegated: the app can only act as me, on my own mailbox, while I'm signed in. It can't read anyone else's mail.

If you'd rather start smaller, read-only access is enough for now: `User.Read`, `offline_access`, `Mail.Read`, `Calendars.Read`.

## How to approve it (about 2 minutes)

**Option A, simplest:** sign in as an iM3 admin and open this link:

```
https://login.microsoftonline.com/<iM3 tenant domain>/v2.0/adminconsent?client_id=CLIENT_ID&scope=https%3A%2F%2Fgraph.microsoft.com%2F.default&redirect_uri=http%3A%2F%2Flocalhost
```

The page ends on a `localhost` address that won't load. That's expected.

**To limit it to me only (recommended):** after approving, go to **Entra admin centre > Enterprise applications > Familiar > Properties**, set **Assignment required?** to **Yes**, then under **Users and groups** add only me.

**Option B:** if you have the admin consent workflow turned on, I can submit a request from the "Need admin approval" screen and you approve it from **Enterprise applications > Admin consent requests**.

Thanks. I'm happy to show you the app or answer any questions.
