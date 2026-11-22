# Releasing

`.github/workflows/release.yml` builds the desktop app (`familiar`), famd and fam for Windows (x64), macOS (Apple silicon) and Linux (x64), and publishes them as a GitHub release with a `SHA256SUMS.txt`. Each archive holds the three programs side by side, so the app finds famd next to itself, plus the README, licences and `config.example.toml`.

## One-time setup: the Google client

Release builds compile in Familiar's own Google client so Gmail needs no setup (see [gmail-setup.md](gmail-setup.md)). The values never go in the repo. Store them in the repository's Actions settings (Settings, Secrets and variables, Actions):

| Name | Kind | Value |
| --- | --- | --- |
| `FAMILIAR_GOOGLE_CLIENT_ID` | Variable (or secret) | `installed.client_id` from `google-client.json` |
| `FAMILIAR_GOOGLE_CLIENT_SECRET` | Secret | `installed.client_secret` from `google-client.json` |

From PowerShell with the GitHub CLI signed in, this sets both from the file without showing the secret:

```powershell
$c = (Get-Content "$env:USERPROFILE\.familiar\google-client.json" | ConvertFrom-Json).installed
gh variable set FAMILIAR_GOOGLE_CLIENT_ID --repo marcsstevenson/familiar-client --body $c.client_id
$c.client_secret | gh secret set FAMILIAR_GOOGLE_CLIENT_SECRET --repo marcsstevenson/familiar-client
```

The workflow stops before building if either is missing, so a release never ships without the built-in client.

## Making a release

Either push a tag:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

or open Actions, Release, Run workflow, and enter the tag (for example `v0.2.0`); the tag is created on the commit you run it from. The release notes are generated from the pull requests and commits since the previous release; edit them on the release page afterwards if you like.

The builds are not code-signed, so Windows SmartScreen and macOS Gatekeeper warn on first open (on macOS, run `xattr -dr com.apple.quarantine` on the unpacked folder once).
