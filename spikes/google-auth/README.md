# Google auth spike (Gmail plan WP G.0)

Throwaway. Signs in to one Google account with authorisation code + PKCE through the system browser (loopback redirect), then calls Gmail `profile`, `labels`, `history`, `messages.list` and `settings/sendAs`, Calendar `calendarList`, and People `connections` and `otherContacts`. It refreshes the token once and calls `profile` again. Each line prints `ok` or `failed` with the HTTP status and Google's error reason. Only counts are printed (messages, threads, labels, send-as addresses, calendars, contacts), never names, addresses or subjects. Tokens stay in memory: they are never printed or written to disk.

It reads the OAuth client from the JSON file Google Cloud gives you for a **Desktop app** client, by default `%USERPROFILE%\.familiar\google-client.json`:

```powershell
cargo run
cargo run -- --client-file D:\path\to\client.json
```

The step-by-step guide, including creating the Google project, is `docs/google-sign-in-test-guide.md`.
