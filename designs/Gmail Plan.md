# Gmail Plan

7 Nov 2026 · Companion to the [Development Plan](Development%20Plan.md) and the [Email Client Design Document](Email%20Client%20Design%20Document.md)

## Summary

Familiar can add Gmail accounts through Google's own APIs: the Gmail API for mail, the Calendar API for calendars and the People API for contacts. The store, search, op queue, agent permissions and every surface stay as they are. The work is a second provider crate, a small provider boundary in `famd`, Google sign-in, and one real store change: a Gmail message can carry several labels, while the store today gives each message exactly one folder.

The key decisions proposed are:

- **Gmail API, not IMAP.** IMAP with OAuth needs the same restricted Google scope as the API, so it saves no sign-up work, and the API gives native threads, a cheap change feed (`history.list`), server search with Gmail's own syntax, and send-as aliases with their signatures.
- **Marc's own Google Cloud project, in production but unverified.** Google's full verification for restricted Gmail scopes needs a yearly third-party security assessment (CASA). For one person's use that isn't worth it: an unverified production app works after a one-time "Google hasn't verified this app" warning, and is capped at 100 users for its lifetime. Testing mode is avoided because its refresh tokens expire every 7 days. The sign-in test (WP G.0) confirms this before any code is written.
- **Labels become folders, many-to-many.** Every label is shown as a folder, and a message appears in every label it carries. Archive removes the Inbox label, move swaps one label for another, and delete moves to Trash.
- **One change stream per account.** Graph syncs folder by folder; Gmail syncs the whole mailbox from one `historyId`. The engine keeps its schedule, body window, ageing and backoff, and asks the provider for "a round" instead of planning folders itself.
- **Gmail comes after the Phase 3 gate**, as its own phase (Phase G), unless Marc wants it sooner. Only WP G.0, the sign-in test, is worth doing now, because its answer could change the plan.

## Where Familiar is today

Familiar is Microsoft Graph only, and Graph is wired directly into `famd`:

| Area | How Graph-specific it is | What Gmail needs |
| --- | --- | --- |
| `familiar-auth` | Microsoft endpoints, tenants, scopes and the client ID are constants in the crate | A Google flow beside it, sharing the token stores and refresh timing |
| `familiar-provider-graph` | All of it, by design | A sibling `familiar-provider-gmail` |
| `famd` engine (`crates/daemon/src/engine.rs`) | Calls `MailSync`, `CalendarSync`, `ContactSync` and matches on `GraphError` directly; plans folders one at a time | A provider boundary (below) |
| `famd` op runner (`crates/daemon/src/ops.rs`) | Calls `MailApi` and `CalendarApi` and builds Graph JSON (attachments, events) | Each op sent through the account's provider |
| Store schema | `accounts.provider` already exists (default `'graph'`). `messages.folder_id` is a single folder, and folder counters and `folder_threads` are derived from it by triggers | A message-to-folder table (WP G.3) |
| Calendar schema | Neutral enough: events are stored expanded, times UTC, zones IANA | Google uses IANA natively, so less mapping than Graph |
| Core commands, op payloads, search query language, agent grants, MCP, GUI | Provider-neutral, apart from `fam api` (`graph.get`) and a few `well_known` folder names | `fam api` sends to the account's provider; Gmail system labels map to the same well-known names |
| Config | `tenant` is required for every account; `aliases` already exists | `provider = "gmail"`, no tenant; Google client in `[app.google]` |

So most of Familiar does not change. The parts that do are concentrated in `famd` and the store.

## Gmail API or IMAP

