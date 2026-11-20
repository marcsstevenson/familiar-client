# familiar-mock-gmail

A local mock of the Gmail API and Google sign-in, so the Gmail provider (WP G.5 onwards),
sync engine and CLI tests run offline, including in cloud threads that can't reach Google
(WP G.4). It works the way `familiar-mock-graph` does for Microsoft Graph.

## Using it in a test

```rust
use familiar_mock_gmail::{Fault, MockGmail};

#[tokio::test]
async fn syncs_labels() {
    let mock = MockGmail::start().await.unwrap();       // mailbox "default", token "test-token"
    let api = mock.api_url();                           // http://127.0.0.1:<port>/gmail/v1
    mock.push_fault(Fault::rate_limited().on_path("/history").times(1));
    // ... point the provider at `api`, sync, then change the mailbox:
    mock.mailbox("default").modify("msg-dinner", &["Label_1"], &["INBOX"]).unwrap();
    // ... sync again: history.list reports labelAdded and labelRemoved.
}
```

Add it as a dev-dependency with `familiar-mock-gmail.workspace = true`.

Several mailboxes can run at once (`.mailbox("gmail", ...)`); each gets the bearer token
`<key>-token`. One port stands in for `gmail.googleapis.com`, `oauth2.googleapis.com` and
`accounts.google.com`: use `api_url()`, `upload_url()`, `batch_url()`, `auth_url()`,
`token_url()` and `revoke_url()`.

## What it serves

Paths are under `/gmail/v1/users/{userId}`, where `userId` is `me` or the account's address
(any other address gets 403 "Delegation denied").

| Area | Routes |
| --- | --- |
| Profile | `GET /profile` (`emailAddress`, `messagesTotal`, `threadsTotal`, `historyId`) |
| Labels | `GET /labels`, `GET /labels/{id}` (with message and thread counts), `POST /labels` (409 on a name clash, case-insensitive), `PATCH`/`PUT /labels/{id}` (system labels refused; children keep their names), `DELETE /labels/{id}` (taken off every message) |
| Messages | `GET /messages` with `q`, `labelIds` (all must match), `includeSpamTrash`, `maxResults` (default 100, at most 500), `pageToken`; newest first; Spam and Trash left out unless asked for |
| Message | `GET /messages/{id}` with `format=minimal`, `metadata` (+ `metadataHeaders`), `raw` or `full` (the default; MIME tree with `attachmentId`s); `GET /messages/{id}/attachments/{attachmentId}` |
| Changes | `POST /messages/{id}/modify`, `/trash`, `/untrash`, `POST /messages/batchModify` and `batchDelete` (up to 1,000 IDs; unknown IDs skipped), `DELETE /messages/{id}` |
| Threads | `GET /threads/{id}` (messages oldest first, any format) |
| Send | `POST /messages/send` with JSON `raw` (base64url) and optional `threadId`, or `POST /upload/gmail/v1/users/me/messages/send?uploadType=media` (body is the RFC 5322 message) or `uploadType=multipart` (JSON metadata part + `message/rfc822` part); 35 MB limit |
| Drafts | `GET /drafts`, `GET /drafts/{id}`, `POST /drafts`, `PUT /drafts/{id}`, `POST /drafts/send`, `DELETE /drafts/{id}`, also through `/upload` |
| History | `GET /history` with `startHistoryId`, `labelId`, `historyTypes`, `maxResults`, `pageToken` |
| Send-as | `GET /settings/sendAs`, `GET /settings/sendAs/{email}` |
| Batch | `POST /batch/gmail/v1`: `multipart/mixed` of `application/http` parts (relative paths or full URLs), at most 100; answers `multipart/mixed` with `Content-ID: <response-…>` in the same order |

`q` understands `from:`, `to:`, `cc:`, `bcc:`, `subject:`, `label:` (ID, or name with `-` for
spaces and `/`), `in:` (`inbox`, `sent`, `drafts`, `trash`, `spam`, `starred`, `anywhere`),
`is:` (`unread`, `read`, `starred`, `important`), `has:attachment`, `after:`/`before:`
(`2026/11/01` or epoch seconds), `rfc822msgid:`, quoted phrases, bare words (subject, body,
from, to) and `-` to negate.

Gmail behaviour it copies, because the provider has to handle it:

- **Labels, not folders.** A message has any number of labels; `UNREAD`, `STARRED`,
  `IMPORTANT` and `CATEGORY_*` are labels too. The 14 system labels are always there.
- **History.** Each change writes a record: `messagesAdded`, `messagesDeleted`,
  `labelsAdded` or `labelsRemoved` (one record per kind, so a modify that adds and
  removes writes two). `historyId`, `internalDate` and record IDs are strings, as in Gmail.
  Filtering by `labelId` still shows the record that removed that label. Loading the
  fixture is not history: `history.list` from the starting profile `historyId` is empty.
- **Expired history.** `MailboxHandle::expire_history()` makes `history.list` from any
  older `startHistoryId` answer 404 `notFound`, as after a week offline. The client then
  resyncs and carries on from the profile's new `historyId`.
