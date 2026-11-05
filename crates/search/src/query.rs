// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search query language.
//!
//! A query is a list of words, `"quoted phrases"` and operators, all of which
//! must match. `-` in front of any of them excludes instead.
//!
//! | Operator | Matches |
//! | --- | --- |
//! | `word`, `"a phrase"` | subject, sender, recipients or message text |
//! | `from:ann`, `from:"Ann Smith"` | the sender's name or address |
//! | `to:ann` | a To or Cc recipient's name or address |
//! | `subject:agm`, `subject:"annual meeting"` | the subject |
//! | `body:invoice` | the message text only |
//! | `has:attachment` | messages with file attachments |
//! | `before:2015-01-01` | received before that day (UTC) |
//! | `after:2026-01-01` | received on or after that day (UTC) |
//! | `folder:inbox`, `folder:"Club/Committee"` | in a folder, by name or well-known name |
//! | `account:parklands` | in an account, by its config name or address |
//! | `is:unread`, `is:read`, `is:flagged` | by read or flag state |
//!
//! Dates are `YYYY`, `YYYY-MM` or `YYYY-MM-DD`, or a relative age such as
//! `7d`, `2w`, `6m` or `1y` (so `after:30d` is the last 30 days). An operator
//! the language doesn't know, such as `re:` in a pasted subject, is searched
//! as ordinary text. `AND` between terms is allowed and ignored.

use serde::{Deserialize, Serialize};

/// Which part of a message a text term searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextField {
    /// Subject, sender, recipients and message text.
    Any,
    /// The sender's name and address.
    From,
    /// To and Cc recipients' names and addresses.
    To,
    /// The subject.
    Subject,
    /// The message text.
    Body,
}

/// A word or phrase to look for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextTerm {
    /// Where to look.
    pub field: TextField,
    /// The text as typed (without quotes).
    pub text: String,
    /// Typed in quotes: the words must appear together, in order. Unquoted
    /// text that splits into several words (such as an address) is matched
    /// as a phrase too.
    pub phrase: bool,
}

/// A parsed query. Every part must match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Words and phrases that must match.
    pub text: Vec<TextTerm>,
    /// Words and phrases that must not match (`-word`).
    pub exclude: Vec<TextTerm>,
    /// `has:attachment` (true) or `-has:attachment` (false).
    pub has_attachment: Option<bool>,
    /// Received on or after this time (Unix seconds).
    pub after: Option<i64>,
    /// Received before this time (Unix seconds).
    pub before: Option<i64>,
    /// In one of these folders (names as typed).
    pub folders: Vec<String>,
    /// In none of these folders.
    pub not_folders: Vec<String>,
    /// In one of these accounts (names as typed).
    pub accounts: Vec<String>,
    /// In none of these accounts.
    pub not_accounts: Vec<String>,
    /// `is:unread` (true) or `is:read` (false).
    pub unread: Option<bool>,
    /// `is:flagged` (true) or `-is:flagged` (false).
    pub flagged: Option<bool>,
}

/// Why a query could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

impl Query {
    /// Parses `input`. `now` (Unix seconds) anchors relative dates such as
    /// `after:7d`.
    pub fn parse(input: &str, now: i64) -> Result<Query, ParseError> {
        let mut q = Query::default();
        for tok in tokenize(input) {
            q.add(tok, now)?;
        }
        Ok(q)
    }

    /// Whether the query looks for any words or phrases, as opposed to only
    /// filtering by folder, date and so on.
    pub fn has_text(&self) -> bool {
        !self.text.is_empty()
    }

    /// Whether any word or phrase can match message text, which is only
    /// held locally for the sync window.
    pub fn searches_body(&self) -> bool {
        self.text
            .iter()
            .any(|t| matches!(t.field, TextField::Any | TextField::Body))
    }

