// SPDX-License-Identifier: MIT OR Apache-2.0
//! Time zones: IANA names inside Familiar, Windows names at the Graph boundary.
//!
//! Familiar stores and shows IANA zones (`Pacific/Auckland`). Graph and
//! Outlook often use Windows zone names (`New Zealand Standard Time`), and
//! one past session found the consumer service handled the Windows name more
//! reliably, so writes send Windows names and reads accept either.
//!
//! The mapping is CLDR's `windowsZones.xml`, generated into `tz_table.rs`
//! by `scripts/gen-windows-zones.py`. A Windows zone maps to its primary
//! IANA zone (territory `001`); an IANA zone maps to the Windows zone CLDR
//! lists it under.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::tz_table::{IANA_TO_WINDOWS, WINDOWS_TO_IANA};

/// Current IANA names that CLDR still lists under their older name.
const IANA_ALIASES: &[(&str, &str)] = &[
    ("America/Argentina/Buenos_Aires", "America/Buenos_Aires"),
    ("America/Argentina/Catamarca", "America/Catamarca"),
    ("America/Argentina/Cordoba", "America/Cordoba"),
    ("America/Argentina/Jujuy", "America/Jujuy"),
    ("America/Argentina/Mendoza", "America/Mendoza"),
    ("America/Atikokan", "America/Coral_Harbour"),
    ("America/Indiana/Indianapolis", "America/Indianapolis"),
    ("America/Kentucky/Louisville", "America/Louisville"),
    ("America/Nuuk", "America/Godthab"),
    ("Asia/Ho_Chi_Minh", "Asia/Saigon"),
    ("Asia/Kathmandu", "Asia/Katmandu"),
    ("Asia/Kolkata", "Asia/Calcutta"),
    ("Asia/Yangon", "Asia/Rangoon"),
    ("Atlantic/Faroe", "Atlantic/Faeroe"),
    ("Europe/Kyiv", "Europe/Kiev"),
    ("Pacific/Chuuk", "Pacific/Truk"),
    ("Pacific/Kanton", "Pacific/Enderbury"),
    ("Pacific/Pohnpei", "Pacific/Ponape"),
];

/// The primary IANA zone for a Windows zone name (`New Zealand Standard
/// Time` → `Pacific/Auckland`). Case-insensitive.
pub fn windows_to_iana(windows: &str) -> Option<&'static str> {
    let w = windows.trim();
    WINDOWS_TO_IANA
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(w))
        .map(|(_, v)| *v)
}

/// The Windows zone name for an IANA zone (`Pacific/Auckland` → `New
/// Zealand Standard Time`). `UTC` and `Etc/UTC` give `UTC`.
pub fn iana_to_windows(iana: &str) -> Option<&'static str> {
    let i = iana.trim();
    if i.eq_ignore_ascii_case("UTC") {
        return Some("UTC");
    }
    let look = |name: &str| {
        IANA_TO_WINDOWS
            .binary_search_by(|(k, _)| (*k).cmp(name))
            .ok()
            .map(|n| IANA_TO_WINDOWS[n].1)
    };
    let exact = parse_iana(i).map(|tz| tz.name()).unwrap_or(i);
    look(exact).or_else(|| {
        IANA_ALIASES
            .iter()
            .find(|(new, _)| *new == exact)
            .and_then(|(_, old)| look(old))
    })
}

fn parse_iana(name: &str) -> Option<Tz> {
    name.parse::<Tz>()
        .ok()
        .or_else(|| Tz::from_str_insensitive(name).ok())
}

/// A zone from either an IANA or a Windows name. `None` if neither knows it.
pub fn parse_zone(name: &str) -> Option<Tz> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    if n.eq_ignore_ascii_case("UTC") || n.eq_ignore_ascii_case("tzone://Microsoft/Utc") {
        return Some(Tz::UTC);
    }
    parse_iana(n).or_else(|| windows_to_iana(n).and_then(parse_iana))
}

/// The IANA name for an IANA or Windows zone name, as Familiar stores it.
pub fn to_iana(name: &str) -> Option<String> {
    parse_zone(name).map(|tz| tz.name().to_string())
}

/// The machine's own zone, or UTC if it can't be found.
pub fn local_zone() -> Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|n| parse_zone(&n))
        .unwrap_or(Tz::UTC)
}

