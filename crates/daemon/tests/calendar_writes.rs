// SPDX-License-Identifier: MIT OR Apache-2.0
//! WP 2.4: calendar writes against the mock. Events are added (to a
//! purpose's calendar, from an email, recurring), changed, answered and
//! deleted; each shows locally at once, reaches the server through famd's
//! op runner, and the store agrees with the mock afterwards. Offline adds
//! wait for famd, and refusals are undone.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use common::{Env, KEY, fast_policy};
use familiar_config::Config;
use familiar_config::model::Purpose;
use familiar_core::ErrorKind;
use familiar_core::commands::cal_write::{
    CalAddInput, CalDeleteInput, CalEditInput, CalRsvpInput, add_at, delete_request, edit_request,
    rsvp_request,
};
use familiar_core::commands::write::{WriteFlags, submit_at};
use familiar_core::events::{instant, today_in};
use familiar_core::ops::{WriteOutput, WriteRequest};
use familiar_daemon::policy::SyncPolicy;
use familiar_mock_graph::{EmailAddress, Fault, Message};
use familiar_store::{EventId, EventRange, LOCAL_REMOTE_PREFIX, OpState};

const LIMIT: Duration = Duration::from_secs(60);
const ME: &str = "test.user@example.com";
const NZ: &str = "Pacific/Auckland";

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn policy() -> SyncPolicy {
    SyncPolicy {
        op_retry_min: Duration::from_millis(100),
        op_retry_max: Duration::from_millis(500),
        calendar_interval: Duration::from_millis(300),
        ..fast_policy()
    }
}

fn waiting() -> WriteFlags {
    WriteFlags {
        wait: true,
        wait_secs: Some(30),
        ..WriteFlags::default()
    }
}

