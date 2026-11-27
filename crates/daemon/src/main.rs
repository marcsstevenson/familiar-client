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
use familiar_daemon::{AccountClient, AccountSpec, Daemon, DaemonOptions, log};
use familiar_provider_graph::{AuthTokens, ClientOptions, GraphClient};

const HELP: &str = "famd: the Familiar sync daemon

Usage: famd [--version] [--help]

Syncs every account in config.toml into the local store and answers fam and
the desktop app on a local socket (named pipe on Windows). Runs in the
foreground; stop it with Ctrl-C.

Environment: FAMILIAR_CONFIG (config path), FAMILIAR_DATA_DIR (store, socket
and token file), FAMILIAR_GRAPH_URL and FAMILIAR_GMAIL_URL (another Graph or Gmail, such as
the mocks).";

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
    // A new install has no config yet. famd still starts, so the desktop app
    // has a daemon to talk to while the person adds their first account; the
    // app restarts famd once the config names one.
    let config = if path.exists() {
        familiar_config::load(&path).map_err(|e| config_err(format!("{}: {e}", path.display())))?
    } else {
        familiar_config::Config::default()
    };
    if config.accounts.is_empty() {
        log(&format!(
            "no accounts in {} yet; add one and restart famd to start syncing",
            path.display()
        ));
    }
    let policy = SyncPolicy::from_config(&config.sync)
        .and_then(|p| p.with_calendar(&config.calendar))
        .map_err(config_err)?;
    let paths = DaemonPaths::resolve().map_err(|e| (e.exit_code(), e.message))?;
    keep_private(&paths.data_dir);
    let auth = Arc::new(Auth::from_env().map_err(|e| (3, e.to_string()))?);
    let graph = auth.endpoints().graph.clone();
    let gmail = auth.endpoints().gmail.clone();

    let mut accounts = Vec::new();
    for (key, a) in &config.accounts {
        let tokens = Arc::new(AuthTokens::new(auth.clone(), key.clone()));
        let client: AccountClient = match a.provider {
            familiar_config::Provider::Graph => {
                GraphClient::new(ClientOptions::with_base(graph.clone()), tokens)
                    .map_err(|e| (1, e.to_string()))?
                    .into()
            }
            familiar_config::Provider::Gmail => familiar_provider_gmail::GmailClient::new(
                familiar_provider_gmail::ClientOptions::with_base(gmail.clone()),
                tokens,
            )
            .map_err(|e| (1, e.to_string()))?
            .into(),
        };
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
        opts.agent = familiar_daemon::AgentGrants::File(path.clone());
        opts.spam = Some(familiar_daemon::spam::SpamAuto::new(path.clone()));
        opts.hooks = Some(familiar_daemon::hooks::Hooks::new(path.clone()));
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

/// Makes the data directory (store, bodies, search index, socket) readable by
/// this user only. On Unix it would otherwise be created with the umask,
/// usually 0755, so other local users could read the mail. Windows folders
/// under the user's profile are already private. Fails open with a log line.
fn keep_private(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let done = std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)));
        if let Err(e) = done {
            log(&format!(
                "couldn't make {} private (0700): {e}",
                dir.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}
