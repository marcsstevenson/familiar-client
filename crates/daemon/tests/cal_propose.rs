// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP AI.1: `fam cal propose` against the mock. A meeting request comes back
//! as an invitation (and says so when its event hasn't synced), `.ics`
//! attachments give their events with repeats, an event already in the
//! calendar is marked, plain text falls back to the guess, and without famd
//! only the text is read.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use common::{Env, KEY, fast_policy};
use familiar_core::commands::cal_propose::{CalProposeInput, CalProposeOutput, propose_at};
use familiar_core::events::{instant, today_in};
use familiar_daemon::policy::SyncPolicy;
use familiar_mock_graph::{Attachment, EmailAddress, Meeting, Message};

const LIMIT: Duration = Duration::from_secs(60);
const ME: &str = "test.user@example.com";
const NZ: &str = "Pacific/Auckland";

fn policy() -> SyncPolicy {
    SyncPolicy {
        calendar_interval: Duration::from_millis(300),
        ..fast_policy()
    }
}

fn now(env: &Env) -> i64 {
    env.now.load(Ordering::SeqCst)
}

fn day(env: &Env, ahead: i64) -> NaiveDate {
    today_in(now(env), NZ) + chrono::Duration::days(ahead)
}

fn at(d: NaiveDate, h: u32, m: u32) -> i64 {
    instant(d, NaiveTime::from_hms_opt(h, m, 0).unwrap(), NZ)
}

fn utc(t: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(t, 0).unwrap()
}

fn mail(env: &Env, subject: &str, html: &str) -> Message {
    let mut m = Message::new(
        "inbox",
        subject,
        EmailAddress::new("Jo Smith", "jo@club.example"),
        utc(now(env) - 3600),
    );
    m.body_html = Some(html.to_string());
    m
}

/// Waits until the message has synced with its body; returns its local ID.
async fn synced(env: &Env, remote: &str) -> i64 {
    let mut id = 0;
    env.until("the email", LIMIT, |e| {
        match e.reader().message_by_remote(e.account(), remote).unwrap() {
            Some(m) if m.body.is_some() => {
                id = m.id.0;
                true
            }
            _ => false,
        }
    })
    .await;
    id
}

