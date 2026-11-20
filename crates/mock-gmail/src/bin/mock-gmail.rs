// SPDX-License-Identifier: MIT OR Apache-2.0
//! Runs the mock Gmail server from the command line, for poking at it with curl or
//! pointing a debug build of `famd` at it.
//!
//! ```text
//! mock-gmail [--port 8788] [--synthetic 10000 [--seed 1]] [--token dev-token] [--latency-ms 0]
//! ```

use std::time::Duration;

use familiar_mock_gmail::{Mailbox, MockGmail};

fn usage() -> ! {
    eprintln!(
        "usage: mock-gmail [--port N] [--synthetic N [--seed N]] [--token TOKEN] [--latency-ms N]"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut port = 8788u16;
    let mut synthetic = None;
    let mut seed = 1u64;
    let mut token = "dev-token".to_string();
    let mut latency = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--port" => port = val().parse().unwrap_or_else(|_| usage()),
            "--synthetic" => synthetic = Some(val().parse().unwrap_or_else(|_| usage())),
            "--seed" => seed = val().parse().unwrap_or_else(|_| usage()),
            "--token" => token = val(),
            "--latency-ms" => latency = val().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    let mailbox = match synthetic {
        Some(n) => Mailbox::synthetic(n, seed),
        None => Mailbox::small(),
    };
    let mock = MockGmail::builder()
        .mailbox("default", mailbox)
        .token(token.clone(), "default")
        .port(port)
        .latency(Duration::from_millis(latency))
        .start()
        .await?;
    println!("mock Gmail listening on {}", mock.api_url());
    println!(
        "try: curl --oauth2-bearer {token} '{}/users/me/labels'",
        mock.api_url()
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