| | Gmail API | IMAP + SMTP with OAuth | IMAP + SMTP with an app password |
| --- | --- | --- | --- |
| Google scope | `gmail.modify` (restricted) plus `gmail.send` | `https://mail.google.com/` (restricted, and broader) | None, no OAuth app at all |
| Verification burden | Same as IMAP | Same as API | None, but needs 2-Step Verification, and Workspace admins can turn app passwords off |
| Change feed | `history.list` from a `historyId`, 2 quota units a call | `CONDSTORE`/`QRESYNC` per folder, or Gmail's `X-GM-*` extensions | Same as IMAP |
| Labels | Native, many per message | Emulated as folders; a message appears once per label | Same as IMAP |
| Threads | Native `threadId` | `X-GM-THRID` extension | Same as IMAP |
| Server search | Gmail search syntax (`q=`), paged, no result cap | `X-GM-RAW` | Same as IMAP |
| Send-as aliases and signatures | `users.settings.sendAs` lists both | Not available | Not available |
| Calendar and contacts | Same Google sign-in | Still needs the Google APIs | CalDAV/CardDAV with the app password |
| Reuse elsewhere | Gmail only | A generic IMAP provider later | A generic IMAP provider later |

**Recommendation: the Gmail API.** OAuth IMAP buys nothing on verification, and the API matches Familiar's existing model (a delta feed, server-side threads, JSON over HTTPS through one scheduler). App-password IMAP is the only route with no Google project at all, and it stays the fallback if WP G.0 shows the unverified-app route is closed. A generic IMAP provider remains on the design doc's "Later" list and is not part of this plan.

## Sign-in and Google verification

### Scopes

| Scope | Google's tier | Needed for |
| --- | --- | --- |
| `openid`, `email`, `profile` | Non-sensitive | Identity |
| `https://www.googleapis.com/auth/gmail.modify` | Restricted | Read, label, archive, trash, drafts |
| `https://www.googleapis.com/auth/gmail.send` | Sensitive | Send |
| `https://www.googleapis.com/auth/gmail.settings.basic` | To confirm in WP G.0 | Send-as aliases and signatures; filters (Phase 4 rules) |
| `https://www.googleapis.com/auth/calendar` (or `calendar.events` plus `calendar.readonly`) | Sensitive | Calendars and events |
| `https://www.googleapis.com/auth/contacts.readonly`, `contacts.other.readonly` | Sensitive | Contacts and Google's auto-saved "other contacts" |

`gmail.modify` makes the whole app "restricted", so the strictest rules apply whichever other scopes are added. Like the Microsoft sign-in, Familiar asks for everything at once so nobody consents twice. `gmail.modify` can't permanently delete mail, only move it to Trash, which suits "safe by default".

### Three ways to get a working Google sign-in

| Route | What it takes | Who can use it | Fits |
| --- | --- | --- | --- |
| **A. Testing mode**, Marc as a test user | Nothing beyond a Cloud project | Up to 100 listed test users | **No:** refresh tokens expire after 7 days, so Marc would sign in again every week |
| **B. Production, unverified** (recommended) | Publish the consent screen without submitting for verification | Anyone, after an "unverified app" warning, capped at 100 users over the project's lifetime | **Yes, for Marc.** Tokens last until revoked. The cap and the warning rule it out for a public release |
| **C. Production, verified** | Brand verification, a privacy policy and home page on a domain Marc owns, a demo video of each scope in use, and a CASA Tier 2 security assessment from an authorised lab, renewed every year | Anyone, no warning | Only if Familiar is published for other people's Gmail. Reported costs run from about US$540 a year through Google's discounted lab to far more elsewhere, plus 1 to 3 weeks or longer per round |

A Google Workspace account has a fourth route: if the Cloud project belongs to that Workspace organisation, an "Internal" app needs no verification. That only helps for a Workspace account Marc administers.

**Recommendation: route B for Marc, and "bring your own Google project" for anyone else.** Familiar's open source release documents how to create a Google project in about ten minutes, and the client ID goes in each person's own `config.toml`. Familiar does not ship a Google client ID, because a public one would spend Marc's 100-user lifetime cap on strangers and they'd all see his project name on the warning. This is the usual approach for open source mail tools that use the Gmail API.

