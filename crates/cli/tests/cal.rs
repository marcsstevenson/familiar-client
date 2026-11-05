// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam cal list` and `fam cal calendars` (WP 1.11), end to end: the mock's
//! calendars synced into a store, then read by the `fam` binary.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use familiar_mock_graph::MockGraph;
use familiar_provider_graph::{
    CalendarApi, CalendarSync, ClientOptions, GraphClient, Priority, StaticToken,
};
use familiar_store::{NewAccount, Writer};
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

const CONFIG: &str = r#"
version = 1

[accounts.hotmail]
email = "pat@example.com"
tenant = "consumers"

[calendar.purposes]
family = { account = "hotmail", calendar = "Family Room" }
"#;

/// Syncs the mock's small mailbox calendars into `<data>/store` for a
/// window from 1 November 2026 to 1 February 2027.
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
        let sync = CalendarSync::new(CalendarApi::new(client), acc);
        sync.sync_calendars(&mut w, Priority::User).await.unwrap();
        let window = (1_793_491_200, 1_801_440_000); // 2026-11-01 .. 2027-02-01 UTC
        for cal in ["cal-default", "cal-family"] {
            sync.sync_events(&mut w, cal, window, Priority::User)
                .await
                .unwrap();
        }
    });
}

#[test]
fn family_room_lists_in_auckland_time() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    std::fs::write(data.join("config.toml"), CONFIG).unwrap();

    // No store yet.
    let (code, _, err) = fam(data, &["cal", "list", "--json"]);
    assert_eq!(code, 6, "{err}");

    sync_store(data);

    // By purpose, a fortnight from 5 November, in New Zealand time given by
    // its Windows name.
    let (code, out, err) = fam(
        data,
        &[
            "cal",
            "list",
            "--calendar",
            "family",
            "--from",
            "2026-11-05",
            "--days",
            "14",
            "--tz",
            "New Zealand Standard Time",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["time_zone"], "Pacific/Auckland");
    assert_eq!(v["calendars"], serde_json::json!(["hotmail/Family Room"]));
    assert_eq!(v["from"], "2026-11-05T00:00:00+13:00");
    let evs = v["events"].as_array().unwrap();
    assert_eq!(evs.len(), 2, "{out}");
    // 04:00 UTC is 17:00 NZDT.
    assert_eq!(evs[0]["subject"], "Junior practice");
    assert_eq!(evs[0]["start"], "2026-11-05T17:00:00+13:00");
    assert_eq!(evs[0]["end"], "2026-11-05T18:00:00+13:00");
    assert_eq!(evs[0]["recurring"], true);
    assert_eq!(evs[1]["start"], "2026-11-12T17:00:00+13:00");
    assert!(v.get("warnings").is_none(), "{out}");

    // By name, case-insensitive; the all-day holiday keeps its date.
    let (code, out, err) = fam(
        data,
        &[
            "cal",
            "list",
            "--calendar",
            "family room",
            "--from",
            "2026-12-18",
            "--days",
            "1",
            "--tz",
            "Pacific/Auckland",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let evs = v["events"].as_array().unwrap();
    assert_eq!(evs.len(), 1, "{out}");
    assert_eq!(evs[0]["subject"], "School holidays");
    assert_eq!(evs[0]["all_day"], true);
    assert_eq!(evs[0]["start"], "2026-12-18");
    assert_eq!(evs[0]["end"], "2026-12-19");
    // ... and doesn't spill into the next day.
    let (_, out, _) = fam(
        data,
        &[
            "cal",
            "list",
            "--calendar",
            "family room",
            "--from",
            "2026-12-19",
            "--days",
            "1",
            "--tz",
            "Pacific/Auckland",
            "--json",
        ],
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["events"], serde_json::json!([]), "{out}");

    // Every calendar, as text, in UTC.
    let (code, out, err) = fam(
        data,
        &[
            "cal",
            "list",
            "--from",
            "2026-11-09",
            "--days",
            "2",
            "--tz",
            "UTC",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Tue 10 Nov 2026"), "{out}");
    assert!(
        out.contains("06:00-07:30  Committee meeting @ Clubrooms"),
        "{out}"
    );
    assert!(out.contains("Times in UTC."), "{out}");

    // Outside the synced window: a note.
    let (_, out, _) = fam(
        data,
        &[
            "cal",
            "list",
            "--calendar",
            "family",
            "--from",
            "2027-03-01",
            "--json",
        ],
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["warnings"][0].as_str().unwrap().contains("not synced"),
        "{out}"
    );

    // Unknown calendar and bad input.
    let (code, _, err) = fam(data, &["cal", "list", "--calendar", "Work"]);
    assert_eq!(code, 6, "{err}");
    assert!(err.contains("Family Room"), "{err}");
    let (code, _, _) = fam(data, &["cal", "list", "--tz", "Mars/Olympus"]);
    assert_eq!(code, 2);
    let (code, _, _) = fam(data, &["cal", "list", "--from", "someday"]);
    assert_eq!(code, 2);

    // The calendar list.
    let (code, out, err) = fam(data, &["cal", "calendars", "--json"]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let cals = v["calendars"].as_array().unwrap();
    assert_eq!(cals.len(), 2);
    assert_eq!(cals[0]["name"], "Calendar");
    assert_eq!(cals[0]["is_default"], true);
    assert_eq!(cals[1]["name"], "Family Room");
    assert_eq!(cals[1]["purposes"], serde_json::json!(["family"]));
    assert_eq!(cals[1]["events"], 3);
    assert_eq!(cals[1]["synced_from"], "2026-11-01");
}
