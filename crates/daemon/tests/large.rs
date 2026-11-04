// SPDX-License-Identifier: MIT OR Apache-2.0
//! The WP 1.7 done criterion: a large synthetic mailbox syncs from the mock
//! and stays consistent through a burst of changes.
//!
//! CI runs 20,000 messages. The full 500,000 is ignored by default because
//! it takes minutes; run it in release:
//!
//! ```sh
//! cargo test --release -p familiar-daemon --test large -- --ignored --nocapture
//! ```
//!
//! `FAMILIAR_LARGE_MESSAGES` and `FAMILIAR_LARGE_WINDOW_DAYS` change its size.

mod common;

use std::time::{Duration, Instant};

use chrono::Duration as Span;
use common::{Env, KEY, fast_policy};
use familiar_mock_graph::{EmailAddress, Message};

fn env_or(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

async fn large(messages: usize, window_days: u32, limit: Duration) {
    let mut env = Env::new(messages).await;
    let mut policy = fast_policy();
    policy.window_days = window_days;
    let t = Instant::now();
    env.start(policy);
    env.until("first sync", limit, |e| e.settled(window_days))
        .await;
    let first = t.elapsed();
    let r = env.reader();
    let acc = env.account();
    let stats = r.body_stats(acc, env.window_start(window_days)).unwrap();
    let t = Instant::now();
    let diff = env.differences(window_days);
    let check = t.elapsed();
    assert!(diff.is_empty(), "{diff:#?}");
    assert_eq!(stats.messages as usize, messages);

    // A burst of changes across the mailbox: 1% of messages touched.
    let mbx = env.mock.mailbox(KEY);
    let inbox = mbx.message_ids("inbox");
    let n = (messages / 100).max(10);
    let newest = env.spec.newest;
    for i in 0..n / 4 {
        mbx.add_message(Message::new(
            "inbox",
            format!("Burst {i}"),
            EmailAddress::new("Ann", "ann@example.com"),
            newest + Span::seconds(i as i64 + 1),
        ))
        .unwrap();
    }
    let step = (inbox.len() / n).max(1);
    for (k, id) in inbox.iter().step_by(step).take(n).enumerate() {
        match k % 4 {
            0 => {
                mbx.move_message(id, "archive").unwrap();
            }
            1 => mbx.delete_message(id).unwrap(),
            2 => mbx.update_message(id, |m| m.is_read = !m.is_read).unwrap(),
            _ => {
                mbx.move_message(id, "fld-user-002").unwrap();
            }
        }
    }
    let t = Instant::now();
    env.until("the burst", limit, |e| {
        e.settled(window_days) && e.differences(window_days).is_empty()
    })
    .await;
    let burst = t.elapsed();
    assert_eq!(env.daemon().writer().lock().repair_derived().unwrap(), 0);
    let live = env.daemon().service().status().accounts[0].clone();
    assert_eq!(live.failures, 0, "{live:?}");
    let disk = r.stored_body_bytes().unwrap();
    eprintln!(
        "large sync: {messages} messages, {} window bodies ({window_days} days), first sync {:.1}s, \
         consistency check {:.1}s, {n} changes applied in {:.1}s, bodies {:.1} MB stored, \
         {} requests",
        stats.with_body,
        first.as_secs_f64(),
        check.as_secs_f64(),
        burst.as_secs_f64(),
        disk as f64 / 1_048_576.0,
        env.mock.requests().len()
    );
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_thousand_messages_sync_and_stay_consistent() {
    large(20_000, 30, Duration::from_secs(240)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "takes minutes; run in release with --ignored"]
async fn five_hundred_thousand_messages_sync_and_stay_consistent() {
    let messages = env_or("FAMILIAR_LARGE_MESSAGES", 500_000);
    let window = env_or("FAMILIAR_LARGE_WINDOW_DAYS", 30) as u32;
    large(messages, window, Duration::from_secs(3600)).await;
}
