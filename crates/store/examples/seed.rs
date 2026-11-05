// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fills a store with synthetic mail, for trying the desktop app and timing it
//! at scale without a real account. Nothing in it is real.
//!
//! ```sh
//! cargo run --release -p familiar-store --example seed -- <data dir>/store 500000
//! ```

use familiar_store::*;

const FIRST: &[&str] = &[
    "Ana", "Ben", "Chloe", "Dev", "Ema", "Finn", "Grace", "Hugo", "Isla", "Jack", "Kiri", "Liam",
];
const LAST: &[&str] = &[
    "Brown", "Ngata", "Patel", "Smith", "Tane", "Walker", "Young", "Zhou",
];
const TOPICS: &[&str] = &[
    "Invoice",
    "Team meeting",
    "Weekend plans",
    "School newsletter",
    "Your order has shipped",
    "Quarterly report",
    "Re: question about the roster",
    "Photos from Saturday",
    "Reminder",
];

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(count)) = (args.next(), args.next()) else {
        eprintln!("usage: seed <store dir> <messages>");
        std::process::exit(2);
    };
    let count: usize = count.parse().expect("message count");
    let mut w = Writer::open(std::path::Path::new(&root)).expect("open store");
    let mut rng = Rng(7);
    let now = 1_793_000_000_i64;
    let mut targets = Vec::new();
    for key in ["home", "work"] {
        let (account, folders) = w
            .write(|tx| {
                let a = tx.upsert_account(&NewAccount {
                    key: key.into(),
                    email: format!("{key}@example.com"),
                    display_name: key.into(),
                    ..Default::default()
                })?;
                let f = |name: &str, wk: Option<&str>, parent| {
                    tx.upsert_folder(&NewFolder {
                        account: a,
                        remote_id: format!("{key}-{name}"),
                        parent,
                        name: name.into(),
                        well_known: wk.map(str::to_string),
                    })
                };
                let inbox = f("Inbox", Some("inbox"), None)?;
                let club = f("Club", None, Some(inbox))?;
                let archive = f("Archive", Some("archive"), None)?;
                f("Drafts", Some("drafts"), None)?;
                f("Sent Items", Some("sentitems"), None)?;
                f("Deleted Items", Some("deleteditems"), None)?;
                Ok((a, vec![inbox, inbox, inbox, archive, club]))
            })
            .expect("folders");
        targets.push((account, folders));
    }
    let batch = 20_000;
    let mut done = 0;
    while done < count {
        let n = batch.min(count - done);
        w.write(|tx| {
            for i in done..done + n {
                let (account, folders) = &targets[rng.below(targets.len())];
                let folder = folders[rng.below(folders.len())];
                let first = FIRST[rng.below(FIRST.len())];
                let last = LAST[rng.below(LAST.len())];
                let topic = TOPICS[rng.below(TOPICS.len())];
                tx.upsert_message(&NewMessage {
                    account: *account,
                    folder,
                    remote_id: format!("m{i}"),
                    conversation_id: format!("c{}", i / 3),
                    subject: format!("{topic} #{}", i % 997),
                    from: Address {
                        name: format!("{first} {last}"),
                        addr: format!(
                            "{}.{}@example.com",
                            first.to_lowercase(),
                            last.to_lowercase()
                        ),
                    },
                    received_at: now - (count - i) as i64 * 60,
                    is_read: rng.below(10) > 1,
                    is_flagged: rng.below(40) == 0,
                    importance: 1,
                    has_attachments: rng.below(8) == 0,
                    preview: format!(
                        "Kia ora, a short preview line for message {i} about {topic}."
                    ),
                    ..Default::default()
                })?;
            }
            Ok(())
        })
        .expect("messages");
        done += n;
        eprintln!("{done}/{count}");
    }
}
