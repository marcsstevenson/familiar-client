# CLAUDE.md

## Change workflow: judge the risk, then pick the route

Each thread judges how risky its change is and picks one of two routes. When unsure, use a pull request.

Every change, whichever route, first passes the local checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` and `bash scripts/check-licence-headers.sh`. CI does not run on pull requests, because it is too slow.

**Low risk: push straight to `main`.** Small UI tweaks, docs, icons, config examples, and contained bug fixes that touch one area and cannot damage user data. Run the checks, commit and push to `main`. No PR.

**Higher risk: pull request with Codex review.** The sync engine, the store or its schema, the op queue or anything that writes to live mail or calendars, auth and tokens, refactors across several crates, and anything else that touches user data.

- Include `@codex review` in the PR body when you open it, so Codex reviews it on first open.
- Watch every PR you open. Answer each Codex finding by the triage rules below, then merge the PR yourself once your local checks pass on the PR branch. Marc does not need to review.

Either way, CI runs on `main` after the push or merge. Check that run, and if it goes red, fix it straight away (by the same risk rule).

### Codex review triage: fix what is real, decline what is imagined

Codex reviews every PR and will keep finding things for as long as you keep asking. That is its job. Yours is to decide which findings describe a failure a real user of Familiar can hit, and to say no to the rest. We build a personal mail and calendar client, not missile-launch software: simple, straightforward code that handles the cases we have actually observed beats layered defences against cases nobody has seen.

The pattern to avoid: a finding about an unobserved edge case got a mechanism, the mechanism got its own finding, that got another mechanism, and the code ended up more fragile than the bug it started from. Half of the review rounds were Codex reviewing our own over-engineering.

Rules:

- **Before fixing a finding, ask: has this failure been observed, or can it be provoked?** If the answer is a scenario with no known path to occur (an API returning data it has never returned, two events racing inside a millisecond window, a server lying about itself), reply on the thread with the evidence and decline. A declined finding with a one-line reason on the PR is a good outcome, not a failure.
- **A fix must leave the code simpler or the same.** If addressing a finding means adding a new abstraction, a new state type, a fallback for the fallback, or a branch that only an imagined input can reach, that is the signal to decline, not to build.
- **Fail-open plus a log line beats new machinery.** For a genuinely unobserved edge, log a warning and keep the existing behaviour. If the hypothetical ever becomes real, the log will say so and we fix it then, with evidence.
- **Two rounds, then stop.** Address the first review, address the second if it found something real, then stop requesting reviews. A third round almost always reviews the fixes, not the feature.
- **Weigh the fix's own failure mode.** Some "fixes" trade an imagined problem for a real one. Say so on the thread.
- **Severity badges are Codex's opinion, not a priority.** A P1 on a hypothetical is still a hypothetical.
