// SPDX-License-Identifier: MIT OR Apache-2.0
//! The WP 1.3 done-when test: edits keep comments and ordering.

use familiar_config::{ConfigDocument, Severity, SignatureKind, check};

const EXAMPLE: &str = include_str!("../../../config.example.toml");

#[test]
fn example_config_is_valid_with_no_warnings() {
    let r = check(EXAMPLE);
    assert!(r.problems.is_empty(), "{:#?}", r.problems);
    let c = r.config.unwrap();
    assert_eq!(c.accounts.len(), 2);
    assert_eq!(c.accounts["hotmail"].tenant, "consumers");
    assert_eq!(
        c.app.client_id_or_default(),
        familiar_config::DEFAULT_CLIENT_ID
    );
    let sig = c.accounts["hotmail"].signature.as_ref().unwrap();
    assert_eq!(sig.reply.as_deref(), Some("<p>Your Name</p>\n"));
}

#[test]
fn unedited_document_round_trips_byte_for_byte() {
    let doc = ConfigDocument::parse(EXAMPLE).unwrap();
    assert_eq!(doc.to_string(), EXAMPLE);
}

#[test]
fn crlf_round_trips() {
    let crlf = EXAMPLE.replace('\n', "\r\n");
    let doc = ConfigDocument::parse(&crlf).unwrap();
    assert_eq!(doc.to_string(), crlf);
}

/// Changing one value changes exactly that line: every comment, blank line
/// and key stays where it was.
#[test]
fn editing_a_value_changes_only_that_line() {
    let mut doc = ConfigDocument::parse(EXAMPLE).unwrap();
    doc.set(&["app", "theme"], "dark").unwrap();
    doc.set(&["sync", "window"], "6m").unwrap();
    let out = doc.to_string();
    let before: Vec<&str> = EXAMPLE.lines().collect();
    let after: Vec<&str> = out.lines().collect();
    assert_eq!(before.len(), after.len());
    let changed: Vec<(&str, &str)> = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| a != b)
        .map(|(a, b)| (*a, *b))
        .collect();
    assert_eq!(
        changed,
        vec![
            (
                r#"theme = "system"           # system, light or dark"#,
                r#"theme = "dark"           # system, light or dark"#
            ),
            (
                r#"window = "1y"              # full bodies kept for this long: 90d, 6m, 1y ..."#,
                r#"window = "6m"              # full bodies kept for this long: 90d, 6m, 1y ..."#
            ),
        ]
    );
    assert!(check(&out).is_valid());
}

#[test]
fn signature_edit_keeps_everything_else() {
    let mut doc = ConfigDocument::parse(EXAMPLE).unwrap();
    let html = "<p>Marc's new sig</p>\n<p>Line two</p>";
    doc.set_signature("parklands", SignatureKind::Reply, html)
        .unwrap();
    let out = doc.to_string();
    // Readable in the file, not escaped.
    assert!(
        out.contains("reply = '''\n<p>Marc's new sig</p>\n<p>Line two</p>\n'''"),
        "{out}"
    );
    // Only the signature lines differ.
    let old_sig = "reply = '''\n<p>Your Name<br>Your club role</p>\n'''";
    let new_sig = "reply = '''\n<p>Marc's new sig</p>\n<p>Line two</p>\n'''";
    let idx = EXAMPLE.rfind(old_sig).unwrap();
    let expected = format!(
        "{}{}{}",
        &EXAMPLE[..idx],
        new_sig,
        &EXAMPLE[idx + old_sig.len()..]
    );
    assert_eq!(out, expected);
    let c = check(&out).config.unwrap();
    assert_eq!(
        c.accounts["parklands"]
            .signature
            .as_ref()
            .unwrap()
            .reply
            .as_deref(),
        Some("<p>Marc's new sig</p>\n<p>Line two</p>\n")
    );
}

