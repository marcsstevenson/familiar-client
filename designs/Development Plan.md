# Familiar Development Plan

3 Nov 2026 · Companion to [Email Client Design Document](Email%20Client%20Design%20Document.md)

## Summary

Familiar is built in six gated phases. Each phase ends with a gate that Marc approves before the next one starts, and each phase leaves something useful behind even if the project stops there.

| Phase | Goal | Gate (Marc approves) |
| --- | --- | --- |
| **0. Prove it can work** | Sign-in on all three accounts, UI toolkit choice, performance at 500k messages, signature API check | iM3 Dental sign-in answered; Tauri or GPUI chosen; perf targets look reachable |
| **1. Read-only headless engine** | `famd` + `fam` sync, store and search real mail, read-only | All three accounts (or those that sign in) sync fully and search instantly from the CLI |
| **2. Writes and agent parity** | Drafts, send, organise, calendar writes, MCP, agent permissions | Past Composio tasks work through `fam`: find a thread, save a draft, create a Family Room event from an email |
| **3. Desktop mail** | Tauri app for daily mail | Marc uses Familiar for mail on one account for a week |
| **4. Calendar, rules, contacts and migration** | Remaining V1 features in the GUI | Every V1 row in the design doc's requirements table is done |
| **5. Parallel run and release** | Packaging, reinstall test, open source release | Two weeks with Outlook closed; reinstall from `config.toml` works; Outlook retired |

The work is split into **work packages** (WP) below. Each one is sized for a single thread, names what it depends on, and says what "done" means, so a later thread can pick one up from this file alone.

## Ground rules for every thread

- **Repo:** `familiar-client`, commit and push straight to `main` (no branches or PRs) once the work is in a good state. Commits and pushes use the `marcsstevenson` identity (see project memory).
- **Live Microsoft access** happens only on Marc's computer. Cloud threads use the mock Graph server (WP 1.1) and synthetic mail.
- **Real accounts stay read-only until Phase 2.** Write tests then use a `Familiar Test` folder and send only to Marc.
- **No personal data in the repo.** No real mail, tokens, addresses or signatures. Secret scanning runs in CI from WP 1.0.
- **Marc's jobs:** register the Microsoft app, do every sign-in, and approve each gate.

## Phase 0: Prove it can work

Phase 0 answers the questions that could change the design. The iM3 Dental sign-in test runs first, because it decides whether iM3 can ever leave its current connector. The other Phase 0 packages can run in parallel once 0.1 has started.

### WP 0.1 Register the Microsoft app (Marc, with a guide from Claude)

- **Who:** Marc does the clicks; Claude writes the step-by-step guide.
- **What:** In the Entra admin centre under the Parklands tenant (where Marc is admin), register a public client app named "Familiar":
  - Supported accounts: *any organisational directory and personal Microsoft accounts*.
  - Platform: *Mobile and desktop*, redirect URI `http://localhost` (loopback, any port).
  - "Allow public client flows" on, so device code works.
  - Delegated permissions: `User.Read`, `offline_access`, `Mail.ReadWrite`, `Mail.Send`, `Calendars.ReadWrite`, `Contacts.Read`, `MailboxSettings.ReadWrite`.
  - Grant admin consent for the Parklands tenant.
- **Done when:** the client ID is recorded (it's public, so it can go in the repo's example config).
- **Outcome (2026-11-03):** the app is registered in Marc's own Azure directory instead of the Parklands tenant. Parklands admin consent link is in the guide (Part 1 step 7), not yet done. The sign-in steps are in `docs/sign-in-test-guide.md`.
- **Done (2026-11-04):** Application (client) ID `c6057b6a-a692-47dd-b34e-12629372e32a` (public, not a secret). It goes in the example config when that exists (WP 1.3).

### WP 0.2 iM3 Dental sign-in test (the first real test)

This runs in two steps, cheapest first.

**Step A: consent probe, no code.** Claude gives Marc an authorise URL for the `organizations` endpoint with the full scope list. Marc opens it in a private browser window and signs in as his iM3 Dental user. The screen that comes back answers the question:

| What Marc sees | Meaning | Next step |
| --- | --- | --- |
| A normal consent prompt listing the permissions | Users can consent; iM3 will work | Go to step B |
| "Need admin approval" | User consent is blocked for this app | Try the fallbacks below |
| "Your sign-in was blocked" or a Conditional Access error | A policy blocks unknown apps or devices | Record the error code and go to the fallbacks |

Fallbacks, tried in order and each recorded:

