// SPDX-License-Identifier: MIT OR Apache-2.0
//! The facts behind a spam and scam check (AI spam plan, WP S.1): what the
//! receiving mail server recorded about a message, and what the message's
//! sender, links and attachments look like. All local and deterministic;
//! the AI grade in [`crate::commands::check_spam`] is given these facts.
//!
//! Nothing here fetches a link or looks anything up on the network.

use serde::{Deserialize, Serialize};

use schemars::JsonSchema;

/// One internet header, as Graph's `internetMessageHeaders` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Header {
    /// Name.
    pub name: String,
    /// Value.
    pub value: String,
}

/// One thing known about the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Fact {
    /// What it is: `dmarc`, `scl`, `reply_to`, `link_mismatch`, ...
    pub name: String,
    /// Its value, short: `fail`, `6`, `gmail.com`, ...
    pub value: String,
    /// Which way it points: `spam`, `scam`, `safe`, or empty for neither.
    pub points: String,
    /// The fact in words.
    pub detail: String,
}

impl Fact {
    fn new(name: &str, value: impl Into<String>, points: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            points: points.into(),
            detail: detail.into(),
        }
    }
}

/// The first header called `name` (topmost: the last one added).
pub fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// `key=value` from an `Authentication-Results` value, such as `dmarc`.
fn auth_result(v: &str, key: &str) -> Option<String> {
    v.split(';').find_map(|part| {
        let part = part.trim();
        let (k, rest) = part.split_once('=')?;
        // The first clause may start with the authserv-id: "mx.microsoft.com 1; spf=...".
        if !k.trim().eq_ignore_ascii_case(key) {
            return None;
        }
        rest.split_whitespace()
            .next()
            .map(|s| s.trim_end_matches(';').to_ascii_lowercase())
    })
}

/// `KEY:value` from a `;`-separated Microsoft header, such as `SCL:6`.
fn ms_field(v: &str, key: &str) -> Option<String> {
    v.split(';').find_map(|part| {
        let (k, val) = part.trim().split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(key)
            .then(|| val.trim().to_string())
    })
}

/// The facts the receiving mail server recorded: sender authentication
/// from the topmost `Authentication-Results` (the one it added; lower
/// copies and the sender's `-Original`/`-Untrusted` ones don't count), and
/// Microsoft's spam verdict.
pub fn server_facts(headers: &[Header]) -> Vec<Fact> {
    let mut out = Vec::new();
    if let Some(ar) = header(headers, "Authentication-Results") {
        for (key, what) in [("spf", "SPF"), ("dkim", "DKIM"), ("dmarc", "DMARC")] {
            let Some(v) = auth_result(ar, key) else {
                continue;
            };
            let points = match v.as_str() {
                "fail" | "permerror" => "scam",
                "softfail" if key == "spf" => "scam",
                _ => "",
            };
            let detail = match v.as_str() {
                "pass" => format!("{what} passed"),
                "none" => format!("No {what} for the sender's domain"),
                other => format!("{what}: {other}"),
            };
            out.push(Fact::new(key, v, points, detail));
        }
        if let Some(v) = auth_result(ar, "compauth") {
            let points = if v == "fail" { "scam" } else { "" };
            let detail = if v == "fail" {
                "Microsoft thinks the sender is spoofed (compauth fail)".to_string()
            } else {
                format!("Microsoft's sender check: {v}")
            };
            out.push(Fact::new("compauth", v, points, detail));
        }
    }
    if let Some(scl) =
        header(headers, "X-MS-Exchange-Organization-SCL").and_then(|v| v.trim().parse::<i32>().ok())
    {
        let points = if scl >= 5 { "spam" } else { "" };
        let words = match scl {
            -1 => "skipped spam filtering",
            0 | 1 => "not spam",
            5 | 6 => "spam",
            9 => "high-confidence spam",
            _ => "unsure",
        };
        out.push(Fact::new(
            "scl",
            scl.to_string(),
            points,
            format!("Outlook's spam level {scl} ({words})"),
        ));
    }
    if let Some(bcl) = header(headers, "X-Microsoft-Antispam")
        .and_then(|v| ms_field(v, "BCL"))
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|b| *b > 0)
    {
        let points = if bcl >= 7 { "spam" } else { "" };
        out.push(Fact::new(
            "bcl",
            bcl.to_string(),
            points,
            format!("Bulk mail, complaint level {bcl} of 9"),
        ));
    }
    if let Some(report) = header(headers, "X-Forefront-Antispam-Report") {
        if let Some(cat) = ms_field(report, "CAT").filter(|c| c != "NONE" && !c.is_empty()) {
            let (points, words) = match cat.as_str() {
                "PHSH" | "HPHSH" | "HPHISH" => ("scam", "phishing"),
                "SPOOF" | "GIMP" | "UIMP" | "DIMP" | "OSPM" => {
                    ("scam", "impersonation or spoofing")
                }
                "MALW" => ("scam", "malware"),
                "BULK" => ("spam", "bulk mail"),
                _ => ("spam", "spam"),
            };
            out.push(Fact::new(
                "category",
                cat.clone(),
                points,
                format!("Microsoft filed it as {words} ({cat})"),
            ));
        }
    }
    if let Some(dest) =
        header(headers, "X-Microsoft-Antispam-Mailbox-Delivery").and_then(|v| ms_field(v, "dest"))
    {
        let (points, words) = match dest.as_str() {
            "J" => ("spam", "Junk Email"),
            "I" => ("", "the Inbox"),
            _ => ("", "another folder"),
        };
        out.push(Fact::new(
            "delivered_to",
            dest,
            points,
            format!("Outlook delivered it to {words}"),
        ));
    }
    out
}

