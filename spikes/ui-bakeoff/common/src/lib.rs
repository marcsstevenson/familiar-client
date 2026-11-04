//! Shared pieces for the WP 0.4 UI bake-off: deterministic synthetic rows,
//! a synthetic HTML newsletter, the sanitiser both prototypes use, and a
//! small benchmark recorder that writes JSON results.
//!
//! Nothing here is real mail. Every name and address is generated.

use std::collections::HashSet;
use std::io::Write as _;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const ROW_COUNT: usize = 500_000;
pub const SEED: u64 = 0x0F4A_11A2;

pub const FOLDERS: &[&str] = &[
    "Inbox",
    "Drafts",
    "Sent Items",
    "Archive",
    "Deleted Items",
    "Junk Email",
    "Receipts",
    "Travel",
    "Family",
    "Cricket",
    "Cricket/Fixtures",
    "Cricket/Committee",
    "Dental",
    "Dental/Suppliers",
    "Dental/Staff",
    "Newsletters",
    "Banking",
    "Insurance",
    "House",
    "House/Builders",
    "Car",
    "School",
    "Health",
    "Projects",
    "Projects/Familiar",
    "Reading",
    "Old/2006",
    "Old/2007",
    "Old/2008",
    "Old/2009",
    "Old/2010",
    "Old/2011",
    "Old/2012",
    "Old/2013",
    "Old/2014",
    "Old/2015",
    "Old/2016",
    "Old/2017",
    "Old/2018",
    "Old/2019",
    "Old/2020",
    "Old/2021",
    "Old/2022",
    "Old/2023",
    "Old/2024",
    "Old/2025",
];