1. **Read-only scopes:** retry with only `User.Read offline_access Mail.Read Calendars.Read`. Some tenants allow user consent to low-impact permissions only. If this works, iM3 can be read and searched but not written.
2. **Device code:** retry through device code, in case the tenant treats it differently (the risk table notes some tenants block it).
3. **Ask the iM3 admin** for consent, with a one-page explanation Claude drafts. Marc decides whether to send it.
4. **Publisher verification** of the app, which some tenants require for user consent. This needs a Microsoft Partner ID and is worth doing for open source users anyway.
5. **Accept the block:** iM3 stays on Outlook on the web and its current connector, and Familiar ships for Hotmail and Parklands.

**Chosen 2026-11-03:** fallback 5. User consent was refused for both the full and the read-only scopes, and Marc decided not to send the admin request for now (see the Results log).

**Step B: token spike.** A throwaway Rust binary (`spikes/auth`) that:

- signs in with authorisation code + PKCE through the system browser, and separately through device code;
- calls `/me`, `/me/mailFolders?$top=5`, `/me/messages?$top=1` and `/me/calendars`;
- refreshes the token once to prove `offline_access` works;
- runs against all three accounts (Hotmail on `consumers`, Parklands and iM3 on `organizations`).

Tokens are printed as "ok/failed" only and never written to disk.

- **Runs on:** Marc's computer.
- **Done when:** a results table (account × flow × scope set → result + error code) is added to this file, and project memory is updated with the iM3 answer.

### WP 0.3 Signature API recheck

- Check current Graph docs and the beta endpoint for any signature or roaming-settings API on both consumer and work accounts. Try `GET /me/mailboxSettings` on each account during WP 0.2 step B and note what comes back.
- **Done when:** the design doc's signature section is confirmed or corrected.

### WP 0.4 UI toolkit bake-off: Tauri 2 vs GPUI

- Build the same tiny prototype twice: a three-pane window with a virtualised list of 500k synthetic rows, and a reading pane showing a sanitised HTML newsletter in a sandboxed frame.
- Measure on Windows (primary), plus macOS and Linux where available: cold start, idle memory, scroll frame times, keypress latency, HTML render time.
- Note the developer-experience side too: how easily Claude can drive and screenshot each one in CI.
- **Done when:** a results table and a recommendation are added here, and Marc chooses.

**Status (4 Nov 2026):** done. **Marc chose Tauri 2 + SolidJS on 4 Nov 2026.** Prototypes, harnesses and results are in `spikes/ui-bakeoff/`; full write-up in [RESULTS.md](../spikes/ui-bakeoff/RESULTS.md). **Recommendation: Tauri 2 + SolidJS.** On Windows it met every speed target. Every option that can show HTML mail misses the 150 MB memory target, so the recommendation is to raise that target to about 400 MB for the GUI. GPUI's native HTML view can't lay out a real newsletter. With an embedded webview, GPUI used nearly as much memory as Tauri and was slower to start and to open messages.

| Windows (RTX 4080, 165 Hz), 500k rows | Target | GPUI, native HTML | GPUI + webview | Tauri 2 |
| --- | --- | --- | --- | --- |
| Cold start, median | < 500 ms | 514 ms | 663 ms | 442 ms |
| Idle memory, private bytes | < 150 MB | 142 MB | 351 MB | 399 MB |
| Scroll frame p95 | 16.7 ms | 6.4 ms | 6.3 ms | 6.2 ms |
| Real keypress to frame p95 | < 16 ms | 9.2 ms | 6.7 ms | 11.0 ms |
| Open message p95 | < 50 ms | 6.1 ms (simplified render) | 68 ms | 30 ms |
| HTML newsletter | faithful | broken layout | faithful | faithful |

Linux numbers (cloud, no GPU) are in RESULTS.md. There, GPUI started and responded faster, and the GPUI + webview build lost keyboard input to the webview.

### WP 0.5 Store and search at 500k messages

- Synthetic mail generator (`tools/synth`) producing realistic headers, threads across folders and bodies, with a fixed seed.
- Prototype the SQLite (WAL) schema and a Tantivy index; load 500k messages.
- Measure: first page of a folder, search first page, thread assembly, `list --json` equivalent, index size, DB size.
- **Done when:** numbers are compared against the design doc's targets, with any target that looks out of reach called out.

### Phase 0 gate

Marc reviews the WP 0.2 to 0.5 results and decides: which accounts are in scope, Tauri or GPUI, and whether the performance targets stand. The design doc is updated with the outcomes.

**Approved by Marc on 2026-11-04.** Accounts in scope: Hotmail and Parklands (iM3 Dental omitted). UI: Tauri 2 + SolidJS. Performance targets stand, with the GUI idle memory target raised to 400 MB private. Phase 1 is open.