    fn add(&mut self, tok: Token, now: i64) -> Result<(), ParseError> {
        let Token {
            negated,
            op,
            value,
            quoted,
        } = tok;
        let text = |field| TextTerm {
            field,
            text: value.clone(),
            phrase: quoted,
        };
        let Some(op) = op else {
            if !quoted && !negated && value.eq_ignore_ascii_case("and") {
                return Ok(());
            }
            self.push_text(negated, text(TextField::Any));
            return Ok(());
        };
        let need_value = || {
            if value.is_empty() {
                Err(ParseError(format!("{op}: needs a value")))
            } else {
                Ok(())
            }
        };
        match op.as_str() {
            "from" => {
                need_value()?;
                self.push_text(negated, text(TextField::From));
            }
            "to" => {
                need_value()?;
                self.push_text(negated, text(TextField::To));
            }
            "subject" => {
                need_value()?;
                self.push_text(negated, text(TextField::Subject));
            }
            "body" => {
                need_value()?;
                self.push_text(negated, text(TextField::Body));
            }
            "has" => match value.to_ascii_lowercase().as_str() {
                "attachment" | "attachments" => self.has_attachment = Some(!negated),
                v => {
                    return Err(ParseError(format!(
                        "has:{v} isn't supported; use has:attachment"
                    )));
                }
            },
            "is" => match (value.to_ascii_lowercase().as_str(), negated) {
                ("unread", n) => self.unread = Some(!n),
                ("read", n) => self.unread = Some(n),
                ("flagged", n) => self.flagged = Some(!n),
                (v, _) => {
                    return Err(ParseError(format!(
                        "is:{v} isn't supported; use is:unread, is:read or is:flagged"
                    )));
                }
            },
            "before" | "after" => {
                need_value()?;
                if negated {
                    return Err(ParseError(format!(
                        "-{op}: isn't supported; use {} instead",
                        if op == "before" { "after:" } else { "before:" }
                    )));
                }
                let t = parse_date(&value, now).ok_or_else(|| {
                    ParseError(format!(
                        "{op}:{value} isn't a date; use YYYY-MM-DD, YYYY-MM, YYYY or an age such as 30d"
                    ))
                })?;
                if op == "before" {
                    self.before = Some(self.before.map_or(t, |b| b.min(t)));
                } else {
                    self.after = Some(self.after.map_or(t, |a| a.max(t)));
                }
            }
            "folder" | "in" => {
                need_value()?;
                if negated {
                    self.not_folders.push(value);
                } else {
                    self.folders.push(value);
                }
            }
            "account" => {
                need_value()?;
                if negated {
                    self.not_accounts.push(value);
                } else {
                    self.accounts.push(value);
                }
            }
            _ => unreachable!("tokenize only returns known operators"),
        }
        Ok(())
    }

    fn push_text(&mut self, negated: bool, t: TextTerm) {
        if t.text.trim().is_empty() {
            return;
        }
        if negated {
            self.exclude.push(t);
        } else {
            self.text.push(t);
        }
    }
}

const OPERATORS: &[&str] = &[
    "from", "to", "subject", "body", "has", "is", "before", "after", "folder", "in", "account",
];

#[derive(Debug, PartialEq, Eq)]
struct Token {
    negated: bool,
    op: Option<String>,
    value: String,
    quoted: bool,
}

/// Splits a query into tokens: `[-][op:]value`, where value may be quoted
/// with `"`. An unclosed quote runs to the end.
fn tokenize(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            return out;
        };
        let mut negated = false;
        if first == '-' {
            chars.next();
            negated = true;
        }
        // Read up to whitespace, honouring quotes anywhere in the token.
        let mut raw = String::new();
        let mut quoted = false;
        let mut was_quoted = false;
        let mut quote_at = None;
        while let Some(&c) = chars.peek() {
            if c == '"' {
                quoted = !quoted;
                was_quoted = true;
                quote_at.get_or_insert(raw.len());
                chars.next();
                continue;
            }
            if c.is_whitespace() && !quoted {
                break;
            }
            raw.push(c);
            chars.next();
        }
        // An operator is a known name before the first colon, and the colon
        // comes before any quote: `from:"Ann Smith"`, not `"re: hello"`.
        let (op, value) = match raw.split_once(':') {
            Some((name, rest))
                if OPERATORS.contains(&name.to_ascii_lowercase().as_str())
                    && quote_at.is_none_or(|q| q > name.len()) =>
            {
                (Some(name.to_ascii_lowercase()), rest.to_string())
            }
            _ => (None, raw),
        };
        if op.is_none() && value.is_empty() && !was_quoted {
            // A lone "-".
            continue;
        }
        out.push(Token {
            negated,
            op,
            value,
            quoted: was_quoted,
        });
    }
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