- **Threading** follows `In-Reply-To`/`References` (or the `threadId` given on send or
  draft), so a reply joins its parent's thread.
- **Drafts** keep their draft ID but get a new message ID on every update (history shows a
  delete and an add). `drafts.send` deletes the draft message and adds the sent one.
- **Sending.** A `From` that is neither the account nor a verified send-as alias is
  rewritten to the account's address. `Message-ID` and `Date` are added if missing. No
  recipients is 400 "Recipient address required". Mail to one of the account's own
  addresses is labelled `SENT`, `INBOX` and `UNREAD`.
- **Errors** use Google's body: `{"error": {"code", "message", "errors": [{"domain",
  "reason", "message"}], "status"}}`.

## Sign-in (Google OAuth)

- `GET /o/oauth2/v2/auth` redirects to `redirect_uri` with `code` and `state` for the user
  named by `login_hint`, else the one set with `mock.sign_in_as(Some("gmail"))`; with
  nobody signed in it redirects with `error=access_denied`, as when the user cancels.
- `POST /token` takes `authorization_code` (PKCE `S256` or `plain` checked, `redirect_uri`
  must match, codes are single-use) and `refresh_token`. A refresh token comes back only
  with `access_type=offline`, and refreshing does not rotate it, as with Google. ID tokens
  are unsigned JWTs with `email`, `name` and `sub`.
- `POST /revoke?token=…` revokes a refresh token (and that account's access tokens) or one
  access token.
- `Builder::client(id, secret)` checks the client; by default any client ID is accepted.
- `Builder::token_lifetime(d)` sets `expires_in`; tokens lapse after it.
  `revoke_refresh_tokens(key)`, `expire_access_tokens(key)` and `expire_token(t)` do the rest.

## Failures

| Fault | Response |
| --- | --- |
| `Fault::rate_limited()` | 429 `rateLimitExceeded` (`RESOURCE_EXHAUSTED`), no `Retry-After` |
| `Fault::user_rate_limited()` | 403 `userRateLimitExceeded` |
| `Fault::too_many_concurrent()` | 429 "Too many concurrent requests for user" |
| `Builder::quota(units, window)` | 429 `rateLimitExceeded` once a mailbox would go over its units in the window. Units follow Google's table (`messages.get` 5, `history.list` 2, `messages.send` 100, `batchModify` 50, ...); `MailboxHandle::units_used()` shows the total |
| `Builder::max_concurrent(Some(n))` | 429 "Too many concurrent requests for user" past `n` in flight (off by default; batch parts don't count) |
| `Fault::unavailable()` | 503 `backendError` |
| missing, unknown, lapsed or expired bearer, or `Fault::auth_expired()` | 401 `authError` with `WWW-Authenticate` |
| `Fault::insufficient_scope()` | 403 `insufficientPermissions` |
| `Fault::status(code, reason, message)` | anything else |
| `Fault::delay(d)`, `Builder::latency(d)` | slow responses |

Faults can be narrowed with `.on_path("/history")`, `.on_method("POST")` and `.times(n)`,
and given `.retry_after(secs)`. They apply to each part of a batch on its own, so one part
can get 429 while the rest succeed. `MockGmail::requests()` lists every request with its
status (batch parts have `in_batch: true`).

## Data

- `Mailbox::small()` (`kea.tester@gmail.com`): a three-message thread across the Inbox,
  `Clients/Acme` and Sent; unread, starred, promotions and attachment mail in the Inbox; an
  archived receipt under `Receipts`; an archived message with no label (All Mail only);
  2015 mail under a hidden label; one each in Spam and Trash; a draft; a verified send-as
  alias with a signature and a pending one. Message IDs are readable (`msg-dinner`,
  `msg-acme-1`, ...).
- `Mailbox::synthetic(n, seed)`: `n` deterministic messages across the Inbox and three
  labels, in threads of up to three.
- `Mailbox::new(email, name)` with `.label(...)`, `.message(...)` and `.send_as(...)` builds
  anything else. `Message::new(from, to, subject, body)` is labelled `INBOX` and `UNREAD`;
  `.labels`, `.at`, `.id`, `.thread`, `.message_id`, `.in_reply_to` and `.attach` adjust it,
  and `Message::from_raw` takes a whole RFC 5322 message.

## Running it by hand

```sh
cargo run -p familiar-mock-gmail -- --port 8788 --synthetic 10000
curl --oauth2-bearer dev-token 'http://127.0.0.1:8788/gmail/v1/users/me/labels'
```

Options: `--synthetic N --seed N`, `--token TOKEN`, `--latency-ms N`.

## Not yet

- Calendar API and People API (for G.8 and G.9, after the Phase G gate).
- `threads.list`, `messages.insert`/`import`, resumable uploads, filters and other settings.
- Gmail's own subject matching for threads; the mock threads by `References` or `threadId`
  only. Label colours are not checked against Gmail's palette.
