// SPDX-License-Identifier: MIT OR Apache-2.0
//! First run (onboarding WP OB.1): famd starts with no config file and with a
//! config that has no accounts, so the desktop app has a daemon to talk to
//! while the person adds their first account.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use familiar_core::daemon::{Client, DaemonPaths, METHOD_STATUS, METHOD_STOP};
use serde_json::{Value, json};

/// Starts the real famd binary on `data` with `config` as its config path.
fn famd(data: &Path, config: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_famd"))
        .env("FAMILIAR_DATA_DIR", data)
        .env("FAMILIAR_CONFIG", config)
        .env("FAMILIAR_TOKEN_STORE", "file")
        .env("FAMILIAR_TOKEN_PASSPHRASE", "test-passphrase")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start famd")
}

/// Waits for famd to answer on its socket, failing if it exits first.
fn connect(child: &mut Child, socket: &Path) -> Client {
    let start = Instant::now();
    loop {
        if let Ok(c) = Client::connect_with_timeout(socket, Duration::from_secs(5)) {
            return c;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("famd exited with {status} instead of waiting for an account");
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "famd never answered"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn starts_and_stops(config: &Path, data: &Path) {
    let socket = DaemonPaths::at(data).socket;
    let mut child = famd(data, config);
    let mut client = connect(&mut child, &socket);
    let status: Value = client.call(METHOD_STATUS, &json!({})).unwrap();
    assert!(status.is_object(), "status: {status}");
    let _: Value = client.call(METHOD_STOP, &json!({})).unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "famd didn't stop"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn starts_without_a_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    starts_and_stops(&config, &dir.path().join("data"));
    assert!(!config.exists(), "famd must not create the config");
}

#[test]
fn starts_with_a_config_that_has_no_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "version = 1\n").unwrap();
    starts_and_stops(&config, &dir.path().join("data"));
}

#[test]
fn still_refuses_an_invalid_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "version = 1\n[accounts.x]\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_famd"))
        .env("FAMILIAR_DATA_DIR", dir.path().join("data"))
        .env("FAMILIAR_CONFIG", &config)
        .env("FAMILIAR_TOKEN_STORE", "file")
        .env("FAMILIAR_TOKEN_PASSPHRASE", "test-passphrase")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
}
