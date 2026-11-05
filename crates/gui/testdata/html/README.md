# Reading pane HTML test set

Hostile and ordinary message bodies for the reading pane's sanitiser
(`crates/gui/src/reading/sanitize.rs`). Each file is sanitised by
`cargo test -p familiar-gui html_test_set`, which checks that nothing
active or remote survives, and `scripts/check-reading-html.mjs` renders the
results in Chromium inside the same sandboxed frame and CSP the app uses,
failing if a script runs or a request leaves the page.

Add a file here whenever a real message renders wrongly or unsafely. Files
ending in `.txt` are plain-text bodies; everything else is HTML.