## Phase 1: Read-only headless engine

The goal is a daemon and CLI that keep a complete, correct, read-only local copy of every account that can sign in. No GUI yet.

| WP | Work | Depends on | Done when |
| --- | --- | --- | --- |
| 1.0 | **Workspace and CI.** Cargo workspace with `core`, `provider-graph`, `store`, `search`, `rules`, `daemon`, `cli`, `gui` (stub). CI on Windows, macOS, Linux; the non-GUI crates build on a runner with no display libraries; clippy, fmt, tests, secret scanning, licence headers | Phase 0 gate | Empty crates build and CI is green |
| 1.1 | **Mock Graph server.** A local server that replays recorded and synthetic responses for folders, messages, delta, `$search`, calendars, throttling (429 + `Retry-After`) and auth errors | 1.0 | Provider tests run fully offline in the cloud |
| 1.2 | **Command registry.** Typed command definitions in `core`; generators for CLI args, JSON schema (`fam schema --json`), MCP tool list and TS bindings; the parity test that fails CI if a command is missing from a surface; stable exit codes | 1.0 | One sample command appears on every surface and the parity test catches a deliberately missing one |
| 1.3 | **Config.** `config.toml` parsing and validation (`fam config validate`), comment-preserving write-back, example config in the repo | 1.2 | Round-trip test keeps comments and ordering |
| 1.4 | **Auth.** PKCE and device code from the spike, made production: token storage in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service), encrypted-file fallback for containers, refresh, `fam auth login/status/logout` | 1.2, WP 0.2 | Sign-in survives restart on Marc's machine for each in-scope account |
| 1.5 | **Store.** SQLite schema (accounts, folders, messages, threads, sync state, op queue), migrations, body files keyed by content hash, the single-writer rule | 1.0, WP 0.5 | Schema and migrations tested; readers don't block the writer |
| 1.6 | **Graph provider: mail read.** Folder tree, header paging, body fetch, per-folder delta with saved links, move detection by immutable ID and Internet message ID, throttling scheduler (4 concurrent, `Retry-After`, user actions first) | 1.1, 1.4, 1.5 | Passes against the mock, including moves and throttling |
| 1.7 | **Sync engine and daemon.** `famd`: first sync (headers for all history, then bodies for the window, newest first), incremental polling (Inbox 30 s, others 5 min), window ageing, pinning, body cache limit, local socket or named pipe, `fam sync status` | 1.6 | A synthetic 500k mailbox syncs from the mock and stays consistent |
| 1.8 | **Threads.** Local conversation assembly from `conversationId` across all folders | 1.7 | Inbox + Sent Items threads assemble correctly in tests |
| 1.9 | **Search.** Tantivy index, query language (`from:`, `subject:`, `has:attachment`, `before:`, `after:`, `folder:`, `account:`, `is:unread`), server `$search` for older bodies merged and labelled online, offline notice | 1.7 | Search targets from WP 0.5 met; online results labelled |
| 1.10 | **CLI read commands.** `fam mail list/search/thread/fetch/pin`, `fam folder list`, `fam api` escape hatch (GET only in this phase), JSON output to the schema | 1.2, 1.7–1.9 | Every command has `--json`, tests and documentation |
| 1.11 | **Calendar read.** Calendars and events by delta for a rolling window, IANA ↔ Windows time-zone mapping, `fam cal list` | 1.6, 1.7 | Family Room events list correctly on Marc's machine |
| 1.12 | **Consistency check.** Store versus server comparison that reports drift without fixing it | 1.7 | Runs clean after a full sync of each real account |
| 1.13 | **Live read-only run (Marc's computer).** Full sync of each in-scope account; measure time, disk and memory | 1.4–1.12 | Results recorded here |

**Phase 1 gate:** Marc runs `fam` against his real accounts for a few days, read-only, and confirms sync is complete and search is fast.

## Phase 2: Writes and agent parity

The goal is a CLI that can replace Composio and the Microsoft 365 connector for mail and calendar.

| WP | Work | Depends on | Done when |
| --- | --- | --- | --- |
| 2.1 | **Durable op queue.** Local-first writes with optimistic store updates, idempotent retries, restart survival, operation IDs and `--wait`, `--dry-run` on every write | Phase 1 | Kill-and-restart tests lose nothing |
| 2.2 | **Mail writes.** Mark read, flag, move, archive, delete to Deleted Items, folder create/rename/move/delete | 2.1 | Passes against the mock, then in the `Familiar Test` folder |
| 2.3 | **Compose and send.** Drafts (create, update, attachments, inline images), reply/reply all/forward, sending identity, per-account new/reply signatures from config, undo-send delay, outbox while offline, warning for many visible outside recipients | 2.1, 1.3 | Sends to Marc only work from each account |
| 2.4 | **Calendar writes.** Create, edit, delete, RSVP, recurrence (server-expanded), reminders, purposes (`--purpose family`), `fam cal add --from-mail` with link back and suggested details | 2.1, 1.11 | A Family Room event is created from a real email |
| 2.5 | **Contacts.** Read and search Outlook contacts plus a local list of past correspondents, for autocomplete | 1.6 | `fam contacts search` works |
| 2.6 | **Agent permissions and audit log.** `[agent] allow` enforced by `famd`, per-account grants for send, delete, rules and forward, audit log of every automated write with its source, non-interactive mode with `--yes` rules | 2.1 | Denied actions fail with the documented exit code and are logged |
| 2.7 | **MCP server and local JSON-RPC API.** Generated from the registry; `fam mcp serve --allow read,draft` | 1.2, 2.6 | Claude Code can use it through MCP and through the shell |
| 2.8 | **Agent skill.** The shipped skill describing common workflows, built from `fam schema --json` | 2.7 | Claude completes the gate tasks using only the skill |
| 2.9 | **Gate tasks on real accounts.** Find a thread, save a draft, create a Family Room event from an email, all through `fam` | 2.1–2.8 | Marc confirms each task |

**Phase 2 gate:** the WP 2.9 tasks pass. At this point Marc can switch Claude from Composio to `fam` for mail and calendar.

## Phase 3: Desktop mail

Tauri 2 + SolidJS was chosen in Phase 0 (WP 0.4).

| WP | Work | Depends on | Done when |
| --- | --- | --- | --- |
| 3.1 | **Shell and design tokens.** Tauri app, custom title bar per OS, light/dark/system themes, generated TS bindings, daemon auto-start | Phase 2 | App opens to an empty three-pane layout |
| 3.2 | **Folder tree and message list.** Unified and per-account views, virtualised list, sorting in Rust, rows-on-screen only | 3.1 | 60 fps scrolling at 500k |
| 3.3 | **Reading pane.** Rust HTML sanitiser, sandboxed frame with strict CSP, remote images blocked with per-message and per-sender overrides, link destinations shown, attachment preview and save, prefetch of likely next messages, HTML test set | 3.1 | Test set renders safely; open-message target met |
| 3.4 | **Compose window.** Rich and plain text, inline images, drag-and-drop attachments, autocomplete, identity and signature switching | 3.1, 2.3 | Compose round-trips with the CLI's drafts |
| 3.5 | **Command palette and keyboard.** Every command reachable, showing its keybinding and CLI equivalent; single-key actions rebindable from config; `fam ui` commands | 3.2 | Every registered command is in the palette (checked in CI) |
| 3.6 | **Search UI and smart folders.** Query bar, online results labelled, saved searches from `[[views]]` | 3.2, 1.9 | Search target met in the GUI |
| 3.7 | **Performance and accessibility CI.** Automated checks for the design doc's targets; screen reader labels; WCAG 2.1 AA contrast | 3.2–3.6 | CI fails on a regression |

**Phase 3 gate:** Marc uses Familiar for mail on one account for a week.

## Phase 4: Calendar, rules, contacts and migration

| WP | Work | Depends on | Done when |
| --- | --- | --- | --- |
| 4.1 | **Calendar views.** Day, week, month and agenda, multiple calendars together, event editor, RSVP | Phase 3, 2.4 | Daily calendar use works |
| 4.2 | **Event from email panel.** Side panel with that day's events while creating from a message | 4.1 | Clashes are visible while creating |
| 4.3 | **Server rules as code.** `fam rules plan/apply/test`, ordering, enable/disable, Junk-folder rule example | 2.1 | Config and server stay in sync |
| 4.4 | **Local rules.** Conditions the server can't express, run by `famd`, clearly labelled | 4.3 | Junk sender clean-up rule works locally |
| 4.5 | **Rules and contacts in the GUI** | 4.3, 2.5 | Editable from the GUI, written back to config |
| 4.6 | **First-run migration.** Import server rules and folder favourites into config; guided one-time signature copy | 4.3, 1.3 | A fresh config is generated from a real account |

**Phase 4 gate:** every V1 requirement in the design doc is done, checked row by row.

## Phase 5: Parallel run and release

| WP | Work | Depends on | Done when |
| --- | --- | --- | --- |
| 5.1 | **Packaging.** winget, Homebrew, deb/rpm/AppImage, OCI image with `famd` as the main process | Phase 4 | One-command install on each OS |
| 5.2 | **Code signing and publisher verification** (Marc supplies certificates and Partner ID) | 5.1 | Signed installers; verified publisher in Entra |
| 5.3 | **Reinstall drill.** Wipe, install, restore `config.toml`, sign in, sync | 5.1 | Working install with nothing else restored |
| 5.4 | **Parallel run.** Two weeks of daily use with Outlook closed; daily consistency checks | 5.1 | No lost or misfiled mail |
| 5.5 | **Open source release.** README, contributing guide, security policy, docs site, public HTML test set | 5.3 | Public release tagged |

**Phase 5 gate:** Marc retires New Outlook on the desktop.

## Ordering and parallel work

```
Phase 0:  0.1 ──► 0.2A ──► 0.2B ──► 0.3
                   │
          0.4 ─────┼──────────────────┐   (0.4 and 0.5 start any time, in the cloud)
          0.5 ─────┘                  ▼
                                 Phase 0 gate
Phase 1:  1.0 ─► 1.1, 1.2, 1.5 ─► 1.3, 1.4, 1.6 ─► 1.7 ─► 1.8, 1.9, 1.11, 1.12 ─► 1.10 ─► 1.13
Phase 2:  2.1 ─► 2.2, 2.3, 2.4, 2.6 (parallel) ─► 2.7 ─► 2.8 ─► 2.9
Phase 3:  3.1 ─► 3.2, 3.3, 3.4 (parallel) ─► 3.5, 3.6 ─► 3.7
Phase 4:  4.1 ─► 4.2;  4.3 ─► 4.4, 4.5, 4.6
Phase 5:  5.1 ─► 5.2, 5.3, 5.4 ─► 5.5
```

Workstreams that can run as separate threads at the same time:

- **Auth and Microsoft setup** (0.1, 0.2, 1.4): needs Marc's computer.
- **Engine** (1.5–1.9, 2.1): cloud, against the mock.
- **Surfaces and parity** (1.2, 1.10, 2.6–2.8): cloud.
- **UI** (0.4, then Phase 3): cloud, with Windows checks on Marc's computer.
- **Calendar** (1.11, 2.4, 4.1–4.2).
- **Rules and migration** (4.3–4.6).

## Suggested first threads

1. **"Test iM3 Dental sign-in"**: WP 0.1 guide and WP 0.2 step A. This is the next thing to do and needs about 15 minutes of Marc's time.
2. **"Compare Tauri and GPUI"**: WP 0.4, can start in the cloud straight away.
3. **"Benchmark store at 500k messages"**: WP 0.5, including the synthetic mail generator.

## Results log

Results are recorded here as they come in.

| WP | Date | Result |
| --- | --- | --- |
| 0.2 | 2026-11-03 | Step A: app registered in Marc's own Azure directory (not Parklands). iM3 Dental user, `organizations` endpoint: full scopes → "Need admin approval"; read-only scopes (`User.Read offline_access Mail.Read Calendars.Read`) → "Need admin approval". User consent is blocked in iM3 for this app. Device code skipped: consent policy is per tenant, not per flow, so it gives the same answer. Decision (Marc, 2026-11-03): omit iM3 for now. The admin request is not sent; it's kept in `docs/im3-admin-consent-request.md` for later. Familiar targets Hotmail and Parklands; iM3 stays on Outlook web and its current connector. |
| 0.2 | 2026-11-04 | Step B: auth spike run on Marc's PC, full scopes, iM3 omitted. Hotmail passes on both flows. Parklands passes on PKCE; device code signed in and read fine, but its refresh against `organizations` failed (AADSTS65001). Refreshing against the tenant ID worked on PKCE. Results table below. |
| 0.3 | 2026-11-04 | Partial: `GET /me/mailboxSettings` on Hotmail and Parklands returns `archiveFolder, automaticRepliesSetting, dateFormat, delegateMeetingMessageDeliveryOptions, language, timeFormat, timeZone, userPurpose, workingHours`. No signature field on either. Graph docs and beta endpoint still to check. |
| 0.4 | 2026-11-04 | Windows and Linux measured; see the WP 0.4 table and `spikes/ui-bakeoff/RESULTS.md`. Recommendation: Tauri 2 + SolidJS (meets start, keypress, open and scroll targets on Windows). No webview design meets the 150 MB GUI memory target, so raise it to about 400 MB. Marc chose Tauri 2 + SolidJS (2026-11-04). |
| 0.5 | 2026-11-03 | Store and search meet the targets at 500k messages on a 4-vCPU cloud VM; most operations are under 1 ms at p95. Three design changes for WP 1.5/1.9: store folder counters (a recount takes 0.86 s), keep the one-year body index (phrase search over all bodies is 109 ms p95), and pack small bodies (one file each uses 2.0 GB on disk for 510 MB). Details: [WP 0.5 Store benchmark](WP%200.5%20Store%20benchmark.md) |
| Gate | 2026-11-04 | Phase 0 gate approved by Marc: Hotmail + Parklands in scope (iM3 omitted), Tauri 2 + SolidJS, performance targets stand with GUI idle memory raised to 400 MB. |
| 1.0 | 2026-11-04 | Cargo workspace (`crates/`: core, provider-graph, store, search, rules, daemon → `famd`, cli → `fam`, gui stub) and CI (`.github/workflows/ci.yml`): clippy and tests on Windows, macOS and Linux; headless build and GUI-dependency check in a `rust:1-slim` container with no display libraries; rustfmt; SPDX licence headers; gitleaks secret scanning. |
| 1.1 | 2026-11-04 | Mock Graph server in `crates/mock-graph` (`familiar-mock-graph`, plus a `mock-graph` binary). Serves profile, `mailboxSettings`, folders and folder delta, messages with `$top/$skip/$select/$filter/$orderby/$count`, per-folder message delta (moves as `@removed` + arrival, REST IDs that change on move, `IdType="ImmutableId"`), `$search` (1,000 cap, refused with `$filter`/`$orderby`), calendars, events, `calendarView` and its delta. Failures: 429 + `Retry-After`, 4-concurrent `MailboxConcurrency` limit, request budget, 503, 401 expired/missing tokens, 403, 410 lost sync state, delays. Data from JSON fixtures, deterministic synthetic mailboxes (20k-message full delta sync in the tests) and replayed recordings. 31 tests, all offline on 127.0.0.1. GET only; writes come with Phase 2. See `crates/mock-graph/README.md`. |
| 1.2 | 2026-11-04 | Command registry in `familiar-core` (`registry.rs`): each command is a Rust type with typed input/output (serde + schemars), access level and error kinds. Generated from it: `fam` subcommands and flags (`familiar-cli`), `fam schema --json`, the MCP tool list (`fam schema --format mcp`) and TS bindings (`bindings/commands.ts`, checked with `tsc --strict` in CI). Stable exit codes 0–12 (`docs/exit-codes.md`). Sample commands `fam version` and `fam schema`. `crates/cli/tests/parity.rs` fails CI when a command is missing from any surface or generated files are stale; tests show it catches a dropped command on each surface and a new command with stale bindings. Guide: `docs/command-registry.md`. |
| 1.3 | 2026-11-04 | Config in `crates/config` (`familiar-config`): typed `config.toml` model with unknown keys rejected, validation that reports every problem with line and column (account names, emails, tenants with a warning for `organizations`/`common`, references from rules and calendar purposes, periods, sizes, duplicate views and keys, `agent.allow` kept in step with the registry's access levels), path resolution (argument, `FAMILIAR_CONFIG`, platform default), and `ConfigDocument` for comment-preserving write-back with atomic saves. Commands `fam config validate [path]` (exit 3 on errors) and `fam config path`. Example at `config.example.toml` (Hotmail on `consumers`, a placeholder org tenant, signatures, no real addresses); guide `docs/config.md`. Round-trip tests: the unedited example and a CRLF copy come back byte for byte, editing two values changes exactly those two lines, a signature edit changes only its lines, new keys and tables append without reordering. |
| 1.5 | 2026-11-04 | Store in `crates/store` (`familiar-store`), built from the WP 0.5 prototype. SQLite in WAL mode; schema v1 in `migrations/0001_initial.sql`: accounts, folders with stored `total`/`unread`, threads (conversation ID stored once, messages refer to it by integer), messages, `folder_threads`, bodies, sync state (delta links per account and resource) and the op queue. Migrations are append-only, tracked in `user_version`, one transaction per step; newer stores are refused. Counters and thread rows are kept by triggers, and a randomised test checks they always equal a full recount (`Writer::repair_derived` also fixes drift). Bodies: zstd, named by BLAKE3 of the raw bytes, deduplicated; those that compress to 4 KB or less stay inline in SQLite (WP 0.5 finding), larger ones in `bodies/ab/cd/<hash>.zst`, written atomically and checked against the hash on read; `gc_bodies` removes unreferenced rows and orphan files. Single writer: `Writer::open` takes an exclusive lock on `store.lock` and is the only path that migrates or writes (a second writer, in-process or in another process, gets `WriterLocked`); `Reader` uses a read-only connection and keyset paging. Op queue: idempotency keys, ops on one target run in order, `Retry-After` via `not_before`, in-flight ops return to the queue on restart, op and local change commit together. 33 tests. Readers don't block the writer: commits go through while a reader holds a snapshot, reads go through while the writer holds an open transaction, and with 4 readers paging continuously the writer loads 3,000 messages at about 3,600/s with a worst commit of 30 ms (release, cloud VM). Loading 100k messages with bodies through the writer runs at about 11,600/s (`tests/load.rs`, ignored by default); folder pages are index range scans (0.14 ms for 50 rows at 100k). |
| 1.6 | 2026-11-04 | Graph mail read in `crates/provider-graph` (`familiar-provider-graph`), passing against the mock. `GraphClient` (reqwest + rustls) always sends `Prefer: IdType="ImmutableId"`, takes tokens from a `TokenSource` (`AuthTokens` wraps WP 1.4's `Auth`), refreshes once on 401 and refuses links to other hosts. `Scheduler` per mailbox: at most 4 in flight, `Priority::User` waiters before `Background`, and a 429/503 pauses the whole mailbox for `Retry-After` (doubling backoff when absent, 6 attempts). `MailApi`: folder tree by `childFolders`, well-known folder lookup, header pages newest first, bodies, folder and message delta pages. `MailSync` writes through the store's `Writer`: folder-tree delta (parents first, well-known names kept), per-folder message delta with the `nextLink` saved mid-round and the `deltaLink` at the end (`sync_state` resources `folders` and `messages:<id>`), 410 restarts with a full round that reconciles vanished messages. Moves: removals are held until all folders in the call are read; a message still in the reporting folder is looked up by immutable ID and moved locally (body kept) or deleted on 404; when the ID changed, the Internet message ID carries the body to the new copy. Tests: 24 (scheduler units, 8 sync and 6 throttling tests against the mock: moves both ways, ID-changing moves, 410, peak 4 in flight with no 429s, Retry-After honoured mailbox-wide, request budget ridden out, user request overtaking 36 queued background ones, 401 refresh). Not done here: when to sync (WP 1.7) and real-account runs (WP 1.13) |
| 1.7 | 2026-11-04 | Sync engine and daemon in `crates/daemon` (`familiar-daemon`, bin `famd`); guide in `docs/sync.md`. One task per account: folder tree every 5 min; first sync one folder at a time, Inbox first (newest page of headers stored before its delta round), Deleted Items and Junk last, headers for all history, recorded as done in `sync_state` `engine:first-sync`; then polling Inbox every 30 s and other folders every 5 min in one call so moves settle locally; window bodies newest first in batches of 64 after any round with changes; hourly ageing (bodies out of the window dropped unless pinned or cached, `sync.headers` pruning, cache trimmed to `sync.cache_limit` least recently used first, body gc); failures back off 30 s doubling to 10 min. Store: schema v2 (`body_cached_at`, account/time index), `SharedWriter` so the engines and requests share the one writer a transaction at a time, and `MailSync` now takes any `StoreWriter`; familiar-store no longer depends on familiar-core. Socket: per-user Unix socket (0600) or named pipe, JSON-RPC 2.0 one object per line (`daemon.status`, `mail.pin`, `mail.fetch_body`, `sync.now`, `daemon.stop`); protocol and a blocking client in `familiar_core::daemon`, so commands stay synchronous. New commands: `fam sync status` (store counts even without famd, live phase from famd) and `fam mail pin <id> [--unpin]` (exit 11 without famd). Done criterion met against the mock: 500,000 synthetic messages (3 years, 30-day window) first-synced in 80 s (headers for all history in 76 s, 13,257 window bodies), store matched the server exactly (every message in the right folder with the right read state, counters and threads drift-free, bodies present for exactly the window), then 5,000 changes (new mail, moves, deletes, read flips) were applied and consistent again in 17 s. With the default 1-year window: 166,400 bodies, first sync 131 s, 34.5 MB of bodies stored. Release build on a 4-vCPU cloud VM. CI runs the same check at 20,000 messages plus tests for ageing, pinning, the cache, the socket, `fam sync status` and restart without a full round. Not done here: real-account runs (WP 1.13), pausing sync while the user types (Phase 3). |
| 1.9 | 2026-11-05 | Search in `crates/search` (`familiar-search`); guide in `docs/search.md`. Query language: words, `"phrases"`, `from:`, `to:`, `subject:`, `body:`, `has:attachment`, `before:`/`after:` (dates or ages such as `30d`), `folder:` (name, path or well-known), `account:`, `is:unread/read/flagged`, and `-` to exclude any part; malformed values are usage errors. Tantivy index in `search/` beside the store (ID, account, folder, date, attachment as fast fields; subject, sender, recipients, body text with an accent-folding tokenizer); rows are always read from SQLite. Store schema v3 adds `search_queue`, filled by triggers on every indexed change, so the index follows any writer; `famd` drains it every 2 s in commits of 2,000 and rebuilds a missing or outdated index at start. Read and flag state stay out of the index (partial SQLite indexes hand Tantivy the ID set). `fam mail search` (new command) works without famd; `--scope all` sends the words to Graph `$search` through famd (new `mail.search_online` socket method, user priority), applies dates/folders/accounts/attachments/read state locally, drops what the local index would find, and merges the rest labelled `online`; `online` reports `searched`, `offline`, `daemon_unavailable` and so on, with a notice that older message text wasn't searched. Done criterion met: WP 0.5 mailbox (500,000 messages, seed 42, one-year body window) loaded through the store writer in 103 s and indexed from the queue in 17 s (73 MB index); every search case is under the 100 ms target at p95, 50 newest hits with rows: common word 2.3 ms, mid 0.45, rare 0.33, two words 0.66, phrase of two very common words 11.7, `from:` 0.60, `subject:` 0.35, word + largest folder 2.0, word + attachment 2.4, word + one-year range 1.8, word + `is:unread` 35, open store and index then search 3.8 (release, 4-vCPU cloud VM; `tests/scale.rs`, 10,000 in CI). End-to-end tests against the mock cover local results, merged online results with filters, the offline and no-daemon notices, and new mail becoming searchable. Not done here: local time zones for dates (UTC for now), GUI search (WP 3.6). |
| 1.12 | 2026-11-05 | Consistency check: `fam sync check [--account] [--limit] [--no-settle]`, which calls famd's `sync.check` (`crates/daemon/src/check.rs`; guide in `docs/sync.md`). It reads only and fixes nothing. It asks the engine for a round and waits until idle, then lists the server's folder tree and every message (1,000 per page, ID/folder/received/read/flag only) one folder at a time and compares with the store: folders missing, extra, renamed or re-parented; messages missing, extra, in the wrong folder, read state, flag, received time; bodies missing inside the window or kept outside it (unpinned, uncached, past a 1-day grace); and the store's derived counters (new read-only `Reader::derived_drift`). Anything that differed is re-checked after another round, one message at a time (up to 2,000), so changes still syncing aren't reported. Output: `clean`, counts by kind, the first 50 entries; exit 0 either way. Engine: `sync.now` now returns a ticket, served when the round goes idle or fails, so a failing account can't hang the check. Done criterion met against the mock: clean after a full sync of 500,000 messages (check took 21.7 s, release, 4-vCPU cloud VM) and again after 5,000 changes; clean with changes landing just before the check; injected store drift (read flip, flag flip, moved, deleted, ghost message, ghost folder, dropped window body, old unpinned body, counter edited by hand in SQLite) each reported once with the right kind, and nothing changed. CI runs this at 20,000 messages plus `tests/check.rs`. Real-account run: WP 1.13 |

### WP 0.2 step B results (2026-11-04)

Client ID `c6057b6a-a692-47dd-b34e-12629372e32a`, full scope set. "Graph calls" means `/me`, `/me/mailFolders`, `/me/messages`, `/me/calendars` and `/me/mailboxSettings`.

| Account | Authority | Flow | Sign-in | Graph calls | Refresh | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Hotmail | `consumers` | device code | ok | all 200 | ok | |
| Hotmail | `consumers` | PKCE | ok | all 200 | ok | Granted exactly the requested scopes |
| Parklands | `organizations` | device code | ok | all 200 | failed: HTTP 400 `invalid_grant` AADSTS65001 (not consented) | Refresh was sent to `organizations` |
| Parklands | `organizations` | PKCE | ok | all 200 | ok | Refresh sent to the tenant ID from the token's `tid` claim. Granted also included `Contacts.ReadWrite` and `email`, so an earlier grant exists in Parklands |

Findings:

- Both in-scope accounts work with this app and these scopes. No admin consent was needed for Parklands.
- `offline_access` never appears in the granted scope string; the refresh token is the proof it was granted.
- WP 1.4 should refresh work accounts against the tenant ID, not `organizations`. Open: a device code rerun on Parklands with the tenant refresh, to confirm that was the cause (the first rerun code expired unused).