/// The start (00:00 UTC) of a date, or `now` minus an age.
fn parse_date(s: &str, now: i64) -> Option<i64> {
    let s = s.trim();
    if let Some(unit) = s.chars().last().filter(|c| c.is_ascii_alphabetic()) {
        let n: i64 = s[..s.len() - 1].parse().ok()?;
        let days = match unit.to_ascii_lowercase() {
            'd' => n,
            'w' => n * 7,
            'm' => n * 30,
            'y' => n * 365,
            _ => return None,
        };
        return Some(now - days * 86_400);
    }
    let parts: Vec<&str> = s.split('-').collect();
    let num = |p: &str, len: usize| (p.len() == len).then(|| p.parse::<u32>().ok())?;
    let (y, m, d) = match parts.as_slice() {
        [y] => (num(y, 4)?, 1, 1),
        [y, m] => (num(y, 4)?, num(m, 2)?, 1),
        [y, m, d] => (num(y, 4)?, num(m, 2)?, num(d, 2)?),
        _ => return None,
    };
    let y = i64::from(y);
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_491_200; // 2026-11-01

    fn p(s: &str) -> Query {
        Query::parse(s, NOW).unwrap()
    }

    fn any(t: &str) -> TextTerm {
        TextTerm {
            field: TextField::Any,
            text: t.into(),
            phrase: false,
        }
    }

    #[test]
    fn design_doc_example() {
        let q = p(r#"from:club subject:"AGM" has:attachment after:2026-01-01"#);
        assert_eq!(
            q.text,
            vec![
                TextTerm {
                    field: TextField::From,
                    text: "club".into(),
                    phrase: false
                },
                TextTerm {
                    field: TextField::Subject,
                    text: "AGM".into(),
                    phrase: true
                },
            ]
        );
        assert_eq!(q.has_attachment, Some(true));
        assert_eq!(q.after, Some(1_767_225_600));
        assert_eq!(q.before, None);
    }

    #[test]
    fn words_phrases_and_negation() {
        let q = p(r#"invoice "annual meeting" -spam AND -"do not reply""#);
        assert_eq!(
            q.text,
            vec![
                any("invoice"),
                TextTerm {
                    phrase: true,
                    ..any("annual meeting")
                }
            ]
        );
        assert_eq!(
            q.exclude,
            vec![
                any("spam"),
                TextTerm {
                    phrase: true,
                    ..any("do not reply")
                }
            ]
        );
    }

    #[test]
    fn filters() {
        let q = p(
            r#"folder:inbox in:"Club/Committee" -folder:junk account:parklands is:unread -is:flagged before:2015"#,
        );
        assert_eq!(q.folders, vec!["inbox", "Club/Committee"]);
        assert_eq!(q.not_folders, vec!["junk"]);
        assert_eq!(q.accounts, vec!["parklands"]);
        assert_eq!(q.unread, Some(true));
        assert_eq!(q.flagged, Some(false));
        assert_eq!(q.before, Some(1_420_070_400));
        assert!(!q.has_text());
        assert_eq!(p("is:read").unread, Some(false));
        assert_eq!(p("-has:attachment").has_attachment, Some(false));
    }

    #[test]
    fn dates() {
        assert_eq!(p("after:2026-11").after, Some(NOW));
        assert_eq!(p("after:7d").after, Some(NOW - 7 * 86_400));
        assert_eq!(p("after:1y").after, Some(NOW - 365 * 86_400));
        assert_eq!(p("before:1970-01-01").before, Some(0));
        assert_eq!(p("before:2024-02-29").before, Some(1_709_164_800));
        // The tighter bound wins.
        let q = p("after:2020 after:2021 before:2025 before:2024");
        assert_eq!(
            (q.after, q.before),
            (Some(1_609_459_200), Some(1_704_067_200))
        );
        for bad in [
            "after:2023-02-29",
            "before:yesterday",
            "after:20-1-1",
            "after:",
            "-before:2020",
        ] {
            assert!(Query::parse(bad, NOW).is_err(), "{bad}");
        }
    }

    #[test]
    fn unknown_operators_are_text() {
        let q = p(r#"re:hello "re: hello" http://example.com"#);
        assert_eq!(q.text[0], any("re:hello"));
        assert_eq!(
            q.text[1],
            TextTerm {
                phrase: true,
                ..any("re: hello")
            }
        );
        assert_eq!(q.text[2], any("http://example.com"));
    }

    #[test]
    fn bad_values_are_errors() {
        for bad in ["is:urgent", "has:link", "from:", "folder:"] {
            let e = Query::parse(bad, NOW).unwrap_err();
            assert!(!e.0.is_empty(), "{bad}");
        }
    }

    #[test]
    fn odd_input_does_not_panic() {
        for s in [
            "",
            "  ",
            "-",
            "\"",
            "-\"",
            "from:\"unclosed",
            "::",
            "a:b:c",
            "é:ü",
        ] {
            let _ = Query::parse(s, NOW);
        }
        assert_eq!(p("").text, vec![]);
        assert_eq!(p("from:\"Ann Smith").text[0].text, "Ann Smith");
    }

    #[test]
    fn body_and_text_flags() {
        assert!(p("invoice").searches_body());
        assert!(p("body:invoice").searches_body());
        assert!(!p("from:ann subject:agm").searches_body());
        assert!(p("from:ann").has_text());
    }
}
