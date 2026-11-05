# familiar-mock-graph

A local mock of Microsoft Graph so the provider, sync engine and CLI tests run offline,
including in cloud threads that can't reach Microsoft (WP 1.1).

## Using it in a test

```rust
use familiar_mock_graph::{Fault, MockGraph};

#[tokio::test]
async fn syncs_inbox() {
    let mock = MockGraph::start().await.unwrap();          // mailbox "default", token "test-token"
    let base = mock.base_url();                            // http://127.0.0.1:<port>/v1.0
    mock.push_fault(Fault::throttle(2).on_path("/delta").times(1));
    // ... point the provider at `base`, sync, then change the mailbox:
    mock.mailbox("default").move_message("imm-small-01", "archive").unwrap();
    // ... sync again and check the move was matched by immutable ID.
}
```

Add it as a dev-dependency with `familiar-mock-graph.workspace = true`.

Several mailboxes can run at once (`.mailbox("hotmail", ...)`, `.mailbox("parklands", ...)`);
each gets the token `<key>-token`.

## What it serves (GET only)

| Area | Paths under `/v1.0/me` (or `/v1.0/users/<id or upn>`) |
| --- | --- |
| Profile | `/me`, `/mailboxSettings` (no signature field, as in WP 0.3) |
| Folders | `/mailFolders`, `/mailFolders/{id or well-known name}`, `/childFolders`, `/mailFolders/delta` |
| Messages | `/messages`, `/mailFolders/{id}/messages`, `/messages/{id}` with `$top`, `$skip`, `$select`, `$filter` (`and` of `eq/ne/gt/ge/lt/le`), `$orderby=receivedDateTime`, `$count` |
| Delta | `/mailFolders/{id}/messages/delta` with `nextLink`/`deltaLink`, `Prefer: odata.maxpagesize`, `$select` kept across pages, `@removed` for deletes and moves out |
| Search | `$search` on `/messages` and folder messages: words, phrases, `from:`, `to:`, `cc:`, `subject:`, `body:`, `participants:`; at most 1,000 results; 400 with `$filter`, `$orderby` or `$skip` |
| Calendars | `/calendars`, `/calendar`, `/events`, `/calendars/{id}/events`, `/calendarView`, `/calendars/{id}/calendarView` (window required; series masters left out), and both `.../calendarView/delta` (occurrences carry only `id`, `type`, `start`, `end` and `seriesMasterId`, as on Graph) |

Graph behaviour it copies, because the provider has to handle it:

- The default message ID changes on a move. `Prefer: IdType="ImmutableId"` returns the
  immutable ID. The old default ID answers 404 after a move.
- `Prefer: outlook.body-content-type="text"` returns text bodies.
- Event times come back in UTC as `dateTimeTimeZone` (`2026-11-05T04:00:00.0000000`).
  `Prefer: outlook.timezone` is not applied.
- Errors use Graph's `{"error": {"code", "message", "innerError"}}` body with a `request-id`.

## Sign-in (identity platform)

The same port serves `/{tenant}/oauth2/v2.0/authorize`, `/devicecode` and `/token` like
`login.microsoftonline.com`, for the WP 1.4 sign-in tests. Point the client at
`mock.login_url()`; access tokens it issues work on the Graph routes.

- `mock.sign_in_as(Some("hotmail"))` is the person signed in to the browser: `/authorize`
  redirects with a code for them (or for the `login_hint` user), and device code polling
  succeeds on the second poll. `None` leaves device code pending.
- `Builder::tenant(key, CONSUMER_TENANT)` makes an account personal; others are in
  `DEFAULT_TENANT`. `consumers` takes only personal accounts, `organizations` only work
  ones, a tenant ID only its own; `common` takes both. Refreshing a work account against
  `organizations` or `common` fails with `invalid_grant` AADSTS65001, as Parklands did.
- PKCE (S256) is checked; refresh tokens rotate (old ones stay valid until revoked);
  `offline_access` is not echoed in `scope`. ID tokens are unsigned JWTs with `tid`,
  `oid`, `preferred_username` and `name`.
- `revoke_refresh_tokens(key)`, `expire_access_tokens(key)`, `Builder::token_lifetime` and
  `Builder::device_interval` set up expiry and revocation.

## Failures

| Fault | Response |
| --- | --- |
| `Fault::throttle(n)` | 429 `TooManyRequests`, `Retry-After: n` |
| more than 4 requests in flight per mailbox (`Builder::max_concurrent`) | 429 `ApplicationThrottled` (MailboxConcurrency), `Retry-After: 1` |
| `Builder::request_budget(limit, window)` | 429 `ApplicationThrottled` with the rest of the window as `Retry-After` |
| `Fault::service_unavailable(n)` | 503, `Retry-After: n` |
| missing, unknown or `expire_token`'d bearer, or `Fault::auth_expired()` | 401 `InvalidAuthenticationToken` with `WWW-Authenticate` |
| `Fault::access_denied()` | 403 `ErrorAccessDenied` |
| `MailboxHandle::expire_delta_tokens()` | 410 `SyncStateNotFound` for old delta and skip links |
| `Fault::delay(d)`, `Builder::latency(d)` | slow responses |

Faults can be narrowed with `.on_path("/delta")`, `.on_method("GET")` and `.times(n)`.
`MockGraph::requests()` returns every request with its status for assertions, and
`MailboxHandle::peak_in_flight()` shows the most concurrent requests seen.

## Data

- `Mailbox::small()`: the hand-written fixture in `fixtures/small-mailbox.json` (threads across
  folders, nested folders, a hidden folder, 2015 archive mail, a Family Room calendar).
- `Mailbox::synthetic(&SyntheticSpec { messages: 500_000, .. })`: deterministic mailboxes of
  any size. Bodies are generated from the ID when served, so large mailboxes stay small in memory.
- `Recording`: exchanges replayed verbatim before the model, with `{{baseUrl}}` in bodies
  rewritten to the mock. `fixtures/recorded-sample.json` shows the format. Recordings must be
  scrubbed of real names, addresses, subjects, bodies and tokens before they are committed.

## Running it by hand

```sh
cargo run -p familiar-mock-graph -- --port 8787 --synthetic 10000
curl --oauth2-bearer dev-token 'http://127.0.0.1:8787/v1.0/me/mailFolders'
```

Options: `--fixture FILE`, `--synthetic N --seed N`, `--recording FILE`, `--token TOKEN`,
`--latency-ms N`.

## Not yet

Writes over HTTP (POST/PATCH/DELETE, send, move via API) come with Phase 2 (WP 2.1–2.4).
Tests change mailboxes through `MockGraph::mailbox(...)` until then. A capture tool for new
recordings would run on Marc's machine and is left for when the provider needs one.
