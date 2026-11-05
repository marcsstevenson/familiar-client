// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar read (WP 1.11) against the mock: the calendar list, full and
//! incremental calendarView delta rounds, a moving window and a 410.

mod common;

use chrono::{TimeZone, Utc};
use common::client;
use familiar_mock_graph::{Event, MockGraph};
use familiar_provider_graph::{CalendarApi, CalendarSync, Priority, events_resource};
use familiar_store::{AccountId, EventRange, NewAccount, Reader, Writer};

fn ts(y: i32, m: u32, d: u32) -> i64 {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap().timestamp()
}

struct Env {
    mock: MockGraph,
    dir: tempfile::TempDir,
    w: Writer,
    acc: AccountId,
    sync: CalendarSync,
}

async fn env() -> Env {
    let mock = MockGraph::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let acc = w
        .write(|tx| {
            tx.upsert_account(&NewAccount {
                key: "default".into(),
                email: "pat@example.com".into(),
                ..Default::default()
            })
        })
        .unwrap();
    let sync =
        CalendarSync::new(CalendarApi::new(client(&mock, "test-token")), acc).with_page_size(1);
    Env {
        mock,
        dir,
        w,
        acc,
        sync,
    }
}

impl Env {
    fn reader(&self) -> Reader {
        Reader::open(self.dir.path()).unwrap()
    }

    fn subjects(&self, remote_cal: &str) -> Vec<(String, String)> {
        let r = self.reader();
        let cal = r.calendar_by_remote(self.acc, remote_cal).unwrap().unwrap();
        r.events_in(&[cal.id], EventRange::utc(0, i64::MAX / 2))
            .unwrap()
            .into_iter()
            .map(|e| (e.data.remote_id, e.data.subject))
            .collect()
    }
}

