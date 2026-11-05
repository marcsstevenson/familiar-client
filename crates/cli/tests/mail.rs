// SPDX-License-Identifier: MIT OR Apache-2.0
//! The WP 1.10 read commands through the `fam` binary, against a store
//! synced from the mock's small mailbox: `fam folder list`, `fam mail list`,
//! `fam mail thread`, `fam mail fetch` and the parts of `fam api` that don't
//! need famd. Downloads and `fam api` calls through famd are tested in
//! `familiar-daemon` (`tests/read.rs`).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use familiar_mock_graph::MockGraph;
use familiar_provider_graph::{
    ClientOptions, GraphClient, MailApi, MailSync, Priority, StaticToken,
};
use familiar_store::{NewAccount, Reader, Writer};
use serde_json::Value;

fn fam(data: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_fam"))
        .args(args)
        .env("FAMILIAR_DATA_DIR", data)
        .env("FAMILIAR_CONFIG", data.join("config.toml"))
        .output()
        .expect("run fam");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json(data: &Path, args: &[&str]) -> Value {
    let mut a = args.to_vec();
    a.push("--json");
    let (code, out, err) = fam(data, &a);
    assert_eq!(code, 0, "fam {args:?}: {err}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("fam {args:?}: {e}: {out}"))
}

fn fails(data: &Path, args: &[&str]) -> (i32, Value) {
    let mut a = args.to_vec();
    a.push("--json");
    let (code, _, err) = fam(data, &a);
    let e: Value = serde_json::from_str(&err).unwrap_or_else(|e| panic!("{e}: {err}"));
    (code, e["error"].clone())
}

/// Syncs the small mailbox's folders and headers into `<data>/store`, with
/// bodies for two Inbox messages only.
fn sync_store(data: &Path) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mock = MockGraph::start().await.unwrap();
        let client = GraphClient::new(
            ClientOptions::with_base(mock.base_url()),
            Arc::new(StaticToken("test-token".into())),
        )
        .unwrap();
        let mut w = Writer::open(&data.join("store")).unwrap();
        let acc = w
            .write(|tx| {
                tx.upsert_account(&NewAccount {
                    key: "hotmail".into(),
                    email: "pat@example.com".into(),
                    ..Default::default()
                })
            })
            .unwrap();
        let mut sync = MailSync::new(MailApi::new(client), acc);
        sync.sync_folder_tree(&mut w, Priority::User).await.unwrap();
        let remotes: Vec<String> = Reader::open(&data.join("store"))
            .unwrap()
            .folders(acc)
            .unwrap()
            .into_iter()
            .map(|f| f.remote_id)
            .collect();
        let refs: Vec<&str> = remotes.iter().map(String::as_str).collect();
        sync.sync_folders(&mut w, &refs, Priority::User)
            .await
            .unwrap();
        for remote in ["imm-small-01", "imm-small-05"] {
            let id = w
                .write(|tx| Ok(tx.message_by_remote(acc, remote)?.unwrap().id))
                .unwrap();
            sync.fetch_body(&mut w, id, Priority::User).await.unwrap();
        }
    });
}

fn id_of(data: &Path, remote: &str) -> String {
    let r = Reader::open(&data.join("store")).unwrap();
    let acc = r.account_by_key("hotmail").unwrap().unwrap();
    r.message_by_remote(acc.id, remote)
        .unwrap()
        .unwrap()
        .id
        .0
        .to_string()
}