**Relation to the Microsoft publisher verification work.** They are separate programmes and neither helps the other. Microsoft publisher verification is free and about a verified business identity (it was looked at for iM3 Dental's consent block). Google verification for restricted scopes is about a yearly security audit of the app. Both want a business identity and a domain, so if Marc ever sets those up for one, the other gets easier, but neither is needed for Marc's own accounts.

### The flow

- **Desktop and CLI:** authorisation code with PKCE through the system browser, redirecting to a loopback address (`http://127.0.0.1:<port>`), the same shape as the Microsoft flow. Google's "desktop app" client type comes with a client secret that Google says is not treated as secret for installed apps; it goes in config with the client ID.
- **Headless and containers:** Google's device-code flow doesn't support Gmail scopes (to confirm in WP G.0), so there is no `--device-code` for Gmail. Options are an SSH port-forward to the loopback port, or signing in on a desktop and importing the encrypted token file (already a documented route for Microsoft tenants that block device code).
- **Refresh:** the same five-minute-early refresh as Microsoft, against `oauth2.googleapis.com/token`. Google allows about 100 live refresh tokens per client per account and silently drops the oldest after that, which only matters if someone signs in over and over.
- **Storage:** the existing keychain and encrypted-file stores, unchanged.

## Mail sync model

### Labels as folders

| Gmail | Familiar |
| --- | --- |
| `INBOX`, `SENT`, `DRAFT`, `TRASH`, `SPAM` | Folders with the well-known names `inbox`, `sentitems`, `drafts`, `deleteditems`, `junkemail` |
| User labels (`Clients/Acme` nests by `/`) | Folders, nested by the `/` in the name |
| `UNREAD` | `is_read = 0` |
| `STARRED` | `is_flagged = 1` |
| `IMPORTANT`, `CATEGORY_*` (Primary, Social, Promotions, Updates, Forums) | Not shown in V1, the way Focused Inbox isn't; kept as data so a filter or view can use them later |
| All Mail (no label of its own) | A virtual folder holding every message, so archived mail without a user label is still browsable |

A message with labels `INBOX` and `Clients/Acme` appears in both folders, counts as unread in both, and is still one message in search and in its thread.

| User action | Graph today | Gmail |
| --- | --- | --- |
| Archive | Move to Archive | Remove `INBOX` |
| Move from folder A to B | Move | Remove label A, add label B (moving out of Inbox removes `INBOX`) |
| Delete | Move to Deleted Items | `messages.trash` (Trash, kept 30 days) |
| Mark read, flag | PATCH | Remove `UNREAD`, add `STARRED` |
| Folder create, rename, move, delete | Folder API | Label create, rename (renaming the parent prefix moves the children), delete |

The existing op payloads (`MoveMessage`, `DeleteMessage`, `SetRead`, `SetFlag`, the folder ops) already describe these actions in folder terms, so they need no new variants. An explicit "add a label without moving" command is left out of the first cut; Graph has no equivalent, and it can come later if Marc misses it.

### First sync and incremental sync

1. **Labels:** `labels.list`, then the folder tree.
2. **Headers for all history:** `messages.list` over the whole mailbox (Spam and Trash included) gives IDs a page at a time, and `messages.get?format=metadata` fetches each header, labels and thread ID, sent in batch requests of up to 50. Record the profile's `historyId` before starting, so nothing that changes during the first sync is missed.
3. **Bodies for the window**, newest first: `messages.get?format=raw`, parsed locally (MIME parsing into the same HTML/plain body and attachment list the Graph provider produces).
4. **Incremental:** `history.list` from the stored `historyId` returns messages added and deleted, and labels added and removed, across the whole mailbox. At 2 quota units a call it can poll every 30 seconds for almost nothing. One bookmark per account (`history`) replaces Graph's one per folder.
5. **History too old:** Google keeps history for about a week, and sometimes less. A 404 from `history.list` means a full resync of headers: list all IDs, fetch what's new, drop what's gone, and refresh labels for the rest. This happens after a laptop has been off for a while, so it needs to be quick and quiet, not an error.

Moves are easy on Gmail: a message keeps its ID for life, and a move is just a label change, so there's nothing like Graph's move detection to do.

### Threads

Gmail's `threadId` goes into `threads.conversation_id`, so conversation view works with no new logic. Gmail threads by subject and references, which differs a little from Outlook's conversations, but each provider's own threading is the one the user sees on its own web client, so that is the right answer for each.

### Quota

Google's quota page lists 6,000 quota units per user per minute (some copies of the page say 15,000), so the plan budgets for 6,000 and G.0 reads the real figure from the Cloud console. `messages.get` costs 20 units, or 60 for the raw body, and `history.list` costs 2. Header-only first sync at 6,000 units a minute is about 300 messages a minute, so a 10,000-message mailbox takes around 35 minutes and a 100,000-message one around 5 to 6 hours; bodies for the window come after. That's much slower than Graph's paged header listing, but it runs once, the Inbox's newest page is fetched first so new mail shows at once, and the existing scheduler already puts user actions ahead of background work. Steady-state polling uses well under 1% of the limit.

The scheduler gains a quota-unit budget per account (a token bucket in units per minute) alongside its in-flight limit, and treats `429` and `403 rateLimitExceeded`/`userRateLimitExceeded` like Graph's throttling, backing off exponentially when Google sends no `Retry-After`. Google's quota page says charges for going over the limits may start later in 2026; staying inside the per-user limit, as the budget does, means that never applies.

### Server search

Older mail outside the body window is searched with `messages.list?q=`, which takes Gmail's own search syntax, so Familiar's query language (`from:`, `subject:`, `has:attachment`, `before:`, `after:`, `is:unread`, `folder:` as `label:`) translates almost one to one. Unlike Graph's `$search`, it has no 1,000-result cap and can be combined with other terms, so less refining happens locally.

### Push or poll

Gmail can push changes through Google Cloud Pub/Sub, but that needs a Pub/Sub topic in the user's project and another credential, which is extra setup for each user. Polling `history.list` every 30 seconds is cheap enough that push isn't worth it, matching the Graph decision.

## Sending, drafts, aliases and signatures

- **Compose and send:** Gmail takes a complete RFC 5322 message (`raw`), so Familiar builds the MIME itself: HTML and plain parts, inline images, attachments. Replies set `threadId`, `In-Reply-To` and `References`, so they join the thread. Attachments go up with the message (up to 35 MB through the upload endpoint), so there's no separate upload-session work as there is for Graph's 3 MB limit.
- **Drafts:** `drafts.create`, `drafts.update` and `drafts.send`, so drafts round-trip with Gmail on the web and on the phone, as they do with Outlook.
- **Undo send and the outbox** stay in `famd`, unchanged.
- **Aliases:** unlike Graph, Gmail lists send-as addresses (`users.settings.sendAs.list`), including each one's display name and whether it's verified. Familiar reads them at sign-in and on each folder-tree refresh, so `aliases = [...]` in config becomes optional for Gmail accounts. A send from an address that isn't a verified send-as is refused locally with a clear message, rather than letting Gmail rewrite the From line.
- **Signatures:** each send-as entry carries its HTML signature. Gmail has one signature per address, not separate new and reply ones, so WP 4.6's first-run migration can copy it into both `new` and `reply` for that account, and config remains the source of truth afterwards, as for Microsoft accounts.

## Calendar

The Google Calendar API maps onto the existing calendar tables:

- **Calendar list:** `calendarList.list`, including calendars shared with the account and subscribed ones (such as holidays). Read-only ones get `can_edit = 0`.
- **Events in the window:** `events.list` with `singleEvents=true` and `timeMin`/`timeMax` for the `[calendar] past` and `future` window, so the server expands recurring series as the design wants. Google's `syncToken` can't be combined with a time window, so each calendar is listed in full every 5 minutes as one round, using the `round` column the schema already has to drop events that disappeared. A typical personal calendar window is one or two pages, so this is a handful of requests every 5 minutes. If a busy shared calendar makes this slow, the syncToken route (masters and exceptions, expanded locally) is the upgrade, but it isn't needed to start.
- **Time zones:** Google uses IANA names everywhere, so no Windows-zone mapping is needed.
- **Writes:** `events.insert`, `patch` and `delete`; RSVP by patching the account's own attendee `responseStatus`; recurrence as RRULE lines, which is what Graph's patterns are converted from anyway. Invitations go out with `sendUpdates=all`, matching Outlook's behaviour.
- **Purposes:** `[calendar.purposes]` works unchanged, for example `family = { account = "gmail", calendar = "Family" }`.
- **Create event from email** works unchanged: the suggestion code reads the stored message, not Graph.

## Contacts

The People API gives two lists that map onto the existing tables: `people.connections.list` (the user's saved contacts, with a sync token for changes) and `otherContacts.list` (addresses Gmail saved automatically from past mail). Saved contacts go into `contacts`; other contacts join the local correspondents list that already feeds autocomplete. Contacts stay read-only, as in V1 for Microsoft.

## Rules (Phase 4)

Gmail filters (`users.settings.filters`) are the server rules for Gmail accounts. They match on from, to, subject, has-words and size, and can add or remove labels, archive, mark read, star, forward or trash, and they run on all incoming mail, not just the Inbox. `fam rules plan` and `apply` translate the same `[[rules]]` entries; conditions or actions a filter can't express fall back to `run = "local"`, as they do for Graph. Gmail filters can't be edited in place (only created and deleted), so `apply` replaces a changed filter. This waits for WP 4.3 and isn't part of the first Gmail cut.

## Configuration

```toml
[app.google]
# From your own Google Cloud project (see docs/gmail-setup.md).
client_id = "1234567890-abc.apps.googleusercontent.com"
client_secret = "GOCSPX-..."   # a desktop-app secret, not a real secret per Google

[accounts.gmail]
provider = "gmail"
email = "you@gmail.com"
# No tenant. Aliases are read from Gmail; list extra ones only if needed.
```

`fam config validate` requires `tenant` only for Graph accounts, refuses `tenant` on Gmail ones, and requires `[app.google]` once any Gmail account exists. `provider` defaults to `"graph"`, so existing configs don't change. `fam api --account gmail /gmail/v1/users/me/labels` goes to Google's base URL with that account's token.

## The provider boundary

The aim is the smallest boundary that lets `famd` run either provider, not a general plugin system. There are two providers and the set is closed, so an enum is simpler than trait objects:

```rust
// familiar-daemon (sketch)
enum Provider {
    Graph(GraphProvider),   // wraps MailSync, CalendarSync, ContactSync, MailApi, CalendarApi
    Gmail(GmailProvider),
}

impl Provider {
    async fn sync_tree(&mut self, w: &SharedWriter) -> Result<(), ProviderError>;
    async fn sync_mail(&mut self, w: &SharedWriter, due: Due) -> Result<Synced, ProviderError>;
    async fn fetch_bodies(&mut self, w: &SharedWriter, ids: &[MessageId]) -> Result<(), ProviderError>;
    async fn sync_calendars(&mut self, w: &SharedWriter, window: Window) -> Result<(), ProviderError>;
    async fn sync_contacts(&mut self, w: &SharedWriter) -> Result<(), ProviderError>;
    async fn run_op(&mut self, w: &SharedWriter, op: &Op) -> Result<OpResult, ProviderError>;
    async fn get(&self, path: &str) -> Result<Value, ProviderError>;   // fam api
}

enum ProviderError { Auth, Network, Throttled { retry_after }, NotFound, Refused(String), Server(String) }
```

- **What moves:** Graph's folder planning (Inbox first, Deleted Items and Junk last, one folder per round on the first sync) moves from the engine into `GraphProvider::sync_mail`. The engine keeps deciding *when* (Inbox every 30 s, everything every 5 min, calendars, ageing, backoff) and passes that as `Due`. Gmail ignores the per-folder detail, because one `history.list` call covers every folder.
- **What stays in the engine:** status reporting, backoff, the body window, ageing, pinning, the cache limit and the op queue's ordering and retries.
- **What stays in the providers:** request shapes, error mapping into `ProviderError`, and building provider JSON or MIME for ops. The Graph JSON currently built in `ops.rs` (attachments, events) moves into `provider-graph`.
- **Tests:** this step is a pure refactor and lands with every existing daemon test still passing unchanged against the Graph mock, before any Gmail code exists.

## Store change: messages in more than one folder

This is the riskiest change, because the store's speed comes from `messages.folder_id` and the triggers that keep folder counters and `folder_threads` up to date.

**Proposal:** a new `message_folders (message_id, folder_id, received_at)` table, with the existing triggers rewritten to maintain counters and `folder_threads` from it. Graph messages get exactly one row, so Graph behaviour is unchanged. Folder list views page over `message_folders (folder_id, received_at DESC, message_id DESC)`, the same keyset shape as today. `messages.folder_id` stays as the message's primary location (Drafts, else Inbox, else Sent, else its first user label, else All Mail) for anything that only needs one folder. That isn't only the reading pane's breadcrumb: core checks such as `need_draft()` in `crates/core/src/ops.rs` read the primary folder's well-known name, which is why Drafts comes first.

This goes through the same 500k benchmark as WP 0.5 and the GUI perf job before it ships: folder open, unread counts, scrolling and `fam mail list` must stay inside their targets for Graph-only stores. If they don't, the fallback is to keep `folder_id` paging for Graph and use `message_folders` only for Gmail accounts.

Search needs a small change: `folder:` matches any of a message's folders, which means indexing every folder name on the message rather than one.

## Testing

- **A mock Gmail server** (`familiar-mock-gmail`), like `familiar-mock-graph`: labels, messages (metadata and raw), history with expiry (404), batch requests, drafts, send, send-as, Calendar events with recurrence expanded, People connections, quota errors (429 and 403 `userRateLimitExceeded`), and expired or revoked tokens. Cloud threads develop entirely against it.
- **The existing daemon and CLI tests** run for both providers wherever behaviour is meant to match (moves, deletes, drafts, events, agent refusals), using the same harness with a provider parameter.
- **Live tests on Marc's computer**, as for Microsoft: read-only until the write work packages, then a `Familiar Test` label and sends only to Marc.
- **CI** stays as it is; the mock means no Google credentials in CI.

## Work packages

All of this is one new phase, **Phase G**, with its own gate. Each package is sized for one thread. Packages G.1, G.2 and G.5 can run in parallel once G.0 has passed.

| WP | Work | Depends on | Done when |
| --- | --- | --- | --- |
| G.0 | **Google project and sign-in test (Marc, about 15 minutes, with a guide from Claude).** Create a Google Cloud project, enable the Gmail, Calendar and People APIs, set up the consent screen as External and publish it to production without verification, create a desktop OAuth client. Then a throwaway spike (`spikes/google-auth`, guide in `docs/google-sign-in-test-guide.md`) signs in with PKCE and loopback, calls `users.getProfile` (which also gives the mailbox's message count), `labels.list`, `history.list`, `messages.list`, `sendAs.list`, `calendarList.list`, `people.connections.list` and `otherContacts.list`, and refreshes once. The consent screen also shows whether `gmail.settings.basic` is restricted | — | A results table in this file; the consent screen shows "In production" (so refresh tokens don't expire after 7 days); Marc has seen exactly what the warning screen looks like |
| G.1 | **Provider boundary.** The `Provider` enum and `ProviderError` in `famd`; Graph folder planning moved into `provider-graph`; Graph JSON for ops moved out of `ops.rs`; `fam api` sent to the account's provider | Phase 3 gate | Pure refactor: every existing test passes unchanged, and no behaviour changes on Marc's PC |
| G.2 | **Store: messages in many folders.** `message_folders` migration, triggers, keyset paging, `folder:` search on all folders, a virtual All Mail folder | Phase 3 gate | 500k benchmark and GUI perf job inside their budgets on a Graph-only store; multi-label tests pass |
| G.3 | **Google sign-in.** `familiar-auth` gains the Google flow from the spike: PKCE with loopback, refresh, token import for headless machines; `[app.google]` and `provider = "gmail"` in config and `fam config validate`; `docs/gmail-setup.md` | G.0 | `fam auth login --account gmail` works and survives a reboot on Marc's PC |
| G.4 | **Mock Gmail server** (`familiar-mock-gmail`), as listed under Testing | G.0 | Provider tests run offline in the cloud |
| G.5 | **Gmail mail read.** `familiar-provider-gmail`: labels to folders, first sync (metadata in batches, Inbox's newest first), bodies via raw MIME, `history.list` polling, history-expired resync, quota-unit scheduler, server search translation | G.1, G.2, G.3, G.4 | Passes against the mock, then a full read-only sync of Marc's Gmail with times, disk and memory recorded |
| G.6 | **Gmail mail writes.** Mark read, flag, archive, move, trash, label create/rename/delete through the existing op queue | G.5 | Passes against the mock, then in the `Familiar Test` label |
| G.7 | **Compose, drafts and send-as.** MIME building, replies in thread, drafts round-trip, attachments, send-as aliases read from Gmail, refusing unverified From addresses | G.6 | A send to Marc from each send-as address works; a draft made in Familiar opens in Gmail on the web |
| G.8 | **Google Calendar** (after the Phase G gate). Calendar list, window rounds with expanded events, writes, RSVP, recurrence, purposes, create from email | Phase G gate | A family event created from a real Gmail message shows in Google Calendar |
| G.9 | **Google contacts** (after the Phase G gate). Connections and other contacts into the existing tables for autocomplete | Phase G gate | `fam contacts search` finds Gmail contacts |
| G.10 | **GUI touches.** Labels shown as folders (a message in two labels appears in both), All Mail, a Gmail badge on the account, "Archive" wording for Gmail, send-as identities in compose | G.6, G.7 | A week of Gmail use in the app alongside the Microsoft accounts |
| G.11 | **Gmail filters as server rules** | G.6, WP 4.3 | `fam rules plan/apply` keeps Gmail filters in sync with config |

```
G.0 ──► G.3 ──┐
   └──► G.4 ──┤
G.1 ──────────┼──► G.5 ──► G.6 ──► G.7 ──► G.10
G.2 ──────────┘
Phase G gate ──► G.8, G.9
G.6 + WP 4.3 ──► G.11
```

**Phase G gate:** Marc's Gmail account syncs completely, search is fast, the mail gate tasks (find a thread, save a draft) work on it through `fam`, and Marc uses it in the desktop app for a week. Calendar and contacts (G.8, G.9) follow the gate; G.0 and G.3 still ask for their scopes, so adding them needs no second sign-in.

## Phasing

Gmail is on the design doc's "Later" list, and Phase 3's gate (a week of daily mail in the app) is in progress. Starting the provider boundary and the store change while that week runs would mean refactoring the code Marc is testing, so the proposed order is:

1. **Now:** WP G.0 only. It needs about 15 minutes of Marc's time and no Familiar code, and its answer (does the unverified production app work for restricted scopes, and do tokens last) decides whether the rest of the plan holds.
2. **After the Phase 3 gate:** G.1 and G.2, the two refactors, while Familiar is otherwise stable.
3. **Then:** G.3 to G.7 and G.10 (mail), before or alongside Phase 4, as Marc prefers. G.8 and G.9 (calendar and contacts) after the Phase G gate. G.11 waits for WP 4.3.

If Gmail matters more to Marc than Phase 4's calendar views and rules, Phase G can go straight after Phase 3; nothing in it depends on Phase 4 apart from G.11.

## Risks

| Risk | Effect | Response |
| --- | --- | --- |
| Google stops letting unverified production apps use restricted scopes, even for their own developer | Route B closes | G.0 tests it first. Fallbacks in order: an Internal app if the account is Workspace, app-password IMAP (a smaller provider: no calendar API, CalDAV instead), or verification with CASA if Familiar is going public anyway |
| The many-folders store change slows Graph accounts | Phase 3 performance targets regress | Benchmarked in G.2 before it ships; fallback keeps the single-folder path for Graph |
| History expires after a long time offline | A full header resync on wake | Designed in: listing IDs is cheap, and only new messages need fetching; labels refresh in the background |
| First sync of a large Gmail account is slow under the per-user quota | About 5 to 6 hours of headers for a 100k mailbox | Inbox first so it's usable at once; only headers before bodies; it happens once |
| Google starts charging for quota overruns later in 2026 | Cost if the limit is exceeded | The scheduler's unit budget keeps each account inside the per-user limit, so overruns don't happen |
| Gmail's threading differs from Outlook's | Some threads group differently across accounts | Accepted: each account shows its own provider's threads, matching its web client |
| Labels confuse folder-shaped commands (a move out of a view of All Mail, say) | Surprising results | Rules written down in this plan; moves from All Mail only add the target label |

## Decisions and open questions

**Answered by Marc (8 Nov 2026):**

- **Account:** one personal `@gmail.com` account. Route B (Marc's own unverified production project) applies; the Workspace "Internal" route doesn't.
- **Scope:** mail first; calendar and contacts later (G.8 and G.9 move after the Phase G gate).

**Still open:**

- **Public Gmail support?** Is "bring your own Google project" acceptable for the open source release, or should verification with CASA be budgeted for later?
- **Inbox categories:** show all Inbox mail (the plan's default), or only Primary, as Gmail does on the web?
- **When:** G.0 now and the rest after the Phase 3 gate, or a different order?
- **Mailbox size:** G.0 reports the message count, which sets the first-sync estimate.

## Sources

Checked on 7 November 2026. Some figures come from third-party pages and are marked as such; G.0 confirms the ones that matter.

- [Gmail API scopes](https://developers.google.com/workspace/gmail/api/auth/scopes) (Google), with the tiers cross-checked against [EmailEngine's scope reference](https://learn.emailengine.app/docs/accounts/gmail/gmail-api-scopes) and [Nylas's scope guide](https://developer.nylas.com/docs/cookbook/use-cases/build/google-oauth-scopes/)
- [Gmail API usage limits](https://developers.google.com/workspace/gmail/api/reference/quota) (Google)
- [Unverified apps and the user cap](https://support.google.com/cloud/answer/15549945) (Google) and [Workspace rules for unverified apps](https://knowledge.workspace.google.com/admin/apps/authorize-unverified-third-party-apps) (Google)
- Testing-mode refresh tokens expiring after 7 days: [Google Ads API forum](https://groups.google.com/g/adwords-api/c/QTdeilIh_QY/m/eOHg0Q9CAgAJ) (forum answers by Google's advisors, not Google documentation)
- CASA assessment cost and renewal: [Nango's Google security review notes](https://nango.dev/docs/api-integrations/google-shared/google-security-review), [Switch Labs' CASA price comparison](https://www.switchlabs.dev/post/casa-tier-2-tier-3-security-review-providers-pricing-and-the-cheapest-option) and [MailMate's experience of CASA Tier 2](https://lists.freron.com/mailmate/2025-April/018278.html) (vendors and a developer, not Google)
- [OAuth 2.0 for TV and limited-input devices](https://developers.google.com/youtube/v3/guides/auth/devices) (Google; points to the allowed-scopes list, which G.0 checks for Gmail)