#[tokio::test]
async fn calendars_then_events_by_delta() {
    let mut env = env().await;
    let report = env
        .sync
        .sync_calendars(&mut env.w, Priority::User)
        .await
        .unwrap();
    assert_eq!(report.upserted, 2);
    let cals = env.reader().calendars(env.acc).unwrap();
    assert_eq!(cals[0].name, "Calendar");
    assert!(cals[0].is_default);
    assert_eq!(cals[1].name, "Family Room");
    assert_eq!(cals[1].color.as_deref(), Some("lightGreen"));

    // Full round over November and December, one event per page.
    let window = (ts(2026, 11, 1), ts(2027, 1, 1));
    let r = env
        .sync
        .sync_events(&mut env.w, "cal-family", window, Priority::User)
        .await
        .unwrap();
    assert!(r.full_round);
    assert_eq!(r.upserted, 3);
    assert_eq!(r.pages, 3);
    assert_eq!(
        env.subjects("cal-family"),
        [
            ("evt-small-02".into(), "Junior practice".into()),
            ("evt-small-03".into(), "Junior practice".into()),
            ("evt-small-04".into(), "School holidays".into()),
        ]
    );
    let r = env.reader();
    let cal = r
        .calendar_by_remote(env.acc, "cal-family")
        .unwrap()
        .unwrap();
    assert_eq!(
        (cal.window_start, cal.window_end),
        (Some(window.0), Some(window.1))
    );
    let evs = r
        .events_in(&[cal.id], EventRange::utc(window.0, window.1))
        .unwrap();
    let practice = &evs[0].data;
    assert_eq!(
        practice.start_at,
        Utc.with_ymd_and_hms(2026, 11, 5, 4, 0, 0)
            .unwrap()
            .timestamp()
    );
    assert_eq!(practice.end_at - practice.start_at, 3600);
    assert_eq!(practice.kind, "occurrence");
    assert_eq!(
        practice.series_master_id.as_deref(),
        Some("evt-small-series")
    );
    assert_eq!(practice.location, "Club grounds");
    let holiday = &evs[2].data;
    assert!(holiday.is_all_day);
    assert_eq!(
        (holiday.start_at, holiday.end_at),
        (ts(2026, 12, 18), ts(2026, 12, 19))
    );
    assert_eq!(holiday.show_as, "free");
    let state = r
        .sync_state(env.acc, &events_resource("cal-family"))
        .unwrap()
        .unwrap();
    assert!(state.delta_link.unwrap().contains("deltatoken"));

    // Changes: one edited, one deleted, one added, one outside the window.
    let mbx = env.mock.mailbox("default");
    mbx.update_event("evt-small-02", |e| e.location = "Indoor nets".into())
        .unwrap();
    mbx.delete_event("evt-small-03").unwrap();
    let mut added = Event::new(
        "Parent evening",
        Utc.with_ymd_and_hms(2026, 11, 20, 6, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 20, 7, 0, 0).unwrap(),
    );
    added.calendar_id = "Family Room".into();
    let added = mbx.add_event(added).unwrap();
    let mut later = Event::new(
        "Next year",
        Utc.with_ymd_and_hms(2027, 2, 1, 6, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2027, 2, 1, 7, 0, 0).unwrap(),
    );
    later.calendar_id = "cal-family".into();
    mbx.add_event(later).unwrap();

    let r = env
        .sync
        .sync_events(&mut env.w, "cal-family", window, Priority::User)
        .await
        .unwrap();
    assert!(!r.full_round);
    assert_eq!((r.upserted, r.removed), (2, 1));
    let subjects = env.subjects("cal-family");
    assert_eq!(subjects.len(), 3);
    assert_eq!(subjects[1], (added, "Parent evening".to_string()));
    let rd = env.reader();
    let cal = rd
        .calendar_by_remote(env.acc, "cal-family")
        .unwrap()
        .unwrap();
    assert_eq!(
        rd.events_in(&[cal.id], EventRange::utc(window.0, window.1))
            .unwrap()[0]
            .data
            .location,
        "Indoor nets"
    );

    // The window moves on past 5 November: a full round drops what left it.
    let moved = (ts(2026, 11, 10), ts(2027, 1, 10));
    let r = env
        .sync
        .sync_events(&mut env.w, "cal-family", moved, Priority::User)
        .await
        .unwrap();
    assert!(r.full_round);
    let ids: Vec<String> = env
        .subjects("cal-family")
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert!(!ids.contains(&"evt-small-02".to_string()), "{ids:?}");
    assert_eq!(ids.len(), 2);
}

#[tokio::test]
async fn expired_delta_link_starts_over() {
    let mut env = env().await;
    env.sync
        .sync_calendars(&mut env.w, Priority::User)
        .await
        .unwrap();
    let window = (ts(2026, 11, 1), ts(2027, 1, 1));
    env.sync
        .sync_events(&mut env.w, "cal-default", window, Priority::User)
        .await
        .unwrap();
    assert_eq!(env.subjects("cal-default").len(), 1);

    env.mock
        .mailbox("default")
        .delete_event("evt-small-01")
        .unwrap();
    env.mock.mailbox("default").expire_delta_tokens();
    let r = env
        .sync
        .sync_events(&mut env.w, "cal-default", window, Priority::User)
        .await
        .unwrap();
    assert!(r.restarted && r.full_round);
    assert!(env.subjects("cal-default").is_empty());
}

#[tokio::test]
async fn removed_calendar_goes_with_its_events() {
    let mut env = env().await;
    env.sync
        .sync_calendars(&mut env.w, Priority::User)
        .await
        .unwrap();
    let extra = env
        .mock
        .mailbox("default")
        .add_calendar(familiar_mock_graph::Calendar {
            id: String::new(),
            name: "Birthdays".into(),
            is_default: false,
            color: "auto".into(),
            owner: None,
        });
    let r = env
        .sync
        .sync_calendars(&mut env.w, Priority::User)
        .await
        .unwrap();
    assert_eq!(r.upserted, 3);
    assert!(
        env.reader()
            .calendar_by_remote(env.acc, &extra)
            .unwrap()
            .is_some()
    );
}
