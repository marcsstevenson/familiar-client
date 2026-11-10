// SPDX-License-Identifier: MIT OR Apache-2.0
//! `[[hooks]]` against the mock: famd runs a hook for mail that arrives
//! after it started, only when `match` fits, with the event on stdin and in
//! the environment, and never with mail content in the command line.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{Env, KEY, fast_policy};
use familiar_daemon::hooks::Hooks;
use familiar_mock_graph::{EmailAddress, Message};

const LIMIT: Duration = Duration::from_secs(60);

fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn message(env: &Env, subject: &str, from: &str) -> Message {
    let now = env.now.load(Ordering::SeqCst);
    let mut m = Message::new(
        "inbox",
        subject,
        EmailAddress::new("Sender", from),
        chrono::DateTime::from_timestamp(now - 60, 0).unwrap(),
    );
    m.body_html = Some("<p>Training moves to Thursday.</p>".into());
    m
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hooks_run_on_new_mail_only() {
    let mut env = Env::new(5).await;
    let out = env.dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let o = out.display();
    let config = env.dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"version = 1

[accounts.{KEY}]
email = "pat@example.com"
tenant = "consumers"

[[hooks]]
name = "club"
on = "new_mail"
match = "from:club"
run = "cat >> '{o}/club.jsonl'"

[[hooks]]
on = "new_mail"
run = "printf '%s|%s\n' \"$FAMILIAR_SUBJECT\" \"$FAMILIAR_ACCOUNT\" >> '{o}/all.txt'"

[[hooks]]
on = "new_mail"
match = "body:thursday"
run = "echo \"$FAMILIAR_MESSAGE_ID\" >> '{o}/body.txt'"

[[hooks]]
on = "new_mail"
enabled = false
run = "touch '{o}/disabled'"
"#
        ),
    )
    .unwrap();
    let mut opts = env.options(fast_policy());
    opts.hooks = Some(Hooks {
        config,
        every: Duration::from_millis(200),
    });
    env.start_with(opts);
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    // Let the hooks take their starting point after the first sync.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        lines(&out.join("all.txt")).is_empty(),
        "old mail ran a hook"
    );

    let mb = env.mock.mailbox(KEY);
    mb.add_message(message(&env, "Club night", "news@club.example.org"))
        .unwrap();
    // A subject that would run something if it reached a shell.
    mb.add_message(message(
        &env,
        "Hi $(touch pwned) `touch pwned`",
        "ann@example.com",
    ))
    .unwrap();

    env.until("both hooks", LIMIT, |_| {
        lines(&out.join("all.txt")).len() == 2
            && lines(&out.join("club.jsonl")).len() == 1
            && lines(&out.join("body.txt")).len() == 2
    })
    .await;
    // Give a wrong extra run time to show.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut all = lines(&out.join("all.txt"));
    all.sort();
    assert_eq!(
        all,
        vec![
            format!("Club night|{KEY}"),
            format!("Hi $(touch pwned) `touch pwned`|{KEY}"),
        ]
    );
    let club: serde_json::Value = serde_json::from_str(&lines(&out.join("club.jsonl"))[0]).unwrap();
    assert_eq!(club["event"], "new_mail");
    assert_eq!(club["subject"], "Club night");
    assert_eq!(club["from"]["addr"], "news@club.example.org");
    assert_eq!(club["folder"], "Inbox");
    assert_eq!(club["account"], KEY);
    assert_eq!(lines(&out.join("body.txt")).len(), 2);
    assert!(!out.join("disabled").exists());
    assert!(!out.join("pwned").exists());
    assert!(!Path::new("pwned").exists());
    env.stop().await;
}
