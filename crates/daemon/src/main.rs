// SPDX-License-Identifier: MIT OR Apache-2.0
//! `famd`, the sync daemon and the only writer to the store (WP 1.7).
//!
//! Reads `config.toml` (`FAMILIAR_CONFIG` or the platform default), takes
//! tokens from the sign-in store and keeps every account's store copy in
//! step with Graph until stopped with Ctrl-C or `daemon.stop`.

use std::process::ExitCode;
use std::sync::Arc;

use familiar_auth::Auth;
use familiar_core::daemon::DaemonPaths;
use familiar_daemon::policy::SyncPolicy;
use familiar_daemon::{AccountSpec, Daemon, DaemonOptions, log};
use familiar_provider_graph::{AuthTokens, ClientOptions, GraphClient};

const HELP: &str = "famd: the Familiar sync daemon

Usage: famd [--version] [--help]

Syncs every account in config.toml into the local store and answers fam and
the desktop app on a local socket (named pipe on Windows). Runs in the
foreground; stop it with Ctrl-C.

Environment: FAMILIAR_CONFIG (config path), FAMILIAR_DATA_DIR (store, socket
and token file), FAMILIAR_GRAPH_URL (another Graph, such as the mock).";

fn main() -> ExitCode {
    if let Some(a) = std::env::args().nth(1) {
        match a.as_str() {
            "--version" | "-V" => {
                println!("famd {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("famd: unknown argument {other:?}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, msg)) => {
            eprintln!("famd: {msg}");
            ExitCode::from(code)
        }
    }
}

fn run() -> Result<(), (u8, String)> {
    let config_err = |m: String| (3u8, m);
    let (path, _) = familiar_config::paths::resolve(None)
        .ok_or_else(|| config_err("no config path; set FAMILIAR_CONFIG".into()))?;
    let config =
        familiar_config::load(&path).map_err(|e| config_err(format!("{}: {e}", path.display())))?;
    if config.accounts.is_empty() {
        return Err(config_err(format!(
            "{} has no [accounts]; add one and sign in with `fam auth login`",
            path.display()
        )));
    }
    let policy = SyncPolicy::from_config(&config.sync).map_err(config_err)?;
    let paths = DaemonPaths::resolve().map_err(|e| (e.exit_code(), e.message))?;
    let auth = Arc::new(Auth::from_env().map_err(|e| (3, e.to_string()))?);
    let graph = auth.endpoints().graph.clone();

    let mut accounts = Vec::new();
    for (key, a) in &config.accounts {
        let client = GraphClient::new(
            ClientOptions::with_base(graph.clone()),
            Arc::new(AuthTokens::new(auth.clone(), key.clone())),
        )
        .map_err(|e| (1, e.to_string()))?;
        let tenant_id = familiar_config::validate::is_guid(&a.tenant).then(|| a.tenant.clone());
        accounts.push(AccountSpec {
            key: key.clone(),
            email: a.email.clone(),
            display_name: a.display_name.clone().unwrap_or_default(),
            tenant_id,
            client,
        });
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| (1, e.to_string()))?;
    rt.block_on(async move {
        let mut opts = DaemonOptions::new(paths.clone(), accounts);
        opts.policy = policy;
        let daemon = Daemon::start(opts).map_err(|e| (1, e.to_string()))?;
        log(&format!(
            "started: store {}, socket {}",
            paths.store.display(),
            paths.socket.display()
        ));
        tokio::select! {
            _ = tokio::signal::ctrl_c() => log("Ctrl-C, stopping"),
            _ = daemon.stopped() => log("asked to stop"),
        }
        daemon.stop().await;
        Ok(())
    })
}
