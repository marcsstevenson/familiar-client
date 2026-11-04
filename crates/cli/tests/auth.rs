// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam auth login/status/logout` end to end: the real `fam` binary against the mock
//! identity platform and Graph, with the encrypted-file token store.

use familiar_mock_graph::{CONSUMER_TENANT, DEFAULT_TENANT, Mailbox, MockGraph};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fam(mock: &MockGraph, data: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_fam"))
        .args(args)
        .env("FAMILIAR_LOGIN_URL", mock.login_url())
        .env("FAMILIAR_GRAPH_URL", mock.base_url())
        .env("FAMILIAR_TOKEN_STORE", "file")
        .env("FAMILIAR_TOKEN_PASSPHRASE", "test passphrase")
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

fn json(out: &(i32, String, String)) -> Value {
    assert_eq!(out.0, 0, "fam failed: {}", out.2);
    serde_json::from_str(&out.1).expect("JSON on stdout")
}

fn data_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fam-auth-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn login_status_logout() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut hotmail = Mailbox::small();
    hotmail.user.user_principal_name = "marc@hotmail.example".into();
    let mock = rt
        .block_on(
            MockGraph::builder()
                .mailbox("hotmail", hotmail)
                .tenant("hotmail", CONSUMER_TENANT)
                .mailbox("parklands", Mailbox::small())
                .device_interval(0)
                .start(),
        )
        .unwrap();
    let data = data_dir("e2e");

    // Without --authority on a first sign-in: a usage error.
    let (code, _, err) = fam(&mock, &data, &["auth", "login", "hotmail", "--device-code"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("--authority") && err.contains("config.toml"),
        "{err}"
    );

    mock.sign_in_as(Some("hotmail"));
    let out = fam(
        &mock,
        &data,
        &[
            "auth",
            "login",
            "hotmail",
            "--authority",
            "consumers",
            "--device-code",
            "--json",
        ],
    );
    let v = json(&out);
    assert!(
        out.2.contains("microsoft.com/devicelogin"),
        "code shown on stderr: {}",
        out.2
    );
    assert_eq!(v["username"], "marc@hotmail.example");
    assert_eq!(v["refresh_authority"], "consumers");
    assert!(
        !out.1.contains("rt-") && !out.1.contains("at-"),
        "no tokens printed"
    );

    mock.sign_in_as(Some("parklands"));
    let v = json(&fam(
        &mock,
        &data,
        &[
            "auth",
            "login",
            "parklands",
            "--authority",
            "organizations",
            "--device-code",
            "--json",
        ],
    ));
    assert_eq!(v["refresh_authority"], DEFAULT_TENANT);

    // A new process, dead access tokens, nobody in the browser: --check must refresh.
    mock.sign_in_as(None);
    mock.expire_access_tokens("hotmail");
    mock.expire_access_tokens("parklands");
    let v = json(&fam(&mock, &data, &["auth", "status", "--check", "--json"]));
    assert_eq!(v["store"], "file");
    let states: Vec<(&str, &str)> = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["account"].as_str().unwrap(), a["state"].as_str().unwrap()))
        .collect();
    assert_eq!(states, [("hotmail", "ok"), ("parklands", "ok")]);

    // Re-login reuses the stored authority.
    mock.sign_in_as(Some("hotmail"));
    json(&fam(
        &mock,
        &data,
        &["auth", "login", "hotmail", "--device-code", "--json"],
    ));

    // Revoked: a check of one account fails with exit code 4 (auth_required).
    mock.revoke_refresh_tokens("parklands");
    mock.expire_access_tokens("parklands");
    let (code, _, err) = fam(&mock, &data, &["auth", "status", "parklands", "--check"]);
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("fam auth login parklands"), "{err}");
    // ... and shows as needing sign-in in the full list, which still exits 0.
    let v = json(&fam(&mock, &data, &["auth", "status", "--check", "--json"]));
    assert_eq!(v["accounts"][1]["state"], "auth_required");

    let (code, text, _) = fam(&mock, &data, &["auth", "status"]);
    assert_eq!(code, 0);
    assert!(
        text.contains("hotmail") && text.contains("signed in"),
        "{text}"
    );

    let v = json(&fam(&mock, &data, &["auth", "logout", "--all", "--json"]));
    assert_eq!(v["removed"], serde_json::json!(["hotmail", "parklands"]));
    let v = json(&fam(&mock, &data, &["auth", "status", "--json"]));
    assert_eq!(v["accounts"], serde_json::json!([]));
    let (code, _, _) = fam(&mock, &data, &["auth", "logout"]);
    assert_eq!(code, 2);

    let _ = std::fs::remove_dir_all(&data);
    let _g = rt.enter();
    drop(mock);
}

#[test]
fn accounts_come_from_config() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut hotmail = Mailbox::small();
    hotmail.user.user_principal_name = "marc@hotmail.example".into();
    let mut parklands = Mailbox::small();
    parklands.user.user_principal_name = "marc@parklands.example".into();
    let mock = rt
        .block_on(
            MockGraph::builder()
                .mailbox("hotmail", hotmail)
                .tenant("hotmail", CONSUMER_TENANT)
                .mailbox("parklands", parklands)
                .device_interval(0)
                .start(),
        )
        .unwrap();
    let data = data_dir("config");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("config.toml"),
        format!(
            "version = 1\n\n[accounts.hotmail]\nemail = \"marc@hotmail.example\"\ntenant = \"consumers\"\n\n\
             [accounts.parklands]\nemail = \"marc@parklands.example\"\ntenant = \"{DEFAULT_TENANT}\"\n"
        ),
    )
    .unwrap();

    // Both configured, neither signed in.
    let v = json(&fam(&mock, &data, &["auth", "status", "--json"]));
    assert_eq!(v["accounts"][0]["state"], "signed_out");
    assert_eq!(v["accounts"][1]["authority"], DEFAULT_TENANT);

    // The wrong person approving the code: Parklands can't sign in at `consumers`.
    mock.sign_in_as(Some("parklands"));
    let (code, _, err) = fam(&mock, &data, &["auth", "login", "hotmail", "--device-code"]);
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("AADSTS50020"), "{err}");

    mock.sign_in_as(Some("hotmail"));
    let v = json(&fam(
        &mock,
        &data,
        &["auth", "login", "hotmail", "--device-code", "--json"],
    ));
    assert_eq!(v["authority"], "consumers");
    let (code, text, _) = fam(&mock, &data, &["auth", "status"]);
    assert_eq!(code, 0);
    assert!(
        text.contains("signed in") && text.contains("signed out"),
        "{text}"
    );
    // A signed-out configured account checked by name needs sign-in (exit 4).
    let (code, _, _) = fam(&mock, &data, &["auth", "status", "parklands", "--check"]);
    assert_eq!(code, 4);

    let _ = std::fs::remove_dir_all(&data);
    let _g = rt.enter();
    drop(mock);
}