/// The instant a wall-clock time in `tz` stands for. A time skipped by a
/// clock change moves forward to the first valid time after it; a time
/// that happens twice takes the earlier.
pub fn local_to_utc(local: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    for minutes in (0..=180).step_by(15) {
        let t = local + chrono::Duration::minutes(minutes);
        if let Some(d) = tz.from_local_datetime(&t).earliest() {
            return d.with_timezone(&Utc);
        }
    }
    Utc.from_utc_datetime(&local)
}

/// Parses Graph's `dateTimeTimeZone` (`2026-11-05T04:00:00.0000000` with a
/// Windows or IANA zone name) into an instant. An unknown zone is read as UTC.
pub fn graph_date_time(date_time: &str, zone: &str) -> Option<DateTime<Utc>> {
    let s = date_time.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()?;
    let tz = parse_zone(zone).unwrap_or(Tz::UTC);
    Some(local_to_utc(naive, tz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auckland_both_ways() {
        assert_eq!(
            windows_to_iana("New Zealand Standard Time"),
            Some("Pacific/Auckland")
        );
        assert_eq!(
            windows_to_iana("new zealand standard time"),
            Some("Pacific/Auckland")
        );
        assert_eq!(
            iana_to_windows("Pacific/Auckland"),
            Some("New Zealand Standard Time")
        );
        assert_eq!(
            iana_to_windows("Antarctica/McMurdo"),
            Some("New Zealand Standard Time")
        );
    }

    #[test]
    fn utc_and_common_zones() {
        assert_eq!(iana_to_windows("UTC"), Some("UTC"));
        assert_eq!(iana_to_windows("Etc/UTC"), Some("UTC"));
        assert_eq!(windows_to_iana("UTC"), Some("Etc/UTC"));
        assert_eq!(iana_to_windows("Europe/London"), Some("GMT Standard Time"));
        assert_eq!(
            iana_to_windows("America/New_York"),
            Some("Eastern Standard Time")
        );
        assert_eq!(
            windows_to_iana("Pacific Standard Time"),
            Some("America/Los_Angeles")
        );
        assert_eq!(
            iana_to_windows("Australia/Sydney"),
            Some("AUS Eastern Standard Time")
        );
    }

    #[test]
    fn current_names_find_cldr_old_names() {
        assert_eq!(iana_to_windows("Asia/Kolkata"), Some("India Standard Time"));
        assert_eq!(iana_to_windows("Europe/Kyiv"), Some("FLE Standard Time"));
        assert_eq!(
            iana_to_windows("asia/kathmandu"),
            Some("Nepal Standard Time")
        );
    }

    #[test]
    fn every_windows_zone_round_trips_and_parses() {
        for (win, iana) in WINDOWS_TO_IANA {
            assert!(parse_iana(iana).is_some(), "{iana} is not in chrono-tz");
            assert_eq!(iana_to_windows(iana), Some(*win), "{iana}");
            assert!(parse_zone(win).is_some(), "{win}");
        }
        assert!(IANA_TO_WINDOWS.windows(2).all(|p| p[0].0 < p[1].0));
    }

    #[test]
    fn parse_zone_takes_either_name() {
        assert_eq!(parse_zone("Pacific/Auckland"), Some(Tz::Pacific__Auckland));
        assert_eq!(
            parse_zone("New Zealand Standard Time"),
            Some(Tz::Pacific__Auckland)
        );
        assert_eq!(parse_zone("UTC"), Some(Tz::UTC));
        assert_eq!(parse_zone("Mars/Olympus"), None);
        assert_eq!(
            to_iana("W. Europe Standard Time").as_deref(),
            Some("Europe/Berlin")
        );
    }

    #[test]
    fn graph_times_in_windows_zones() {
        // 9am in Auckland on 5 Nov 2026 (NZDT, +13) is 20:00 UTC the day before.
        let t =
            graph_date_time("2026-11-05T09:00:00.0000000", "New Zealand Standard Time").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-11-04T20:00:00+00:00");
        let u = graph_date_time("2026-11-05T04:00:00.0000000", "UTC").unwrap();
        assert_eq!(u.to_rfc3339(), "2026-11-05T04:00:00+00:00");
        // A time inside the spring-forward gap (2:30am, 27 Sep 2026) moves to 3:00 NZDT.
        let gap = graph_date_time("2026-09-27T02:30:00", "Pacific/Auckland").unwrap();
        assert_eq!(gap.to_rfc3339(), "2026-09-26T14:00:00+00:00");
    }
}