async fn propose(env: &Env, message: i64) -> CalProposeOutput {
    let p = env.paths.clone();
    let t = now(env);
    tokio::task::spawn_blocking(move || {
        propose_at(
            &p,
            &CalProposeInput {
                from_mail: message,
                tz: Some(NZ.into()),
            },
            t,
        )
    })
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proposals_from_invitations_attachments_and_text() {
    let mut env = Env::new(20).await;
    let d3 = day(&env, 3);
    let d6 = day(&env, 6);

    // A meeting, and the request for it.
    let mut meeting = familiar_mock_graph::Event::new(
        "Committee meeting",
        utc(at(d3, 19, 0)),
        utc(at(d3, 20, 30)),
    );
    meeting.organizer = Some(EmailAddress::new("Jo Smith", "jo@club.example"));
    meeting.attendees = vec![EmailAddress::new("Test User", ME)];
    let meeting_id = env.mock.mailbox(KEY).add_event(meeting).unwrap();
    let mut request = mail(&env, "Committee meeting", "<p>Please come.</p>");
    request.meeting = Some(Meeting {
        meeting_message_type: "meetingRequest".into(),
        event_id: meeting_id,
    });
    let request = env.mock.mailbox(KEY).add_message(request).unwrap();
    // A request whose event never reaches this device.
    let mut orphan = mail(&env, "Other meeting", "<p>Please come.</p>");
    orphan.meeting = Some(Meeting {
        meeting_message_type: "meetingRequest".into(),
        event_id: "no-such-event".into(),
    });
    let orphan = env.mock.mailbox(KEY).add_message(orphan).unwrap();

    // Already in the calendar: the prizegiving on day 6 at 6:30 pm.
    env.mock
        .mailbox(KEY)
        .add_event(familiar_mock_graph::Event::new(
            "Prizegiving",
            utc(at(d6, 18, 30)),
            utc(at(d6, 21, 0)),
        ))
        .unwrap();

    // A booking with an .ics: the prizegiving again, and weekly nets.
    let ymd = |d: NaiveDate| d.format("%Y%m%d").to_string();
    let ics = format!(
        "BEGIN:VCALENDAR\r\nMETHOD:PUBLISH\r\nBEGIN:VEVENT\r\nUID:prize-1\r\nSUMMARY:Prizegiving\r\nDTSTART;TZID=New Zealand Standard Time:{}T183000\r\nDTEND;TZID=New Zealand Standard Time:{}T210000\r\nLOCATION:Pavilion\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:nets-1\r\nSUMMARY:Junior nets\r\nDTSTART;TZID=Pacific/Auckland:{}T160000\r\nDURATION:PT1H\r\nRRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=8\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ymd(d6),
        ymd(d6),
        ymd(d3)
    );
    let mut booking = mail(&env, "Your bookings", "<p>See the attached calendar.</p>");
    booking.has_attachments = true;
    booking.attachments = vec![Attachment {
        id: "att-ics".into(),
        name: "bookings.ics".into(),
        content_type: "text/calendar".into(),
        is_inline: false,
        content_id: None,
        content_bytes: base64::engine::general_purpose::STANDARD.encode(ics),
    }];
    let booking = env.mock.mailbox(KEY).add_message(booking).unwrap();

    // Same subject, overlapping but starting earlier: not the same event.
    env.mock
        .mailbox(KEY)
        .add_event(familiar_mock_graph::Event::new(
            "Quiz night",
            utc(at(d6, 18, 0)),
            utc(at(d6, 22, 0)),
        ))
        .unwrap();
    // A cancellation from outside Exchange, repeating the date it cancels.
    let mut cancel = mail(
        &env,
        "Cancelled: Quiz night",
        &format!(
            "<p>Quiz night on {} at 7:30 pm is off.</p>",
            d6.format("%A %-d %B")
        ),
    );
    cancel.has_attachments = true;
    cancel.attachments = vec![Attachment {
        id: "att-cancel".into(),
        name: "invite.ics".into(),
        content_type: "text/calendar".into(),
        is_inline: false,
        content_id: None,
        content_bytes: base64::engine::general_purpose::STANDARD.encode(format!(
            "BEGIN:VCALENDAR\r\nMETHOD:CANCEL\r\nBEGIN:VEVENT\r\nUID:quiz\r\nSUMMARY:Quiz night\r\nDTSTART:{}T063000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            ymd(d6)
        )),
    }];
    let cancel = env.mock.mailbox(KEY).add_message(cancel).unwrap();

    // Plain text only.
    let plain = mail(
        &env,
        "FW: Quiz night",
        &format!(
            "<p>Quiz night is on {} at 7:30 pm.</p><p>Where: Clubrooms</p>",
            d6.format("%A %-d %B")
        ),
    );
    let plain = env.mock.mailbox(KEY).add_message(plain).unwrap();

    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    let request = synced(&env, &request).await;
    let orphan = synced(&env, &orphan).await;
    let booking = synced(&env, &booking).await;
    let plain = synced(&env, &plain).await;
    let cancel = synced(&env, &cancel).await;
    env.until("the calendar", LIMIT, |e| {
        let r = e.reader();
        let cals: Vec<_> = r
            .calendars(e.account())
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        r.events_in(
            &cals,
            familiar_store::EventRange::utc(at(d3, 0, 0), at(d6, 23, 0)),
        )
        .unwrap()
        .len()
            >= 2
    })
    .await;

    // The meeting request is answered, not copied.
    let out = propose(&env, request).await;
    assert_eq!(out.source, "invite", "{out:?}");
    assert!(out.events.is_empty());
    let inv = out.invitation.unwrap();
    assert_eq!(inv.kind, "request");
    assert_eq!(inv.subject.as_deref(), Some("Committee meeting"));
    assert!(inv.event.is_some());

    // Its event hasn't synced: still an invitation, with a note.
    let out = propose(&env, orphan).await;
    assert_eq!(out.source, "invite");
    assert!(out.events.is_empty());
    assert_eq!(out.invitation.unwrap().event, None);
    assert!(
        out.notes[0].contains("hasn't reached your calendar"),
        "{:?}",
        out.notes
    );

    // The .ics: both events, the repeat, and the one already there.
    let out = propose(&env, booking).await;
    assert_eq!(out.source, "ics", "{out:?}");
    assert_eq!(out.events.len(), 2);
    let prize = &out.events[0];
    assert_eq!(prize.start, format!("{} 18:30", d6.format("%Y-%m-%d")));
    assert_eq!(
        prize.end.as_deref(),
        Some(format!("{} 21:00", d6.format("%Y-%m-%d")).as_str())
    );
    assert_eq!(prize.time_zone, NZ);
    assert_eq!(prize.location.as_deref(), Some("Pavilion"));
    assert!(prize.already.is_some(), "{prize:?}");
    let nets = &out.events[1];
    assert_eq!(
        nets.end.as_deref(),
        Some(format!("{} 17:00", d3.format("%Y-%m-%d")).as_str())
    );
    let rep = nets.repeat.as_ref().unwrap();
    assert_eq!(
        (rep.every.as_str(), rep.on.as_slice(), rep.count),
        ("weekly", &["tue".to_string()][..], Some(8))
    );
    assert_eq!(nets.already, None);

    // Text only: the guess.
    let out = propose(&env, plain).await;
    assert_eq!(out.source, "heuristic", "{out:?}");
    assert_eq!(out.events[0].subject, "Quiz night");
    assert_eq!(
        out.events[0].start,
        format!("{} 19:30", d6.format("%Y-%m-%d"))
    );
    assert_eq!(out.events[0].location.as_deref(), Some("Clubrooms"));
    assert_eq!(
        out.events[0].already, None,
        "an overlapping event isn't this one"
    );

    // A cancellation: nothing to add, not even from its text.
    let out = propose(&env, cancel).await;
    assert!(out.events.is_empty(), "{out:?}");
    assert!(
        out.notes.iter().any(|n| n.contains("cancels")),
        "{:?}",
        out.notes
    );

    // Without famd, the booking's text is all there is to go on.
    env.stop().await;
    let out = propose(&env, booking).await;
    assert_eq!(out.source, "none", "{out:?}");
    assert!(
        out.notes
            .iter()
            .any(|n| n.starts_with("famd isn't running")),
        "{:?}",
        out.notes
    );
}
