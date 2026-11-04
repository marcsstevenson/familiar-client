// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam sync status` and `fam mail pin` without a daemon (WP 1.7). The
//! daemon side is tested in `familiar-daemon`.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

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

#[test]
fn status_without_daemon_or_store() {
    let dir = std::env::temp_dir().join(format!("fam-sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let (code, out, err) = fam(&dir, &["sync", "status", "--json"]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["daemon"]["running"], false);
    assert_eq!(v["accounts"], serde_json::json!([]));
    assert_eq!(v["window"], "1y");

    let (code, out, _) = fam(&dir, &["sync", "status"]);
    assert_eq!(code, 0);
    assert!(out.contains("famd is not running"), "{out}");

    let (code, _, err) = fam(&dir, &["sync", "status", "--account", "hotmail"]);
    assert_eq!(code, 6, "{err}");

    // Pinning needs the daemon: exit code 11.
    let (code, _, err) = fam(&dir, &["mail", "pin", "42", "--json"]);
    assert_eq!(code, 11, "{err}");
    let e: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(e["error"]["kind"], "daemon_unavailable");
    let _ = std::fs::remove_dir_all(&dir);
}