#[test]
fn signature_with_triple_quotes_falls_back_to_escaped() {
    let mut doc = ConfigDocument::parse(EXAMPLE).unwrap();
    let html = "<p>'''odd'''</p>";
    doc.set_signature("hotmail", SignatureKind::New, html)
        .unwrap();
    let c = doc.check().config.unwrap();
    assert_eq!(
        c.accounts["hotmail"]
            .signature
            .as_ref()
            .unwrap()
            .new
            .as_deref(),
        Some(html)
    );
}

#[test]
fn new_keys_and_tables_go_at_the_end_and_keep_order() {
    let mut doc = ConfigDocument::parse(EXAMPLE).unwrap();
    doc.set(&["accounts", "spare", "email"], "spare@example.com")
        .unwrap();
    doc.set(&["accounts", "spare", "tenant"], "consumers")
        .unwrap();
    doc.set(&["keys", "delete"], "#").unwrap();
    let out = doc.to_string();
    // Everything that was there is still there, in order, as a prefix of each section.
    let mut pos = 0;
    for line in EXAMPLE.lines() {
        let found = out[pos..]
            .find(line)
            .unwrap_or_else(|| panic!("lost or reordered: {line:?}\n{out}"));
        pos += found + line.len();
    }
    assert!(out.contains("reply = \"r\"\ndelete = \"#\"\n"), "{out}");
    assert!(
        out.contains("[accounts.spare]\nemail = \"spare@example.com\"\ntenant = \"consumers\"\n"),
        "{out}"
    );
    assert!(
        !out.contains("[accounts]\n"),
        "no empty parent header: {out}"
    );
    let r = check(&out);
    assert!(r.is_valid(), "{:#?}", r.problems);
    assert_eq!(r.config.unwrap().accounts.len(), 3);
}

#[test]
fn remove_and_reload() {
    let mut doc = ConfigDocument::parse(EXAMPLE).unwrap();
    assert!(doc.remove(&["accounts", "parklands", "display_name"]));
    assert!(!doc.remove(&["accounts", "parklands", "display_name"]));
    let c = doc.check().config.unwrap();
    assert_eq!(c.accounts["parklands"].display_name, None);
}

#[test]
fn signature_for_unknown_account_is_refused() {
    let mut doc = ConfigDocument::parse(EXAMPLE).unwrap();
    assert!(doc.set_signature("nope", SignatureKind::New, "x").is_err());
}