#[derive(Clone, Debug, serde::Serialize)]
pub struct Row {
    pub from: String,
    pub subject: String,
    pub preview: String,
    pub date: String,
    pub unread: bool,
    pub flagged: bool,
    pub attachment: bool,
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

const FIRST: &[&str] = &[
    "Aroha", "Ben", "Chloe", "Dev", "Ema", "Finn", "Grace", "Hemi", "Isla", "Jack", "Kiri", "Liam",
    "Mia", "Noah", "Olivia", "Priya", "Quinn", "Ravi", "Sofia", "Tane",
];
const LAST: &[&str] = &[
    "Walker", "Ngata", "Chen", "Patel", "Smith", "Brown", "Wilson", "Taylor", "Singh", "Murphy",
    "Kaur", "Tuhoe", "Reid", "Young", "King", "Wright", "Scott", "Green",
];
const SUBJ_A: &[&str] = &[
    "Re: ",
    "Fwd: ",
    "",
    "",
    "",
    "Re: Re: ",
    "",
    "[Committee] ",
    "",
    "Invoice: ",
];
const SUBJ_B: &[&str] = &[
    "Saturday fixture changes",
    "Quarterly supplier review",
    "Your order has shipped",
    "Minutes from Tuesday",
    "Roster for next week",
    "Renewal notice",
    "Photos from the weekend",
    "Draft agreement attached",
    "Can we move our call?",
    "Monthly newsletter",
    "Lab results ready",
    "Ground booking confirmed",
    "School trip permission",
    "Builder quote v2",
    "Flight itinerary",
    "Statement available",
];
const PREVIEW: &[&str] = &[
    "Thanks for getting back to me so quickly. I have had a look at the numbers and",
    "Just a reminder that the meeting has moved to the clubrooms because the hall is",
    "Please find attached the updated version with the changes we discussed on the",
    "Hi all, a quick update on where things stand before the weekend so nobody is",
    "Your statement for the period is now available to view online. Sign in to see",
];

/// Row `i` of the synthetic mailbox, newest first. Cheap and deterministic,
/// so both prototypes can produce any row on demand without a store.
pub fn row(i: usize) -> Row {
    let h = splitmix(SEED ^ i as u64);
    let pick = |n: usize, shift: u32| ((h >> shift) as usize) % n;
    let first = FIRST[pick(FIRST.len(), 0)];
    let last = LAST[pick(LAST.len(), 8)];
    // Newest first: roughly 20 years over 500k rows.
    let minutes_ago = i as u64 * 21 + (h >> 40) % 20;
    let days = minutes_ago / 1440;
    let (y, m, d) = civil_from_days(20_760 - days as i64); // 2026-11-03 is day 20,760
    Row {
        from: format!("{first} {last}"),
        subject: format!(
            "{}{} #{}",
            SUBJ_A[pick(SUBJ_A.len(), 16)],
            SUBJ_B[pick(SUBJ_B.len(), 24)],
            i
        ),
        preview: PREVIEW[pick(PREVIEW.len(), 32)].to_string(),
        date: format!("{y:04}-{m:02}-{d:02}"),
        unread: (h >> 48) % 7 == 0,
        flagged: (h >> 52) % 23 == 0,
        attachment: (h >> 56) % 9 == 0,
    }
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A raw, unsanitised newsletter of roughly 60 KB in the style of real
/// marketing mail: nested layout tables, a `<style>` block, inline styles,
/// remote images and tracking pixels, plus hostile bits the sanitiser must
/// remove (script, event handlers, javascript: links, a form, an iframe).
pub fn newsletter_html(i: usize) -> String {
    let r = row(i);
    let mut s = String::with_capacity(70_000);
    s.push_str(r#"<!doctype html><html><head><meta charset="utf-8"><title>Newsletter</title>
<style>
body{margin:0;padding:0;background:#f2f4f7;font-family:Helvetica,Arial,sans-serif;color:#1d2330}
.wrap{width:100%;background:#f2f4f7}.card{background:#ffffff;border-radius:6px}
h1{font-size:28px;line-height:34px;margin:0 0 12px}h2{font-size:20px;margin:0 0 8px;color:#0b5cad}
p{font-size:15px;line-height:23px;margin:0 0 14px}.btn{display:inline-block;padding:12px 22px;background:#0b5cad;color:#fff;text-decoration:none;border-radius:4px}
.muted{color:#6b7385;font-size:12px}.col{width:50%;vertical-align:top;padding:12px}
@media (max-width:600px){.col{display:block;width:100%}}
</style>
<script>document.cookie='x=1';fetch('https://tracker.example.invalid/open')</script>
</head><body onload="alert(1)">
<img src="https://tracker.example.invalid/pixel.gif?u=123" width="1" height="1" alt="">
<table class="wrap" role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td align="center" style="padding:24px 12px">
<table class="card" role="presentation" width="640" cellpadding="0" cellspacing="0" style="max-width:640px">
<tr><td style="padding:0"><img src="https://cdn.example.invalid/hero.jpg" width="640" height="240" alt="Hero banner" style="display:block;width:100%;height:auto"></td></tr>
<tr><td style="padding:28px 32px 8px">"#);
    s.push_str(&format!(
        "<h1>{}</h1><p class=\"muted\">From {} · {}</p>",
        esc(&r.subject),
        esc(&r.from),
        r.date
    ));
    s.push_str("</td></tr>");
    let mut k = 0usize;
    while s.len() < 60_000 {
        let rr = row(i.wrapping_mul(31).wrapping_add(k));
        s.push_str(&format!(r#"<tr><td style="padding:8px 20px"><table role="presentation" width="100%"><tr>
<td class="col"><img src="https://cdn.example.invalid/item{k}.png" width="280" height="160" alt="Item {k}" style="display:block;width:100%;border-radius:4px;background:#dde3ec">
<h2>{subj}</h2><p>{prev} {prev} Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.</p>
<a class="btn" href="https://shop.example.invalid/item/{k}?utm_source=mail" onclick="steal()">Read more</a></td>
<td class="col"><h2>{from} recommends</h2><p>{prev} Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.</p>
<ul><li><b>Date:</b> {date}</li><li><i>Ref:</i> <a href="javascript:alert(2)">#{k}</a></li><li style="color:#0b5cad">Attachment: {att}</li></ul></td>
</tr></table></td></tr>"#,
            k = k, subj = esc(&rr.subject), prev = esc(&rr.preview), from = esc(&rr.from), date = rr.date,
            att = if rr.attachment { "yes" } else { "no" }));
        k += 1;
    }
    s.push_str(r#"<tr><td style="padding:24px 32px"><form action="https://phish.example.invalid"><input name="password"></form>
<iframe src="https://evil.example.invalid"></iframe>
<p class="muted">You are receiving this because you subscribed. <a href="https://example.invalid/unsubscribe">Unsubscribe</a></p></td></tr>
</table></td></tr></table></body></html>"#);
    s
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The Content-Security-Policy placed inside every rendered message frame.
/// Nothing can load from the network; only inline styles and data/cid images.
pub const MESSAGE_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src data: cid:; font-src data:";

/// Sanitise raw email HTML. Scripts, handlers, forms, frames and
/// `javascript:` links go; remote images are replaced with a blocked
/// placeholder (the real app adds per-message and per-sender overrides).
/// Returns a complete document with the CSP meta tag, ready for a sandboxed
/// frame's `srcdoc` or a webview's `load_html`.
pub fn sanitize(raw: &str) -> String {
    let tags: HashSet<&str> = ["style", "center", "font"].into_iter().collect();
    let generic: HashSet<&str> = [
        "style",
        "class",
        "align",
        "valign",
        "width",
        "height",
        "bgcolor",
        "color",
        "border",
        "cellpadding",
        "cellspacing",
        "role",
        "dir",
        "lang",
        "title",
    ]
    .into_iter()
    .collect();
    let mut clean_content: HashSet<&str> = ["script"].into_iter().collect();
    clean_content.insert("iframe");
    clean_content.insert("title");
    let body = ammonia::Builder::default()
        .add_tags(tags)
        .rm_clean_content_tags(["style"])
        .clean_content_tags(clean_content)
        .add_generic_attributes(generic)
        .url_schemes(
            ["http", "https", "mailto", "cid", "data"]
                .into_iter()
                .collect(),
        )
        .attribute_filter(|el, attr, val| {
            if el == "img"
                && attr == "src"
                && (val.starts_with("http:") || val.starts_with("https:"))
            {
                // Blocked remote image: keep layout, drop the request.
                return Some(BLOCKED_IMG.into());
            }
            Some(val.into())
        })
        .link_rel(Some("noopener noreferrer"))
        .clean(raw)
        .to_string();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{MESSAGE_CSP}\"></head><body>{body}</body></html>"
    )
}

const BLOCKED_IMG: &str = "data:image/svg+xml;utf8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10'%3E%3Crect width='10' height='10' fill='%23dde3ec'/%3E%3C/svg%3E";

// ---------------------------------------------------------------------------
// Benchmark recorder
// ---------------------------------------------------------------------------

pub fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Bench mode is on when `FAMILIAR_BENCH_OUT` names a file. Each event is one
/// JSON line: {"t_ns":..., "event":"...", ...}. The harness turns lines into
/// the results table.
pub struct Bench {
    out: Option<Mutex<std::fs::File>>,
}

impl Bench {
    pub fn from_env() -> Self {
        let out = std::env::var("FAMILIAR_BENCH_OUT").ok().map(|p| {
            Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .expect("bench out"),
            )
        });
        Bench { out }
    }
    pub fn enabled(&self) -> bool {
        self.out.is_some()
    }
    pub fn event(&self, event: &str, extra: serde_json::Value) {
        if let Some(f) = &self.out {
            let mut v = serde_json::json!({ "t_ns": now_ns().to_string(), "event": event });
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                for (k, x) in e {
                    o.insert(k.clone(), x.clone());
                }
            }
            let mut f = f.lock().unwrap();
            let _ = writeln!(f, "{v}");
            let _ = f.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_deterministic() {
        assert_eq!(row(42).subject, row(42).subject);
        assert_ne!(row(1).subject, row(2).subject);
        assert_eq!(row(0).date, "2026-11-03");
    }

    #[test]
    fn sanitiser_removes_hostile_content() {
        let raw = newsletter_html(7);
        assert!(raw.len() > 55_000);
        let clean = sanitize(&raw);
        for bad in [
            "<script",
            "onload",
            "onclick",
            "javascript:",
            "<form",
            "<iframe",
            "tracker.example",
            "cdn.example",
        ] {
            assert!(!clean.contains(bad), "{bad} survived");
        }
        assert!(clean.contains("<table") && clean.contains("<style"));
        assert!(clean.contains("Content-Security-Policy"));
    }
}