fn config() -> Config {
    let mut c = Config::default();
    c.calendar.purposes = BTreeMap::from([(
        "family".to_string(),
        Purpose {
            account: KEY.into(),
            calendar: "Family Room".into(),
        },
    )]);
    c
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

async fn add(
    env: &Env,
    i: CalAddInput,
    flags: WriteFlags,
) -> Result<WriteOutput, familiar_core::CommandError> {
    let p = env.paths.clone();
    let t = now(env);
    blocking(move || add_at(&p, Some(&config()), &i, NZ, t, &flags)).await
}

async fn submit(
    env: &Env,
    req: WriteRequest,
    flags: WriteFlags,
) -> Result<WriteOutput, familiar_core::CommandError> {
    let p = env.paths.clone();
    blocking(move || submit_at(&p, req, &flags)).await
}

fn created(out: &WriteOutput) -> i64 {
    out.ops[0].item.expect("a new event")
}

/// Every stored event, all calendars.
fn local_events(env: &Env) -> Vec<familiar_store::Event> {
    let r = env.reader();
    let cals: Vec<_> = r
        .calendars(env.account())
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    r.events_in(&cals, EventRange::utc(0, i64::MAX / 2))
        .unwrap()
}

fn local_named(env: &Env, subject: &str) -> Vec<familiar_store::Event> {
    local_events(env)
        .into_iter()
        .filter(|e| e.data.subject == subject)
        .collect()
}

fn mock_named(env: &Env, subject: &str) -> Vec<familiar_mock_graph::Event> {
    env.mock
        .mailbox(KEY)
        .events()
        .into_iter()
        .filter(|e| e.subject == subject && e.event_type != "seriesMaster")
        .collect()
}

async fn sync_now(env: &Env) {
    env.daemon()
        .service()
        .handle("sync.now", serde_json::json!({}))
        .await
        .unwrap();
}

fn base(subject: &str, start: String) -> CalAddInput {
    CalAddInput {
        subject: Some(subject.into()),
        start: Some(start),
        ..CalAddInput::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_added_changed_answered_and_deleted() {
    let mut env = Env::new(120).await;
    env.mock
        .mailbox(KEY)
        .add_calendar(familiar_mock_graph::Calendar {
            id: String::new(),
            name: "Family Room".into(),
            is_default: false,
            color: "auto".into(),
            owner: None,
        });
    // An invitation from someone else.
    let d3 = day(&env, 3);
    let mut invite =
        familiar_mock_graph::Event::new("Club AGM", utc(at(d3, 19, 0)), utc(at(d3, 20, 0)));
    invite.organizer = Some(EmailAddress::new("Secretary", "sec@club.example"));
    invite.attendees = vec![EmailAddress::new("Test User", ME)];
    let invite_id = env.mock.mailbox(KEY).add_event(invite).unwrap();
    // An email with the details of an event.
    let d5 = day(&env, 5);
    let mut mail = Message::new(
        "inbox",
        "FW: Club prizegiving",
        EmailAddress::new("Coach", "coach@club.example"),
        utc(now(&env) - 3600),
    );
    mail.body_html = Some(format!(
        "<p>Hi all,</p><p>The prizegiving is on {} at 6:30 pm.</p><p>Where: Parklands hall</p>",
        d5.format("%A %-d %B")
    ));
    let mail_remote = env.mock.mailbox(KEY).add_message(mail).unwrap();

    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    env.until("calendars and the invitation", LIMIT, |e| {
        !local_named(e, "Club AGM").is_empty()
            && e.reader()
                .calendars(e.account())
                .unwrap()
                .iter()
                .any(|c| c.name == "Family Room")
    })
    .await;
    let family = env
        .reader()
        .calendars(env.account())
        .unwrap()
        .into_iter()
        .find(|c| c.name == "Family Room")
        .unwrap();

    // Add to the family calendar by purpose, with an invitee: it shows at
    // once, then takes the server's ID.
    let d4 = day(&env, 4);
    let out = add(
        &env,
        CalAddInput {
            minutes: Some(90),
            purpose: Some("family".into()),
            location: Some("Hall".into()),
            attendee: vec!["Coach <coach@club.example>".into()],
            reminder: Some(30),
            ..base("Team dinner", format!("{} 18:00", d4.format("%Y-%m-%d")))
        },
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(out.ops[0].state, "done", "{out:?}");
    assert!(
        out.ops[0].summary.contains("to Family Room"),
        "{}",
        out.ops[0].summary
    );
    let dinner = env.reader().event(EventId(created(&out))).unwrap().unwrap();
    assert!(!dinner.data.remote_id.starts_with(LOCAL_REMOTE_PREFIX));
    assert_eq!(dinner.data.calendar, family.id);
    assert_eq!(dinner.data.start_at, at(d4, 18, 0));
    assert_eq!(dinner.data.end_at, at(d4, 19, 30));
    assert_eq!(dinner.data.start_tz.as_deref(), Some(NZ));
    let server = env.mock.mailbox(KEY).event(&dinner.data.remote_id).unwrap();
    assert_eq!(
        server.calendar_id,
        env.mock.mailbox(KEY).calendar_id("Family Room").unwrap()
    );
    assert_eq!(server.start.timestamp(), at(d4, 18, 0));
    assert_eq!(server.location, "Hall");
    assert_eq!(server.attendees[0].address, "coach@club.example");
    assert_eq!(server.reminder_minutes, Some(30));
    assert_eq!(
        server.time_zone.as_deref(),
        Some("New Zealand Standard Time")
    );

    // From the email: subject, date, time and place suggested; the email
    // is linked in the body.
    let msg = env.reader().message_by_remote(env.account(), &mail_remote);
    let msg = match msg {
        Ok(Some(m)) => m,
        _ => {
            sync_now(&env).await;
            let mut found = None;
            env.until("the email", LIMIT, |e| {
                found = e
                    .reader()
                    .message_by_remote(e.account(), &mail_remote)
                    .unwrap();
                found.as_ref().is_some_and(|m| m.body.is_some())
            })
            .await;
            found.unwrap()
        }
    };
    let out = add(
        &env,
        CalAddInput {
            from_mail: Some(msg.id.0),
            ..CalAddInput::default()
        },
        waiting(),
    )
    .await
    .unwrap();
    let prize = env.reader().event(EventId(created(&out))).unwrap().unwrap();
    assert_eq!(prize.data.subject, "Club prizegiving");
    assert_eq!(prize.data.start_at, at(d5, 18, 30));
    assert_eq!(prize.data.location, "Parklands hall");
    let server = env.mock.mailbox(KEY).event(&prize.data.remote_id).unwrap();
    let html = server.body_html.unwrap();
    assert!(
        html.contains("From the email") && html.contains("outlook.invalid/owa"),
        "{html}"
    );

    // Change it: a new start keeps the length.
    let p = env.paths.clone();
    let t = now(&env);
    let edit = CalEditInput {
        event: prize.id.0,
        subject: Some("Club prizegiving 2026".into()),
        start: Some(format!("{} 19:00", d5.format("%Y-%m-%d"))),
        ..CalEditInput::default()
    };
    let req = blocking(move || {
        edit_request(
            &familiar_store::Reader::open(&p.store).unwrap(),
            &edit,
            NZ,
            t,
        )
    })
    .await
    .unwrap();
    submit(&env, req, waiting()).await.unwrap();
    let server = env.mock.mailbox(KEY).event(&prize.data.remote_id).unwrap();
    assert_eq!(server.subject, "Club prizegiving 2026");
    assert_eq!(server.start.timestamp(), at(d5, 19, 0));
    assert_eq!(server.end.timestamp(), at(d5, 20, 0));
    let local = env.reader().event(prize.id).unwrap().unwrap();
    assert_eq!(local.data.start_at, at(d5, 19, 0));

    // Recurring: the server expands it and its occurrences replace the
    // placeholder.
    let d6 = day(&env, 6);
    let out = add(
        &env,
        CalAddInput {
            repeat: Some("weekly".into()),
            count: Some(4),
            ..base("Swim", format!("{} 17:00", d6.format("%Y-%m-%d")))
        },
        waiting(),
    )
    .await
    .unwrap();
    assert!(
        out.ops[0].summary.contains("every week, 4 times"),
        "{}",
        out.ops[0].summary
    );
    let swims = local_named(&env, "Swim");
    assert_eq!(swims.len(), 4, "{swims:?}");
    assert!(swims.iter().all(|s| s.data.series_master_id.is_some()));
    assert_eq!(
        swims[3].data.start_at,
        at(d6 + chrono::Duration::days(21), 17, 0)
    );
    assert_eq!(mock_named(&env, "Swim").len(), 4);

    // Change the whole series.
    let edit = CalEditInput {
        event: swims[1].id.0,
        location: Some("Pool".into()),
        series: true,
        ..CalEditInput::default()
    };
    let p = env.paths.clone();
    let req = blocking(move || {
        edit_request(
            &familiar_store::Reader::open(&p.store).unwrap(),
            &edit,
            NZ,
            t,
        )
    })
    .await
    .unwrap();
    submit(&env, req, waiting()).await.unwrap();
    assert!(
        local_named(&env, "Swim")
            .iter()
            .all(|s| s.data.location == "Pool")
    );
    assert!(
        mock_named(&env, "Swim")
            .iter()
            .all(|s| s.location == "Pool")
    );

    // Delete one occurrence: it leaves the calendar before the server has it.
    env.stop().await;
    let r = env.reader();
    let req = delete_request(
        &r,
        &CalDeleteInput {
            event: swims[0].id.0,
            ..CalDeleteInput::default()
        },
    )
    .unwrap();
    submit(&env, req, WriteFlags::default()).await.unwrap();
    assert_eq!(local_named(&env, "Swim").len(), 3);
    env.start(policy());
    env.until("one swim gone", LIMIT, |e| mock_named(e, "Swim").len() == 3)
        .await;
    // Then the series.
    let r = env.reader();
    let req = delete_request(
        &r,
        &CalDeleteInput {
            event: swims[1].id.0,
            series: true,
            ..CalDeleteInput::default()
        },
    )
    .unwrap();
    submit(&env, req, waiting()).await.unwrap();
    assert!(local_named(&env, "Swim").is_empty());
    assert!(mock_named(&env, "Swim").is_empty());

    // Answer the invitation; declining takes it off the calendar.
    let agm = local_named(&env, "Club AGM").remove(0);
    let rsvp = |response: &str, no_reply: bool| CalRsvpInput {
        event: agm.id.0,
        response: response.into(),
        comment: None,
        no_reply,
        series: false,
        dry_run: false,
        wait: false,
        wait_secs: None,
    };
    submit(
        &env,
        rsvp_request(&rsvp("tentative", true)).unwrap(),
        waiting(),
    )
    .await
    .unwrap();
    assert_eq!(
        env.mock
            .mailbox(KEY)
            .event(&invite_id)
            .unwrap()
            .response
            .as_deref(),
        Some("tentativelyAccepted")
    );
    let local = env.reader().event(agm.id).unwrap().unwrap();
    assert_eq!(local.data.response.as_deref(), Some("tentativelyAccepted"));
    let posted = env
        .mock
        .requests()
        .into_iter()
        .rev()
        .find(|r| r.path.ends_with("/tentativelyAccept"))
        .unwrap();
    assert!(
        posted.body.contains("\"sendResponse\":false"),
        "{}",
        posted.body
    );
    submit(
        &env,
        rsvp_request(&rsvp("decline", false)).unwrap(),
        waiting(),
    )
    .await
    .unwrap();
    assert!(env.mock.mailbox(KEY).event(&invite_id).is_none());
    assert!(local_named(&env, "Club AGM").is_empty());

    // Your own meeting: no invitation to answer, and cancelling needs --yes.
    let err = submit(
        &env,
        rsvp_request(&CalRsvpInput {
            event: dinner.id.0,
            ..rsvp("accept", false)
        })
        .unwrap(),
        waiting(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    let r = env.reader();
    let err = delete_request(
        &r,
        &CalDeleteInput {
            event: dinner.id.0,
            ..CalDeleteInput::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConfirmationRequired);
    let req = delete_request(
        &r,
        &CalDeleteInput {
            event: dinner.id.0,
            yes: true,
            comment: Some("Sorry, off".into()),
            ..CalDeleteInput::default()
        },
    )
    .unwrap();
    let out = submit(&env, req, waiting()).await.unwrap();
    assert!(
        out.ops[0].summary.starts_with("cancel"),
        "{}",
        out.ops[0].summary
    );
    assert!(
        env.mock
            .mailbox(KEY)
            .event(&dinner.data.remote_id)
            .is_none()
    );
    assert!(
        env.mock
            .requests()
            .iter()
            .any(|r| r.path.ends_with("/cancel"))
    );

    // After a full sync round the store matches the mock.
    sync_now(&env).await;
    env.until("store matches the mock", LIMIT, |e| {
        let mut local: Vec<String> = local_events(e)
            .into_iter()
            .map(|x| x.data.remote_id)
            .collect();
        let mut remote: Vec<String> = e
            .mock
            .mailbox(KEY)
            .events()
            .into_iter()
            .filter(|x| x.event_type != "seriesMaster")
            .map(|x| x.id)
            .collect();
        local.sort();
        remote.sort();
        local == remote
    })
    .await;
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_adds_wait_and_refusals_undo() {
    let mut env = Env::new(60).await;
    env.start(policy());
    env.until("first sync", LIMIT, |e| e.settled(30)).await;
    env.until("calendar", LIMIT, |e| {
        !e.reader().calendars(e.account()).unwrap().is_empty()
    })
    .await;
    env.stop().await;

    // Without famd: the event is on this device's calendar at once.
    let d2 = day(&env, 2);
    let out = add(
        &env,
        CalAddInput {
            all_day: true,
            days: Some(2),
            ..base("Camp", d2.format("%Y-%m-%d").to_string())
        },
        WriteFlags::default(),
    )
    .await
    .unwrap();
    let camp = env.reader().event(EventId(created(&out))).unwrap().unwrap();
    assert!(camp.data.remote_id.starts_with(LOCAL_REMOTE_PREFIX));
    assert!(camp.data.is_all_day);
    assert_eq!(camp.data.end_at - camp.data.start_at, 2 * 86_400);
    assert_eq!(local_named(&env, "Camp").len(), 1);
    env.start(policy());
    env.until("camp on the server", LIMIT, |e| {
        mock_named(e, "Camp").len() == 1
    })
    .await;
    env.until("camp has its server ID", LIMIT, |e| {
        e.reader()
            .event(camp.id)
            .unwrap()
            .is_some_and(|c| !c.data.remote_id.starts_with(LOCAL_REMOTE_PREFIX))
    })
    .await;
    let server = mock_named(&env, "Camp").remove(0);
    assert!(server.is_all_day);
    assert_eq!(server.start.timestamp(), camp.data.start_at);

    // The server refuses: the event is taken off again.
    env.mock.push_fault(
        Fault::access_denied()
            .on_method("POST")
            .on_path("events")
            .times(1),
    );
    let err = add(
        &env,
        base("Refused", format!("{} 10:00", d2.format("%Y-%m-%d"))),
        waiting(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Server, "{err:?}");
    assert!(local_named(&env, "Refused").is_empty());
    assert!(mock_named(&env, "Refused").is_empty());
    let failed = env
        .reader()
        .recent_ops(0, 50)
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == "create_event" && o.state == OpState::Failed)
        .count();
    assert_eq!(failed, 1);

    // Bad input is refused before anything changes.
    let err = add(
        &env,
        CalAddInput {
            end: Some(format!("{} 09:00", d2.format("%Y-%m-%d"))),
            ..base("Backwards", format!("{} 10:00", d2.format("%Y-%m-%d")))
        },
        WriteFlags::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    let err = add(
        &env,
        CalAddInput {
            purpose: Some("work".into()),
            ..base("Nowhere", "tomorrow 9am".into())
        },
        WriteFlags::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    env.stop().await;
}