#[test]
fn save_is_atomic_and_refuses_invalid_results() {
    let dir = std::env::temp_dir().join(format!("familiar-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, EXAMPLE).unwrap();

    let mut doc = ConfigDocument::open(&path).unwrap();
    doc.set(&["app", "send_delay_seconds"], 30).unwrap();
    doc.save(&path).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("send_delay_seconds = 30    # undo-send window"));
    assert!(!dir.join("config.toml.tmp").exists());

    doc.set(&["sync", "window"], "forever").unwrap();
    let err = doc.save(&path).unwrap_err();
    assert!(err.0.contains("sync.window"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);

    let loaded = familiar_config::load(&path).unwrap();
    assert_eq!(loaded.app.send_delay_seconds, 30);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn problems_have_locations() {
    let text = "version = 1\n\n[accounts.work]\nemail = \"nope\"\ntenant = \"organizations\"\n\n[[rules]]\nname = \"x\"\naccount = \"home\"\nwhen = { a = 1 }\nthen = { b = 1 }\n";
    let r = check(text);
    let got: Vec<String> = r.problems.iter().map(|p| p.to_string()).collect();
    assert_eq!(
        got,
        vec![
            "4:9: accounts.work.email: `nope` is not an email address",
            "5:10: warning: accounts.work.tenant: `organizations` works for sign-in, but token refresh must use the account's own tenant; set the tenant ID or domain",
            "9:11: rules[0].account: no account called `home` in [accounts]",
        ]
    );
    assert!(!r.is_valid());
    assert_eq!(r.warnings().count(), 1);
}

#[test]
fn syntax_and_shape_errors_have_locations() {
    let r = check("version = 1\n[app\n");
    assert_eq!(r.problems.len(), 1);
    assert_eq!(r.problems[0].line, Some(2));

    let r = check("version = 1\n[app]\nthem = \"dark\"\n");
    let p = &r.problems[0];
    assert_eq!(p.severity, Severity::Error);
    assert_eq!(p.line, Some(3));
    assert!(p.message.contains("them"), "{}", p.message);

    let r = check("version = 1\n[sync]\nwindow = 5\n");
    assert_eq!(r.problems[0].line, Some(3));

    let r = check("[app]\n");
    assert!(
        r.problems[0].message.contains("version"),
        "{:?}",
        r.problems
    );
}

#[test]
fn semantic_checks() {
    let text = r#"
version = 2
[app]
client_id = "not-a-guid"
send_delay_seconds = 500
[accounts.Bad]
email = "a@example.com"
tenant = "consumers"
[accounts.dup]
email = "A@example.com"
tenant = "bad tenant"
[calendar.purposes]
family = { account = "ghost", calendar = "" }
[sync]
window = "1y"
headers = "6m"
cache_limit = "lots"
[[views]]
name = "v"
query = ""
[[views]]
name = "v"
query = "x"
[keys]
archive = "e"
delete = "e"
[agent]
allow = ["draft", "calendar-write"]
"#;
    let r = check(text);
    let paths: Vec<(&str, Severity)> = r
        .problems
        .iter()
        .map(|p| (p.path.as_str(), p.severity))
        .collect();
    use Severity::*;
    for want in [
        ("version", Error),
        ("app.client_id", Error),
        ("app.send_delay_seconds", Error),
        ("accounts.Bad", Error),
        ("accounts.dup.email", Error),
        ("accounts.dup.tenant", Error),
        ("calendar.purposes.family.account", Error),
        ("calendar.purposes.family.calendar", Error),
        ("sync.headers", Error),
        ("sync.cache_limit", Error),
        ("views[0].query", Error),
        ("views[1].name", Error),
        ("keys.delete", Error),
        ("agent.allow[1]", Error),
        ("agent.allow", Warning),
    ] {
        assert!(paths.contains(&want), "missing {want:?} in {paths:#?}");
    }
    assert_eq!(paths.len(), 15, "{:#?}", r.problems);
    // Every problem here can be located.
    for p in &r.problems {
        assert!(p.line.is_some(), "no line for {p}");
    }
    // The second view's name, not the first.
    let v = r
        .problems
        .iter()
        .find(|p| p.path == "views[1].name")
        .unwrap();
    assert_eq!(v.line, Some(22));
}

#[test]
fn minimal_config_gets_defaults() {
    let r =
        check("version = 1\n[accounts.home]\nemail = \"me@example.com\"\ntenant = \"consumers\"\n");
    assert!(r.problems.is_empty(), "{:#?}", r.problems);
    let c = r.config.unwrap();
    assert_eq!(c.sync.window, "1y");
    assert_eq!(c.app.send_delay_seconds, 10);
    assert_eq!(c.agent.allow, vec!["read", "draft"]);
}

#[test]
fn crlf_edit_keeps_crlf() {
    let crlf = EXAMPLE.replace('\n', "\r\n");
    let mut doc = ConfigDocument::parse(&crlf).unwrap();
    doc.set(&["keys", "delete"], "#").unwrap();
    doc.set_signature("hotmail", SignatureKind::New, "<p>a</p>\n<p>b</p>")
        .unwrap();
    let out = doc.to_string();
    assert!(
        !out.replace("\r\n", "").contains('\n'),
        "bare LF in {out:?}"
    );
    assert!(out.contains("reply = \"r\"\r\ndelete = \"#\"\r\n"));
    assert!(check(&out).is_valid());
}
