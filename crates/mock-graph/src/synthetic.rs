// SPDX-License-Identifier: MIT OR Apache-2.0
//! Deterministic synthetic mailboxes of any size.

use chrono::{DateTime, Duration, TimeZone, Utc};

use crate::model::{Calendar, EmailAddress, Event, Folder, Mailbox, Message, User};

/// How to generate a synthetic mailbox. The same spec always gives the same mailbox.
///
/// Bodies are not stored; they are generated from the message ID when served, so a
/// 500,000-message mailbox stays small in memory.
#[derive(Clone, Debug)]
pub struct SyntheticSpec {
    /// Random seed.
    pub seed: u64,
    /// Number of messages.
    pub messages: usize,
    /// Number of user folders, besides the well-known ones. Some are nested.
    pub folders: usize,
    /// Number of events on the default calendar, plus a weekly series on a second
    /// "Family" calendar.
    pub events: usize,
    /// The newest message's time. Older messages spread back over `years`.
    pub newest: DateTime<Utc>,
    /// How many years of history.
    pub years: u32,
}

impl Default for SyntheticSpec {
    fn default() -> Self {
        Self {
            seed: 1,
            messages: 200,
            folders: 6,
            events: 20,
            newest: Utc.with_ymd_and_hms(2026, 11, 4, 9, 0, 0).unwrap(),
            years: 3,
        }
    }
}

impl SyntheticSpec {
    /// A spec with `messages` messages and the other defaults.
    pub fn with_messages(messages: usize) -> Self {
        Self {
            messages,
            ..Self::default()
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const FIRST: &[&str] = &[
    "Alex", "Sam", "Jordan", "Taylor", "Morgan", "Casey", "Riley", "Jamie", "Avery", "Quinn",
    "Harper", "Rowan", "Emerson", "Parker", "Sasha",
];
const LAST: &[&str] = &[
    "Walker", "Ngata", "Smith", "Chen", "Patel", "Brown", "Wilson", "Tane", "Lee", "Martin",
];
const DOMAINS: &[&str] = &["example.com", "example.org", "example.net", "club.example"];
const TOPICS: &[&str] = &[
    "Team practice",
    "Invoice",
    "Quarterly report",
    "Weekend plans",
    "Fixture list",
    "Committee meeting",
    "Newsletter",
    "Your order",
    "Training schedule",
    "Budget review",
    "Photos",
    "Subscription renewal",
    "Volunteer roster",
    "Travel booking",
    "Agenda",
];

fn person(rng: &mut Rng) -> EmailAddress {
    let first = *rng.pick(FIRST);
    let last = *rng.pick(LAST);
    let domain = *rng.pick(DOMAINS);
    EmailAddress::new(
        format!("{first} {last}"),
        format!(
            "{}.{}@{domain}",
            first.to_ascii_lowercase(),
            last.to_ascii_lowercase()
        ),
    )
}

impl Mailbox {
    /// Generates a mailbox from `spec`.
    pub fn synthetic(spec: &SyntheticSpec) -> Self {
        let mut rng = Rng(spec.seed);
        let user = User::default();
        let me = EmailAddress::new(&user.display_name, &user.mail);

        let mut folders = Vec::new();
        for i in 0..spec.folders {
            let name = format!("{} {:02}", rng.pick(TOPICS), i + 1);
            let mut f = Folder::new(name);
            f.id = format!("fld-user-{:03}", i + 1);
            // Every third folder nests under the previous one; the rest sit under Inbox
            // or at the top level.
            f.parent_folder_id = match i % 3 {
                2 if i > 0 => Some(format!("fld-user-{i:03}")),
                1 => Some("inbox".into()),
                _ => None,
            };
            folders.push(f);
        }
        let user_folders: Vec<String> = folders.iter().map(|f| f.id.clone()).collect();

        let span = Duration::days(365 * i64::from(spec.years.max(1)));
        let step = span.num_seconds() / spec.messages.max(1) as i64;
        let mut messages = Vec::with_capacity(spec.messages);
        let mut subjects: Vec<String> = Vec::new();
        for i in 0..spec.messages {
            let received = spec.newest
                - Duration::seconds(step * i as i64 + rng.below(step.max(1) as u64) as i64);
            let roll = rng.below(100);
            let folder = match roll {
                0..=59 => "inbox".to_string(),
                60..=74 => "sentitems".to_string(),
                75..=77 => "deleteditems".to_string(),
                78..=79 => "junkemail".to_string(),
                80..=84 => "archive".to_string(),
                _ if !user_folders.is_empty() => rng.pick(&user_folders).clone(),
                _ => "inbox".to_string(),
            };
            // About a third of messages reply to a recent subject, so threads span folders.
            let subject = if !subjects.is_empty() && rng.below(3) == 0 {
                let base = rng
                    .pick(&subjects[subjects.len().saturating_sub(50)..])
                    .clone();
                format!("RE: {base}")
            } else {
                let s = format!("{} #{}", rng.pick(TOPICS), rng.below(10_000));
                subjects.push(s.clone());
                s
            };
            let (from, to) = if folder == "sentitems" {
                (me.clone(), vec![person(&mut rng)])
            } else {
                (person(&mut rng), vec![me.clone()])
            };
            let mut m = Message::new(folder, subject, from, received);
            m.id = format!("imm-{:07}", i + 1);
            m.to = to;
            m.is_read = i > 30 || rng.below(2) == 0;
            m.has_attachments = rng.below(10) == 0;
            m.flagged = rng.below(40) == 0;
            m.importance = if rng.below(30) == 0 { "high" } else { "normal" }.into();
            messages.push(m);
        }

        let calendars = vec![
            Calendar {
                id: "cal-default".into(),
                name: "Calendar".into(),
                is_default: true,
                color: "auto".into(),
                owner: None,
            },
            Calendar {
                id: "cal-family".into(),
                name: "Family".into(),
                is_default: false,
                color: "lightGreen".into(),
                owner: None,
            },
        ];
        let mut events = Vec::new();
        let around = spec.newest;
        for i in 0..spec.events {
            let day = rng.below(120) as i64 - 60;
            let hour = 8 + rng.below(9) as i64;
            let start = (around + Duration::days(day))
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                + Duration::hours(hour);
            let mut e = Event::new(
                format!("{} meeting", rng.pick(TOPICS)),
                start,
                start + Duration::minutes(30 * (1 + rng.below(4) as i64)),
            );
            e.id = format!("evt-{:05}", i + 1);
            e.calendar_id = "cal-default".into();
            e.location = format!("Room {}", 1 + rng.below(5));
            e.attendees = vec![person(&mut rng)];
            events.push(e);
        }
        // A weekly series, server-expanded into occurrences as Graph's calendarView does.
        let master = "evt-series-001".to_string();
        let first =
            around.date_naive().and_hms_opt(17, 0, 0).unwrap().and_utc() - Duration::weeks(4);
        for w in 0..12 {
            let start = first + Duration::weeks(w);
            let mut e = Event::new("Junior cricket practice", start, start + Duration::hours(1));
            e.id = format!("{master}-occ-{w:02}");
            e.calendar_id = "cal-family".into();
            e.location = "Club grounds".into();
            e.event_type = "occurrence".into();
            e.series_master_id = Some(master.clone());
            events.push(e);
        }

        Mailbox {
            user,
            folders,
            messages,
            calendars,
            events,
        }
    }
}