fn subjects(v: &Value) -> Vec<String> {
    v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["subject"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn read_commands_before_anything_synced() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    for args in [
        &["mail", "list"][..],
        &["folder", "list"],
        &["mail", "thread", "1"],
        &["mail", "fetch", "1"],
    ] {
        let (code, e) = fails(data, args);
        assert_eq!(code, 6, "{args:?}: {e}");
        assert_eq!(e["kind"], "not_found");
    }
}

#[test]
fn folders_lists_threads_and_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    sync_store(data);

    // fam folder list: a tree, depth first, with paths and counts.
    let v = json(data, &["folder", "list"]);
    let rows = v["folders"].as_array().unwrap();
    let paths: Vec<&str> = rows.iter().map(|f| f["path"].as_str().unwrap()).collect();
    for p in [
        "Inbox",
        "Inbox/Receipts",
        "Cricket Club",
        "Cricket Club/Juniors",
        "Sent Items",
    ] {
        assert!(paths.contains(&p), "{p} missing from {paths:?}");
    }
    let pos = |p: &str| paths.iter().position(|x| *x == p).unwrap();
    assert_eq!(pos("Cricket Club") + 1, pos("Cricket Club/Juniors"));
    let inbox = &rows[pos("Inbox")];
    assert_eq!(inbox["well_known"], "inbox");
    assert_eq!(inbox["total"], 2);
    assert_eq!(inbox["unread"], 2);
    assert_eq!(inbox["depth"], 0);
    assert_eq!(rows[pos("Inbox/Receipts")]["depth"], 1);
    assert_eq!(rows[pos("Inbox/Receipts")]["parent"], inbox["id"]);
    let (code, text, _) = fam(data, &["folder", "list"]);
    assert_eq!(code, 0);
    assert!(text.starts_with("hotmail\n"), "{text}");
    assert!(text.contains("    Receipts"), "{text}");
    let (code, e) = fails(data, &["folder", "list", "--account", "nobody"]);
    assert_eq!((code, e["kind"].as_str()), (6, Some("not_found")));

    // fam mail list: the Inbox by default, newest first.
    let v = json(data, &["mail", "list"]);
    assert_eq!(
        subjects(&v),
        ["Practice moved to Thursday", "Committee meeting agenda"]
    );
    assert_eq!(v["folders"][0]["folder"], "Inbox");
    assert!(v.get("next").is_none());
    let m = &v["messages"][1];
    assert_eq!(m["account"], "hotmail");
    assert_eq!(m["folder"], "Inbox");
    assert_eq!(m["from"]["addr"], "casey.chen@club.example");
    assert_eq!(m["to"][0]["addr"], "pat@example.com");
    assert_eq!(m["importance"], "high");
    assert_eq!(m["is_flagged"], true);
    assert_eq!(m["body_local"], true);
    assert_eq!(m["received_at"], "2026-11-02T08:30:00Z");

    assert_eq!(
        subjects(&json(data, &["mail", "list", "--flagged"])),
        ["Committee meeting agenda"]
    );
    assert_eq!(
        json(data, &["mail", "list", "--folder", "sentitems", "--unread"])["messages"],
        serde_json::json!([])
    );
    // A name, a path or a well-known name, any case.
    for f in ["Juniors", "cricket club/juniors"] {
        let v = json(data, &["mail", "list", "--folder", f]);
        assert_eq!(subjects(&v), ["RE: Practice moved to Thursday"], "{f}");
        assert_eq!(v["folders"][0]["folder"], "Cricket Club/Juniors");
    }
    assert_eq!(
        subjects(&json(
            data,
            &[
                "mail",
                "list",
                "--folder",
                "Sent Items",
                "--account",
                "pat@example.com"
            ]
        )),
        ["RE: Practice moved to Thursday"]
    );

    // Paging: one at a time, then the rest.
    let p1 = json(data, &["mail", "list", "--limit", "1"]);
    assert_eq!(subjects(&p1), ["Practice moved to Thursday"]);
    let next = p1["next"].as_str().unwrap().to_string();
    let p2 = json(data, &["mail", "list", "--limit", "1", "--after", &next]);
    assert_eq!(subjects(&p2), ["Committee meeting agenda"]);
    assert!(p2.get("next").is_none(), "{p2}");
    let (code, e) = fails(data, &["mail", "list", "--after", "nonsense"]);
    assert_eq!((code, e["kind"].as_str()), (2, Some("usage")));
    let (code, e) = fails(data, &["mail", "list", "--folder", "Nowhere"]);
    assert_eq!((code, e["kind"].as_str()), (6, Some("not_found")));

    let (code, text, _) = fam(data, &["mail", "list"]);
    assert_eq!(code, 0);
    assert!(text.contains("hotmail/Inbox (2 unread)"), "{text}");
    assert!(text.contains("Practice moved to Thursday"), "{text}");

    // fam mail thread: the conversation across Inbox, Sent Items and a subfolder.
    let first = id_of(data, "imm-small-01");
    let t = json(data, &["mail", "thread", &first]);
    assert_eq!(t["subject"], "Practice moved to Thursday");
    assert_eq!(t["account"], "hotmail");
    assert_eq!(t["message_count"], 3);
    let folders: Vec<&str> = t["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["folder"].as_str().unwrap())
        .collect();
    assert_eq!(folders, ["Inbox", "Sent Items", "Cricket Club/Juniors"]);
    assert_eq!(t["messages"][1]["from_me"], true);
    assert_eq!(t["messages"][0]["from_me"], false);
    assert_eq!(t["messages"][2]["cc"][0]["addr"], "pat@example.com");
    // Any message of the thread gives the same conversation.
    let reply = id_of(data, "imm-small-03");
    assert_eq!(
        json(data, &["mail", "thread", &reply])["thread"],
        t["thread"]
    );
    let (code, text, _) = fam(data, &["mail", "thread", &first]);
    assert_eq!(code, 0);
    assert!(text.contains("(3 messages, "), "{text}");
    let (code, e) = fails(data, &["mail", "thread", "999999"]);
    assert_eq!((code, e["kind"].as_str()), (6, Some("not_found")));

    // fam mail fetch: a local body as text (default) or as stored HTML.
    let f = json(data, &["mail", "fetch", &first]);
    assert_eq!(f["subject"], "Practice moved to Thursday");
    assert_eq!(f["body_type"], "text");
    assert_eq!(f["downloaded"], false);
    let body = f["body"].as_str().unwrap();
    assert!(!body.contains('<'), "{body}");
    assert!(body.to_lowercase().contains("thursday"), "{body}");
    let h = json(data, &["mail", "fetch", &first, "--format", "html"]);
    assert_eq!(h["body_type"], "html");
    assert!(h["body"].as_str().unwrap().contains('<'));
    let (code, text, _) = fam(data, &["mail", "fetch", &first]);
    assert_eq!(code, 0);
    assert!(
        text.contains("From:    Sam Walker <sam.walker@club.example>"),
        "{text}"
    );

    // A body that isn't local: --local shows the header only; otherwise it
    // needs famd to download it.
    let shipped = id_of(data, "imm-small-04");
    let l = json(data, &["mail", "fetch", &shipped, "--local"]);
    assert_eq!(l["subject"], "Your order has shipped");
    assert_eq!(l["folder"], "Inbox/Receipts");
    assert!(l.get("body").is_none(), "{l}");
    assert_eq!(l["body_local"], false);
    let (code, e) = fails(data, &["mail", "fetch", &shipped]);
    assert_eq!((code, e["kind"].as_str()), (11, Some("daemon_unavailable")));
    assert!(e["message"].as_str().unwrap().contains("--local"), "{e}");
    let (code, e) = fails(data, &["mail", "fetch", "999999"]);
    assert_eq!((code, e["kind"].as_str()), (6, Some("not_found")));
}

#[test]
fn api_is_get_only_and_graph_only() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();

    // Writes are refused before anything is sent.
    for m in ["POST", "patch", "DELETE"] {
        let (code, e) = fails(data, &["api", "/me/messages/x", "--method", m]);
        assert_eq!(
            (code, e["kind"].as_str()),
            (5, Some("permission_denied")),
            "{m}"
        );
    }
    // Only Graph paths and links.
    for p in [
        "me/messages",
        "//evil.example/x",
        "https://evil.example/v1.0/me",
    ] {
        let (code, e) = fails(data, &["api", p]);
        assert_eq!((code, e["kind"].as_str()), (2, Some("usage")), "{p}");
    }
    // A good GET needs famd.
    for args in [
        &["api", "/me"][..],
        &[
            "api",
            "https://graph.microsoft.com/v1.0/me",
            "--method",
            "GET",
        ],
    ] {
        let (code, e) = fails(data, args);
        assert_eq!(
            (code, e["kind"].as_str()),
            (11, Some("daemon_unavailable")),
            "{args:?}"
        );
    }
}
