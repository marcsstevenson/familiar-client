// SPDX-License-Identifier: MIT OR Apache-2.0
//! Load speed through the writer's per-message path. Ignored by default:
//! `cargo test -p familiar-store --release --test load -- --ignored --nocapture`

mod common;

use common::*;
use familiar_store::*;
use std::time::Instant;

#[test]
#[ignore]
fn load_100k_messages() {
    const N: i64 = 100_000;
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let folders: Vec<_> = (0..20)
        .map(|i| folder(&mut w, a, &format!("F{i}")))
        .collect();
    let mut rng = Lcg(42);
    let start = Instant::now();
    for b in 0..N / 5_000 {
        w.write(|tx| {
            for i in 0..5_000 {
                let n = b * 5_000 + i;
                let f = folders[rng.below(folders.len() as u64) as usize];
                let body = format!("<p>Message {n} body {}</p>", rng.next());
                let mut m = msg(
                    a,
                    f,
                    &format!("m{n}"),
                    &format!("c{}", n * 10 / 17),
                    n,
                    rng.below(5) != 0,
                );
                m.body = Some(BodyRef {
                    hash: tx.put_body(body.as_bytes())?,
                    is_html: true,
                });
                tx.upsert_message(&m)?;
            }
            Ok(())
        })
        .unwrap();
    }
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "loaded {N} messages in {secs:.1} s ({:.0}/s)",
        N as f64 / secs
    );
    assert_eq!(w.repair_derived().unwrap(), 0);
    let r = Reader::open(dir.path()).unwrap();
    let t = Instant::now();
    for f in &folders {
        r.messages(*f, None, 50).unwrap();
        r.threads(*f, None, 50).unwrap();
    }
    eprintln!(
        "first pages of {} folders, flat and threaded: {:?}",
        folders.len(),
        t.elapsed()
    );
}