/// The address in a header value like `"Name" <a@b.c>` or `a@b.c`.
pub fn address_in(v: &str) -> Option<String> {
    let inner = match (v.rfind('<'), v.rfind('>')) {
        (Some(s), Some(e)) if s < e => &v[s + 1..e],
        _ => v,
    };
    let a = inner.trim().trim_matches(|c| c == '"' || c == '\'');
    (a.contains('@') && !a.contains(char::is_whitespace)).then(|| a.to_ascii_lowercase())
}

/// The domain of an address, lower case.
pub fn domain_of(addr: &str) -> &str {
    addr.rsplit_once('@').map_or("", |(_, d)| d)
}

/// Second-level labels under a two-letter country code that people
/// register below (`co.nz`, `com.au`). An approximation of the public
/// suffix list that covers the domains this check meets.
const SECOND_LEVEL: &[&str] = &[
    "co", "com", "net", "org", "gov", "govt", "ac", "edu", "school", "geek", "gen", "kiwi",
    "maori", "iwi", "mil", "ltd", "plc", "nhs", "ne", "or",
];

/// The registered part of a host: `mail.anz.co.nz` → `anz.co.nz`.
pub fn registered(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if is_ip(&host) {
        return host;
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    let n = labels.len();
    let keep = if n >= 3 && labels[n - 1].len() == 2 && SECOND_LEVEL.contains(&labels[n - 2]) {
        3
    } else {
        2
    };
    labels[n.saturating_sub(keep)..].join(".")
}

/// A domain-looking word in `text` (`anz.co.nz`, `www.paypal.com/login`),
/// as a host.
fn domain_word(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || "()<>[]\"',;".contains(c))
        .find_map(|w| {
            let w = w
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches(['.', ':', '!', '?']);
            let host = w.split(['/', '?', '#']).next().unwrap_or("");
            if host.contains('@') {
                return None;
            }
            let labels: Vec<&str> = host.split('.').collect();
            let tld = labels.last()?;
            let ok = labels.len() >= 2
                && labels.iter().all(|l| {
                    !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
                && tld.len() >= 2
                && tld.chars().all(|c| c.is_ascii_alphabetic());
            ok.then(|| host.to_ascii_lowercase())
        })
}

/// One link in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The text shown.
    pub text: String,
    /// The host it goes to, lower case.
    pub host: String,
    /// The URL has a `user@` part before the host (`https://anz.co.nz@evil.example/`).
    pub userinfo: bool,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let start = tag.len() - rest.len() + 1;
        let v = tag[start..].trim_start();
        return Some(match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_string(),
            _ => v
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

/// The host of an http(s) URL, and whether it has a `user@` part.
fn url_host(url: &str) -> Option<(String, bool)> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((_, h)) => (true, h),
        None => (false, authority),
    };
    let host = if hostport.starts_with('[') {
        hostport
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    (!host.is_empty()).then(|| (host.to_string(), userinfo))
}

/// The http(s) links in an HTML body, in order.
pub fn links(html: &str) -> Vec<Link> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<a") {
        let start = from + i;
        let after = lower.as_bytes().get(start + 2).copied();
        from = start + 2;
        if !after.is_some_and(|c| c.is_ascii_whitespace()) {
            continue;
        }
        let Some(gt) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + gt];
        let body_start = start + gt + 1;
        let body_end = lower[body_start..]
            .find("</a")
            .map_or(html.len(), |e| body_start + e);
        from = body_end;
        // Character references (`https&#58;//`) decoded as a browser would.
        let Some((host, userinfo)) = attr(tag, "href")
            .map(|h| familiar_search::text::html_to_text(&h))
            .and_then(|h| url_host(&h))
        else {
            continue;
        };
        let text = familiar_search::text::html_to_text(&html[body_start..body_end]);
        out.push(Link {
            text,
            host,
            userinfo,
        });
    }
    out
}

