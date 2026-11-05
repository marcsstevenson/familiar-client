// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars, events, calendarView and calendarView delta.

mod common;

use chrono::{TimeZone, Utc};
use common::{Client, ids, present, removed};
use familiar_mock_graph::{Event, MockGraph};

const WINDOW: &str = "startDateTime=2026-11-01T00:00:00Z&endDateTime=2026-12-01T00:00:00Z";

#[tokio::test]
async fn calendars_and_events() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");

    let cals = c.get_all("/me/calendars").await;
    assert_eq!(ids(&cals), ["cal-default", "cal-family"]);
    assert_eq!(cals[0]["isDefaultCalendar"], true);
    assert_eq!(c.get("/me/calendar").await.body["id"], "cal-default");

    let family = c.get_all("/me/calendars/cal-family/events").await;
    assert_eq!(family.len(), 4, "three events and the series master");
    assert_eq!(family[0]["type"], "occurrence");
    assert_eq!(family[0]["seriesMasterId"], "evt-small-series");
    assert_eq!(family[0]["start"]["timeZone"], "UTC");
    assert_eq!(
        family[0]["start"]["dateTime"],
        "2026-11-05T04:00:00.0000000"
    );

    let default_events = c.get_all("/me/events").await;
    assert_eq!(ids(&default_events), ["evt-small-01"]);
    assert_eq!(
        default_events[0]["organizer"]["emailAddress"]["address"],
        "casey.chen@club.example"
    );

    let ev = c.get("/me/calendars/cal-family/events/evt-small-04").await;
    assert_eq!(ev.body["isAllDay"], true);
    assert_eq!(
        c.get("/me/calendars/cal-default/events/evt-small-04")
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn calendar_view_needs_a_window() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token");
    let r = c.get("/me/calendars/cal-family/calendarView").await;
    assert_eq!(r.status, 400);
    assert_eq!(r.error_code(), "ErrorInvalidParameter");

    let nov = c
        .get_all(&format!("/me/calendars/cal-family/calendarView?{WINDOW}"))
        .await;
    assert_eq!(
        ids(&nov),
        ["evt-small-02", "evt-small-03"],
        "December holiday is outside"
    );
}

#[tokio::test]
async fn calendar_view_delta_rounds() {
    let mock = MockGraph::start().await.unwrap();
    let c = Client::new(mock.base_url(), "test-token").prefer("odata.maxpagesize=1");
    let mbx = mock.mailbox("default");

    let start = format!("/me/calendars/cal-family/calendarView/delta?{WINDOW}");
    let (items, pages, link) = c.delta_round(&start).await;
    assert_eq!(ids(&items), ["evt-small-02", "evt-small-03"]);
    assert_eq!(pages, 2);
    assert!(
        !link.contains("startDateTime"),
        "the window travels in the token"
    );

    // Moving one occurrence's place makes it an exception, which delta gives in full.
    mbx.update_event("evt-small-02", |e| {
        e.location = "Indoor nets".into();
        e.event_type = "exception".into();
    })
    .unwrap();
    mbx.delete_event("evt-small-03").unwrap();
    let mut added = Event::new(
        "Parent evening",
        Utc.with_ymd_and_hms(2026, 11, 20, 6, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 20, 7, 0, 0).unwrap(),
    );
    added.calendar_id = "cal-family".into();
    let added = mbx.add_event(added).unwrap();
    // Outside the window: not reported.
    let mut later = Event::new(
        "Next year",
        Utc.with_ymd_and_hms(2027, 2, 1, 6, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2027, 2, 1, 7, 0, 0).unwrap(),
    );
    later.calendar_id = "Family Room".into();
    mbx.add_event(later).unwrap();

    let (items, _, _) = c.delta_round(&link).await;
    let p = present(&items);
    assert_eq!(p, ["evt-small-02".to_string(), added]);
    assert_eq!(removed(&items), ["evt-small-03"]);
    assert_eq!(items[0]["location"]["displayName"], "Indoor nets");

    // The default calendar's delta at /me/calendarView/delta.
    let (items, _, _) = c
        .delta_round(&format!("/me/calendarView/delta?{WINDOW}"))
        .await;
    assert_eq!(ids(&items), ["evt-small-01"]);
}
