// SPDX-License-Identifier: MIT OR Apache-2.0
//! The small hand-written mailbox and generated mailboxes of any size.

use chrono::{Duration, TimeZone, Utc};

use crate::model::{Label, Mailbox, Message, SendAs};

/// The small fixture's address.
pub const SMALL_EMAIL: &str = "kea.tester@gmail.com";

pub(crate) fn small() -> Mailbox {
    let me = SMALL_EMAIL;
    let day = |d: u32, h: u32| Utc.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap();
    Mailbox::new(me, "Kea Tester")
        .label(Label::user("Clients").id("Label_1"))
        .label(
            Label::user("Clients/Acme")
                .id("Label_2")
                .color("#ffffff", "#4a86e8"),
        )
        .label(Label::user("Receipts").id("Label_3"))
        .label(Label::user("Old projects").id("Label_4").hidden())
        .send_as(SendAs::alias("kea@familiarclient.com", "Kea Tester").signature("<p>Kea</p>"))
        .send_as(SendAs::alias("old-kea@example.com", "Kea Tester").pending())
        // A thread across the Inbox, a user label and Sent.
        .message(
            Message::new(
                "Ann Acme <ann@acme.example>",
                me,
                "Quote for the new site",
                "Here is the quote we talked about.",
            )
            .id("msg-acme-1")
            .message_id("acme-1@acme.example")
            .labels(&["INBOX", "Label_1", "Label_2", "CATEGORY_PERSONAL"])
            .at(day(20, 9)),
        )
        .message(
            Message::new(
                &format!("Kea Tester <{me}>"),
                "ann@acme.example",
                "Re: Quote for the new site",
                "Thanks, looks good.",
            )
            .id("msg-acme-2")
            .message_id("acme-2@mock.gmail")
            .in_reply_to("acme-1@acme.example")
            .labels(&["SENT", "Label_2"])
            .at(day(20, 11)),
        )
        .message(
            Message::new(
                "Ann Acme <ann@acme.example>",
                me,
                "Re: Quote for the new site",
                "Great, we start Monday.",
            )
            .id("msg-acme-3")
            .message_id("acme-3@acme.example")
            .in_reply_to("acme-2@mock.gmail")
            .labels(&[
                "INBOX",
                "UNREAD",
                "Label_2",
                "IMPORTANT",
                "CATEGORY_PERSONAL",
            ])
            .at(day(21, 8)),
        )
        // Inbox mail, read and unread, starred, with an attachment, promotions.
        .message(
            Message::new(
                "Bob <bob@example.com>",
                me,
                "Dinner on Friday",
                "Are you free for dinner on Friday?",
            )
            .id("msg-dinner")
            .labels(&["INBOX", "UNREAD", "CATEGORY_PERSONAL"])
            .at(day(25, 18)),
        )
        .message(
            Message::new(
                "Cleo <cleo@example.com>",
                me,
                "Photos from the trip",
                "Photos attached.",
            )
            .id("msg-photos")
            .attach("beach.jpg", b"\xff\xd8\xff\xe0mock-jpeg")
            .labels(&["INBOX", "STARRED", "CATEGORY_PERSONAL"])
            .at(day(26, 10)),
        )
        .message(
            Message::new(
                "Shop <deals@shop.example>",
                me,
                "50% off this week",
                "Our biggest sale of the year.",
            )
            .id("msg-promo")
            .labels(&["INBOX", "UNREAD", "CATEGORY_PROMOTIONS"])
            .at(day(27, 7)),
        )
        // Archived: a receipt with a user label, and one with no label at all (All Mail only).
        .message(
            Message::new(
                "Store <receipts@store.example>",
                me,
                "Your receipt",
                "Total: $42.00",
            )
            .id("msg-receipt")
            .labels(&["Label_3", "CATEGORY_UPDATES"])
            .at(day(15, 12)),
        )
        .message(
            Message::new(
                "Dan <dan@example.com>",
                me,
                "Old chat",
                "Archived without a label.",
            )
            .id("msg-archived")
            .labels(&["CATEGORY_PERSONAL"])
            .at(day(10, 12)),
        )
        // Mail from 2015 under a hidden label.
        .message(
            Message::new(
                "Eve <eve@example.com>",
                me,
                "Project kickoff 2015",
                "Notes from the kickoff.",
            )
            .id("msg-2015")
            .labels(&["Label_4"])
            .at(Utc.with_ymd_and_hms(2015, 3, 2, 9, 0, 0).unwrap()),
        )
        // Spam, Trash and a draft.
        .message(
            Message::new(
                "Prize <win@spam.example>",
                me,
                "You have won",
                "Claim your prize now.",
            )
            .id("msg-spam")
            .labels(&["SPAM", "UNREAD"])
            .at(day(28, 3)),
        )
        .message(
            Message::new(
                "Fay <fay@example.com>",
                me,
                "Unwanted newsletter",
                "Deleted already.",
            )
            .id("msg-trash")
            .labels(&["TRASH", "CATEGORY_UPDATES"])
            .at(day(22, 3)),
        )
        .message(
            Message::new(
                &format!("Kea Tester <{me}>"),
                "bob@example.com",
                "Re: Dinner on Friday",
                "Sounds good, I",
            )
            .id("msg-draft")
            .labels(&["DRAFT"])
            .at(day(25, 19)),
        )
}

pub(crate) fn synthetic(n: usize, seed: u64) -> Mailbox {
    let me = "synthetic@gmail.com";
    let mut mb = Mailbox::new(me, "Synthetic User")
        .label(Label::user("Work").id("Label_1"))
        .label(Label::user("Work/Reports").id("Label_2"))
        .label(Label::user("Family").id("Label_3"));
    let mut rng = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let mut next = || {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        rng >> 33
    };
    let start = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let mut parent: Option<String> = None;
    for i in 0..n {
        let r = next();
        let mid = format!("syn-{seed}-{i}@mock.gmail");
        let reply = parent.is_some() && r % 3 != 0;
        let mut labels = vec!["INBOX"];
        if r % 2 == 0 {
            labels.push("UNREAD");
        }
        match r % 7 {
            0 => labels.push("Label_1"),
            1 => labels.push("Label_2"),
            2 => labels.push("Label_3"),
            3 => labels = vec!["Label_1"],
            _ => {}
        }
        let subject = format!("Synthetic message {}", if reply { i - 1 } else { i });
        let mut m = Message::new(
            &format!("Sender {} <sender{}@example.com>", r % 50, r % 50),
            me,
            &subject,
            &format!("Body of synthetic message {i}, seed {seed}."),
        )
        .message_id(&mid)
        .labels(&labels)
        .at(start - Duration::minutes(10 * i as i64));
        if reply && let Some(p) = &parent {
            m = m.in_reply_to(p);
        }
        parent = if reply || i % 3 == 2 { None } else { Some(mid) };
        mb = mb.message(m);
    }
    mb
}