/// Common URL shorteners, which hide where a link goes.
const SHORTENERS: &[&str] = &[
    "bit.ly",
    "tinyurl.com",
    "t.co",
    "goo.gl",
    "ow.ly",
    "is.gd",
    "buff.ly",
    "rebrand.ly",
    "cutt.ly",
    "shorturl.at",
    "rb.gy",
    "t.ly",
    "tiny.cc",
    "lnkd.in",
    "s.id",
];

fn is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

/// Facts about the links: text that names one site but goes to another,
/// links to bare IP addresses, `user@` tricks, shorteners, punycode.
pub fn link_facts(links: &[Link]) -> Vec<Fact> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for l in links {
        let target = registered(&l.host);
        if let Some(shown) = domain_word(&l.text) {
            let shown_reg = registered(&shown);
            if shown_reg != target && seen.insert(format!("m:{shown_reg}>{target}")) {
                out.push(Fact::new(
                    "link_mismatch",
                    format!("{shown} -> {}", l.host),
                    "scam",
                    format!("A link shows {shown} but goes to {}", l.host),
                ));
            }
        }
        if is_ip(&l.host) && seen.insert(format!("ip:{}", l.host)) {
            out.push(Fact::new(
                "link_ip",
                l.host.clone(),
                "scam",
                format!("A link goes to a bare IP address ({})", l.host),
            ));
        }
        if l.userinfo && seen.insert(format!("u:{}", l.host)) {
            out.push(Fact::new(
                "link_userinfo",
                l.host.clone(),
                "scam",
                format!(
                    "A link hides its real site ({}) behind a name before an @",
                    l.host
                ),
            ));
        }
        if SHORTENERS.contains(&target.as_str()) && seen.insert(format!("s:{target}")) {
            out.push(Fact::new(
                "link_shortener",
                target.clone(),
                "",
                format!("A link goes through a URL shortener ({target})"),
            ));
        }
        if l.host.split('.').any(|x| x.starts_with("xn--")) && seen.insert(format!("p:{}", l.host))
        {
            out.push(Fact::new(
                "link_punycode",
                l.host.clone(),
                "scam",
                format!("A link's site name uses look-alike letters ({})", l.host),
            ));
        }
    }
    let domains: std::collections::BTreeSet<String> =
        links.iter().map(|l| registered(&l.host)).collect();
    if !domains.is_empty() {
        out.push(Fact::new(
            "link_domains",
            domains.len().to_string(),
            "",
            format!(
                "Links go to {}: {}",
                if domains.len() == 1 {
                    "1 site".to_string()
                } else {
                    format!("{} sites", domains.len())
                },
                domains
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    out
}

/// Attachment types used to deliver phishing pages or malware.
const RISKY: &[&str] = &[
    "html", "htm", "shtml", "xhtml", "svg", "iso", "img", "vhd", "vhdx", "lnk", "one", "exe",
    "scr", "com", "pif", "bat", "cmd", "js", "jse", "vbs", "vbe", "wsf", "hta", "ps1", "msi",
    "jar", "docm", "xlsm", "pptm", "xlam", "dotm",
];

/// Facts about attachment names: risky types.
pub fn attachment_facts(names: &[String]) -> Vec<Fact> {
    names
        .iter()
        .filter(|n| {
            n.rsplit_once('.')
                .is_some_and(|(_, ext)| RISKY.contains(&ext.to_ascii_lowercase().as_str()))
        })
        .map(|n| {
            Fact::new(
                "attachment",
                n.clone(),
                "scam",
                format!("{n} is a kind of file often used for phishing or malware"),
            )
        })
        .collect()
}

/// Levenshtein distance, for look-alike domains.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != *cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The sender as the message shows it and the mailbox knows it.
pub struct Sender<'a> {
    /// Display name on From.
    pub name: &'a str,
    /// From address, lower case.
    pub addr: &'a str,
    /// Reply-To address, when the headers have one.
    pub reply_to: Option<&'a str>,
    /// What the mailbox says about the From address.
    pub history: &'a familiar_store::SenderHistory,
    /// The message being checked is counted in `history.received`.
    pub counted: bool,
    /// The domains the account has written to.
    pub sent_domains: &'a [String],
}

/// Facts about the sender: how well you know them, Reply-To and display
/// names that point elsewhere, and look-alikes of domains you write to.
pub fn sender_facts(s: &Sender<'_>) -> Vec<Fact> {
    let mut out = Vec::new();
    let domain = domain_of(s.addr);
    let reg = registered(domain);
    let h = s.history;
    let before = h.received - i64::from(s.counted && h.received > 0);
    if h.contact {
        out.push(Fact::new(
            "contact",
            "yes",
            "safe",
            "The sender is one of your contacts",
        ));
    }
    if h.sent > 0 {
        out.push(Fact::new(
            "written_to",
            h.sent.to_string(),
            "safe",
            format!(
                "You've written to this address {} time{}",
                h.sent,
                if h.sent == 1 { "" } else { "s" }
            ),
        ));
    } else if h.domain_sent > 0 {
        out.push(Fact::new(
            "domain_written_to",
            h.domain_sent.to_string(),
            "safe",
            format!(
                "You've never written to this address, but you have written to others at {domain}"
            ),
        ));
    } else if !h.contact {
        out.push(Fact::new(
            "written_to",
            "0",
            "",
            "You've never written to this address or domain",
        ));
    }
    out.push(Fact::new(
        "received_before",
        before.to_string(),
        "",
        match before {
            0 => "This is the first message from this address on this device".to_string(),
            1 => "One earlier message from this address".to_string(),
            n => format!("{n} earlier messages from this address"),
        },
    ));
    if let Some(rt) = s.reply_to.filter(|rt| !rt.eq_ignore_ascii_case(s.addr)) {
        let rt_reg = registered(domain_of(rt));
        if rt_reg != reg {
            out.push(Fact::new(
                "reply_to",
                rt.to_string(),
                "scam",
                format!("Replies go to {rt}, on another domain than the sender's"),
            ));
        }
    }
    let name = s.name.trim();
    if let Some(shown) = address_in(name).filter(|a| a != s.addr) {
        out.push(Fact::new(
            "display_name",
            shown.clone(),
            "scam",
            format!(
                "The sender's name shows the address {shown}, but it was sent from {}",
                s.addr
            ),
        ));
    } else if let Some(shown) = domain_word(name).filter(|d| registered(d) != reg) {
        out.push(Fact::new(
            "display_name",
            shown.clone(),
            "scam",
            format!("The sender's name shows {shown}, but it was sent from {domain}"),
        ));
    }
    if domain.split('.').any(|l| l.starts_with("xn--")) {
        out.push(Fact::new(
            "punycode",
            domain.to_string(),
            "scam",
            format!("The sender's domain {domain} uses look-alike letters"),
        ));
    }
    if h.domain_sent == 0 && reg.len() >= 5 {
        if let Some(known) = s
            .sent_domains
            .iter()
            .map(|d| registered(d))
            .filter(|d| d.len() >= 5 && *d != reg)
            .find(|d| distance(d, &reg) <= 2)
        {
            out.push(Fact::new(
                "lookalike",
                known.clone(),
                "scam",
                format!("{reg} looks like {known}, a domain you write to"),
            ));
        }
    }
    out
}

/// Facts from list headers: legitimate bulk mail has an unsubscribe header.
pub fn list_facts(headers: &[Header]) -> Vec<Fact> {
    match header(headers, "List-Unsubscribe") {
        Some(_) => {
            let one_click = header(headers, "List-Unsubscribe-Post").is_some();
            vec![Fact::new(
                "unsubscribe",
                if one_click { "one-click" } else { "yes" },
                "",
                if one_click {
                    "Has a one-click unsubscribe header, as bulk senders must"
                } else {
                    "Has an unsubscribe header"
                },
            )]
        }
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, value: &str) -> Header {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }

    fn value<'a>(facts: &'a [Fact], name: &str) -> Option<&'a str> {
        facts
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
    }

    /// Cut from a real Hotmail Junk message (9 Nov 2026).
    fn hotmail_junk() -> Vec<Header> {
        vec![
            h("Received", "from SY4PR01MB5721.ausprd01.prod.outlook.com"),
            h(
                "ARC-Authentication-Results",
                "i=1; mx.microsoft.com 1; spf=pass smtp.mailfrom=cbrc.com.ph; dmarc=pass action=none header.from=cbrc.com.ph; dkim=pass header.d=cbrc.com.ph; arc=none",
            ),
            h(
                "Authentication-Results",
                "mx.microsoft.com 1; spf=none (sender IP is 40.107.75.115) smtp.mailfrom=cbrc.com.ph; dkim=none (message not signed) header.d=none;dmarc=none action=none header.from=cbrc.com.ph;compauth=pass reason=130",
            ),
            h(
                "Authentication-Results-Original",
                "mx.microsoft.com 1; dkim=fail header.d=none;dmarc=fail",
            ),
            h("From", "\"IPTV-ABBO offer\" <9939030535@cbrc.com.ph>"),
            h("Reply-To", "<9939030535@cbrc.com.ph>"),
            h(
                "List-Unsubscribe",
                "<mailto:support@iptv-abbo.com?subject=unsubscribe>",
            ),
            h("List-Unsubscribe-Post", "List-Unsubscribe=One-Click"),
            h("X-Microsoft-Antispam-Untrusted", "BCL:0;ARA:1323"),
            h(
                "X-Forefront-Antispam-Report-Untrusted",
                "CIP:255.255.255.255;SCL:1;SFV:NSPM;CAT:NONE;",
            ),
            h("X-Microsoft-Antispam", "BCL:0;ARA:1444111002|970799063;"),
            h("X-MS-Exchange-Organization-SCL", "6"),
            h(
                "X-Microsoft-Antispam-Mailbox-Delivery",
                "ucf:0;jmr:0;ex:0;psp:0;auth:0;dest:J;OFR:SpamFilterAuthJ;ENG:(5062000311);RF:JunkEmail;",
            ),
        ]
    }

    #[test]
    fn hotmail_junk_reads_the_receiving_servers_verdict() {
        let f = server_facts(&hotmail_junk());
        assert_eq!(value(&f, "spf"), Some("none"));
        assert_eq!(value(&f, "dkim"), Some("none"));
        assert_eq!(value(&f, "dmarc"), Some("none"));
        assert_eq!(value(&f, "compauth"), Some("pass"));
        assert_eq!(value(&f, "scl"), Some("6"));
        assert_eq!(value(&f, "delivered_to"), Some("J"));
        // BCL 0 and the sender's -Untrusted copies say nothing.
        assert_eq!(value(&f, "bcl"), None);
        assert_eq!(value(&f, "category"), None);
        assert!(f.iter().any(|x| x.name == "scl" && x.points == "spam"));
        assert_eq!(
            value(&list_facts(&hotmail_junk()), "unsubscribe"),
            Some("one-click")
        );
    }

    #[test]
    fn parklands_inbox_and_an_exchange_header_without_authserv_id() {
        let ok = vec![
            h(
                "Authentication-Results",
                "mx.microsoft.com 1; spf=pass (sender IP is 103.96.23.130) smtp.mailfrom=nzc.nz; dkim=pass (signature was verified) header.d=nzc.nz;dmarc=pass action=none header.from=nzc.nz;compauth=pass reason=100",
            ),
            h("X-MS-Exchange-Organization-SCL", "1"),
            h(
                "X-Forefront-Antispam-Report",
                "CIP:103.96.23.130;CTRY:AU;LANG:en;SCL:1;SRV:;IPV:NLI;SFV:NSPM;H:au-smtp-delivery-130.mimecast.com;CAT:NONE;",
            ),
            h(
                "X-Microsoft-Antispam-Mailbox-Delivery",
                "ucf:0;jmr:0;auth:0;dest:I;ENG:(910005);",
            ),
        ];
        let f = server_facts(&ok);
        assert_eq!(value(&f, "dmarc"), Some("pass"));
        assert_eq!(value(&f, "delivered_to"), Some("I"));
        assert!(f.iter().all(|x| x.points.is_empty()), "{f:?}");

        // Microsoft's documented form, and a forged copy lower down.
        let bare = vec![
            h(
                "Authentication-Results",
                "spf=fail (sender IP is 192.168.1.8) smtp.mailfrom=contoso.com; dkim=none (message not signed) header.d=none; dmarc=fail action=none header.from=contoso.com; compauth=fail reason=000",
            ),
            h(
                "Authentication-Results",
                "spf=pass; dkim=pass; dmarc=pass; compauth=pass reason=100",
            ),
        ];
        let f = server_facts(&bare);
        assert_eq!(value(&f, "spf"), Some("fail"));
        assert_eq!(value(&f, "dmarc"), Some("fail"));
        assert_eq!(value(&f, "compauth"), Some("fail"));
        assert!(f.iter().any(|x| x.name == "compauth" && x.points == "scam"));
        assert!(server_facts(&[]).is_empty());
    }

    #[test]
    fn categories_and_bulk_levels() {
        let f = server_facts(&[
            h("X-Forefront-Antispam-Report", "SCL:5;SFV:SPM;CAT:PHSH;"),
            h("X-Microsoft-Antispam", "BCL:8;"),
        ]);
        assert!(
            f.iter()
                .any(|x| x.name == "category" && x.value == "PHSH" && x.points == "scam")
        );
        assert!(f.iter().any(|x| x.name == "bcl" && x.points == "spam"));
    }

    #[test]
    fn registered_domains() {
        assert_eq!(registered("mail.anz.co.nz"), "anz.co.nz");
        assert_eq!(registered("www.paypal.com"), "paypal.com");
        assert_eq!(registered("example.com.au"), "example.com.au");
        assert_eq!(registered("a.b.example.de"), "example.de");
        assert_eq!(registered("localhost"), "localhost");
    }

    #[test]
    fn links_that_say_one_thing_and_go_to_another() {
        let html = r#"<p>Sign in at <a href="https://anz-secure-verify.top/login?x=1&amp;y=2">www.anz.co.nz</a>.
            <A HREF='http://192.168.0.9/pay'>Pay now</A>
            <a href="https://www.anz.co.nz@evil.example/">here</a>
            <a href="https://bit.ly/3abc">Track parcel</a>
            <a href="mailto:jo@example.com">jo@example.com</a>
            <a name="top">anchor</a>
            <a href="https://www.anz.co.nz/help">anz.co.nz/help</a>
            <a href="https&#58;//evil.example/login">www.anz.co.nz</a>"#;
        let l = links(html);
        assert_eq!(l.len(), 6, "{l:?}");
        assert_eq!(l[5].host, "evil.example");
        assert_eq!(l[0].host, "anz-secure-verify.top");
        assert_eq!(l[0].text, "www.anz.co.nz");
        assert!(l[2].userinfo);
        assert_eq!(l[2].host, "evil.example");
        let f = link_facts(&l);
        assert_eq!(
            value(&f, "link_mismatch"),
            Some("www.anz.co.nz -> anz-secure-verify.top")
        );
        assert_eq!(value(&f, "link_ip"), Some("192.168.0.9"));
        assert_eq!(value(&f, "link_userinfo"), Some("evil.example"));
        assert_eq!(value(&f, "link_shortener"), Some("bit.ly"));
        assert_eq!(f.iter().filter(|x| x.name == "link_mismatch").count(), 2);
        assert_eq!(value(&f, "link_domains"), Some("5"));
    }

    #[test]
    fn risky_attachments() {
        let f = attachment_facts(&[
            "invoice.pdf".into(),
            "Remittance.HTM".into(),
            "photo.jpg".into(),
            "setup.iso".into(),
        ]);
        let names: Vec<&str> = f.iter().map(|x| x.value.as_str()).collect();
        assert_eq!(names, ["Remittance.HTM", "setup.iso"]);
    }

    #[test]
    fn sender_relationship_and_mismatches() {
        let stranger = familiar_store::SenderHistory {
            received: 1,
            ..Default::default()
        };
        let sent = vec!["parklands.school.nz".to_string(), "anz.co.nz".to_string()];
        let f = sender_facts(&Sender {
            name: "ANZ Bank <alerts@anz.co.nz>",
            addr: "alerts@anx.co.nz",
            reply_to: Some("refunds@gmail.com"),
            history: &stranger,
            counted: true,
            sent_domains: &sent,
        });
        assert_eq!(value(&f, "written_to"), Some("0"));
        assert_eq!(value(&f, "received_before"), Some("0"));
        assert_eq!(value(&f, "reply_to"), Some("refunds@gmail.com"));
        assert_eq!(value(&f, "display_name"), Some("alerts@anz.co.nz"));
        assert_eq!(value(&f, "lookalike"), Some("anz.co.nz"));

        let friend = familiar_store::SenderHistory {
            received: 12,
            sent: 4,
            domain_sent: 6,
            contact: true,
        };
        let f = sender_facts(&Sender {
            name: "Jo Smith",
            addr: "jo@parklands.school.nz",
            reply_to: Some("office@parklands.school.nz"),
            history: &friend,
            counted: true,
            sent_domains: &sent,
        });
        assert_eq!(value(&f, "contact"), Some("yes"));
        assert_eq!(value(&f, "written_to"), Some("4"));
        assert_eq!(value(&f, "received_before"), Some("11"));
        assert!(f.iter().all(|x| x.points != "scam"), "{f:?}");

        // A brand in the name that matches the sender is fine.
        let f = sender_facts(&Sender {
            name: "trademe.co.nz",
            addr: "noreply@mail.trademe.co.nz",
            reply_to: None,
            history: &stranger,
            counted: false,
            sent_domains: &[],
        });
        assert_eq!(value(&f, "display_name"), None);
        assert_eq!(value(&f, "received_before"), Some("1"));
    }

    #[test]
    fn addresses_in_headers() {
        assert_eq!(
            address_in("\"Jo\" <Jo@Example.com>").as_deref(),
            Some("jo@example.com")
        );
        assert_eq!(
            address_in("jo@example.com").as_deref(),
            Some("jo@example.com")
        );
        assert_eq!(address_in("Jo Smith"), None);
    }
}
