// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs the mock Graph server from the command line, for poking at it with curl or
//! pointing a debug build of `famd` at it.
//!
//! ```text
//! mock-graph [--port 8787] [--fixture mailbox.json | --synthetic 10000 [--seed 1]]
//!            [--recording recorded.json] [--token dev-token] [--latency-ms 0]
//! ```

use std::time::Duration;

use familiar_mock_graph::{Mailbox, MockGraph, Recording, SyntheticSpec};

fn usage() -> ! {
    eprintln!(
        "usage: mock-graph [--port N] [--fixture FILE | --synthetic N [--seed N]] \
         [--recording FILE] [--token TOKEN] [--latency-ms N]"
    );
    std::process::exit(2)
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1)
    })
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut port = 8787u16;
    let mut mailbox = None;
    let mut synthetic = None;
    let mut seed = 1u64;
    let mut recording = None;
    let mut token = "dev-token".to_string();
    let mut latency = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--port" => port = val().parse().unwrap_or_else(|_| usage()),
            "--fixture" => {
                mailbox = Some(Mailbox::from_json(&read(&val())).unwrap_or_else(|e| {
                    eprintln!("bad fixture: {e}");
                    std::process::exit(1)
                }))
            }
            "--synthetic" => synthetic = Some(val().parse().unwrap_or_else(|_| usage())),
            "--seed" => seed = val().parse().unwrap_or_else(|_| usage()),
            "--recording" => {
                recording = Some(Recording::from_json(&read(&val())).unwrap_or_else(|e| {
                    eprintln!("bad recording: {e}");
                    std::process::exit(1)
                }))
            }
            "--token" => token = val(),
            "--latency-ms" => latency = val().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    let mailbox = match (mailbox, synthetic) {
        (Some(m), _) => m,
        (None, Some(n)) => Mailbox::synthetic(&SyntheticSpec {
            seed,
            ..SyntheticSpec::with_messages(n)
        }),
        (None, None) => Mailbox::small(),
    };
    let mut b = MockGraph::builder()
        .mailbox("default", mailbox)
        .token(token.clone(), "default")
        .port(port)
        .latency(Duration::from_millis(latency));
    if let Some(r) = recording {
        b = b.recording(r);
    }
    let mock = b.start().await?;
    println!("mock Graph listening on {}", mock.base_url());
    println!(
        "try: curl --oauth2-bearer {token} {}/me/mailFolders",
        mock.base_url()
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
