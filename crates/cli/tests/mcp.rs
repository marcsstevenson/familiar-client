// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 2.7: `fam mcp serve` against famd and the mock.
//!
//! The server offers only the tools within `--allow`, runs them as source
//! `mcp`, and famd still refuses writes `[agent]` doesn't grant. The same
//! server answers commands by dotted name (the local JSON-RPC API).
//!
//! It runs the real `fam mcp serve` over stdio, with famd started in this
//! process from the daemon crate's test harness.

#[path = "../../daemon/tests/common/mod.rs"]
mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use serde_json::{Value, json};

const LIMIT: Duration = Duration::from_secs(60);

/// Sends `requests` to `fam mcp serve`, one per line, and returns its answers.
async fn exchange(env: &Env, allow: &str, requests: Vec<Value>) -> Vec<Value> {
    let data = env.dir.path().to_path_buf();
    let allow = allow.to_string();
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fam"))
            .args(["mcp", "serve", "--allow", &allow])
            .env("FAMILIAR_DATA_DIR", &data)
            .env("FAMILIAR_CONFIG", data.join("config.toml"))
            .env_remove("CLAUDECODE")
            .env_remove("FAMILIAR_AGENT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for r in &requests {
            writeln!(stdin, "{r}").unwrap();
        }
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    })
    .await
    .unwrap()
}

fn req(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn call(id: i64, tool: &str, args: Value) -> Value {
    req(id, "tools/call", json!({ "name": tool, "arguments": args }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_tools_reach_famd_as_mcp() {
    let mut env = Env::new(40).await;
    let config = env.dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "version = 1\n[accounts.{KEY}]\nemail = \"pat@example.com\"\ntenant = \"consumers\"\n"
        ),
    )
    .unwrap();
    env.start(fast_policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let id = env
        .reader()
        .folders(env.account())
        .unwrap()
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("inbox"))
        .map(|f| env.reader().messages(f.id, None, 1).unwrap()[0].id.0)
        .unwrap();

    let answers = exchange(
        &env,
        "draft,write",
        vec![
            req(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } })),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            req(2, "tools/list", json!({})),
            call(3, "mail_list", json!({ "limit": 5 })),
            call(4, "draft_create", json!({ "to": ["marc@example.com"], "subject": "From MCP", "body": "hi" })),
            // Offered (write is in --allow), but famd's [agent] has only read and draft.
            call(5, "mail_read", json!({ "messages": [id] })),
            // Not offered: send isn't in --allow.
            call(6, "draft_send", json!({ "message": 1 })),
            req(7, "mail.list", json!({ "limit": 2 })),
            req(8, "version", Value::Null),
            req(9, "draft.send", json!({})),
            req(10, "no/such", json!({})),
            req(11, "ping", json!({})),
        ],
    )
    .await;
    // One answer per request; none for the notification.
    let ids: Vec<i64> = answers.iter().map(|a| a["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, (1..=11).collect::<Vec<_>>());
    let a = |id: usize| &answers[id - 1];

    assert_eq!(a(1)["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(a(1)["result"]["serverInfo"]["name"], "familiar");
    let tools: Vec<&str> = a(2)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in [
        "mail_list",
        "mail_thread",
        "draft_create",
        "mail_read",
        "cal_add",
        "contacts_search",
    ] {
        assert!(tools.contains(&want), "{want} missing from {tools:?}");
    }
    for not in [
        "draft_send",
        "mail_send",
        "mail_delete",
        "auth_login",
        "auth_logout",
        "mcp_serve",
    ] {
        assert!(!tools.contains(&not), "{not} offered");
    }

    let listed = &a(3)["result"];
    assert_eq!(listed["isError"], false);
    assert_eq!(
        listed["structuredContent"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let text: Value = serde_json::from_str(listed["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, listed["structuredContent"]);

    assert_eq!(a(4)["result"]["isError"], false, "{}", a(4));
    let refused = &a(5)["result"];
    assert_eq!(refused["isError"], true);
    let err: Value = serde_json::from_str(refused["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(err["error"]["kind"], "permission_denied");
    assert_eq!(err["error"]["exit_code"], 5);

    assert_eq!(a(6)["error"]["code"], -32602);
    assert_eq!(a(7)["result"]["messages"].as_array().unwrap().len(), 2);
    assert!(a(8)["result"]["version"].is_string(), "{}", a(8));
    assert_eq!(a(9)["error"]["data"]["kind"], "permission_denied");
    assert_eq!(a(10)["error"]["code"], -32601);
    assert_eq!(a(11)["result"], json!({}));

    // famd logged both as mcp.
    let audit: Vec<(String, String, String)> = env
        .reader()
        .audit_log(0, 100)
        .unwrap()
        .into_iter()
        .map(|e| (e.source, e.command, e.outcome))
        .collect();
    assert!(
        audit.contains(&("mcp".into(), "draft.create".into(), "queued".into())),
        "{audit:?}"
    );
    assert!(
        audit.contains(&("mcp".into(), "mail.read".into(), "denied".into())),
        "{audit:?}"
    );
    env.stop().await;
}
