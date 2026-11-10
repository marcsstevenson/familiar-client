// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam watch`: new mail as it arrives, one JSON line per message.

use std::io::Write as _;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::registry::{Access, Command, CommandSpec, Context};
use crate::watch::{MailEvent, NewMailWatch};

/// How often the store is checked.
const EVERY: Duration = Duration::from_secs(2);

/// `fam watch`.
pub struct Watch;

/// Input for `fam watch`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WatchInput {
    /// Only messages this search matches (`fam mail search` syntax), for
    /// example `from:club`.
    #[serde(default, rename = "match")]
    pub matches: Option<String>,
}

/// Output of `fam watch`. It runs until stopped, so this is never printed;
/// the events go to stdout as they come.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchOutput {
    /// Events printed.
    pub events: u64,
}

impl Command for Watch {
    type Input = WatchInput;
    type Output = WatchOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["watch"],
        summary: "Print new mail as it arrives, as JSON lines",
        description: "Runs until stopped (Ctrl+C), printing one JSON object per new message as famd syncs it: {event: \"new_mail\", id, account, folder, from, to, subject, received_at, preview, has_attachments}. Only mail that arrives after it starts counts; Sent Items, Drafts, Outbox, Deleted Items and Junk are left out. --match keeps only messages a search matches. famd must be running for mail to arrive. Hooks in config.toml ([[hooks]]) get the same events.",
        access: Access::Read,
        errors: &[ErrorKind::Usage, ErrorKind::Internal],
    };

    fn run(&self, ctx: &Context<'_>, i: WatchInput) -> Result<WatchOutput, CommandError> {
        let paths = DaemonPaths::resolve()?;
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64)
        };
        if let Some(q) = &i.matches {
            familiar_search::Query::parse(q, now())
                .map_err(|e| CommandError::usage(format!("--match: {e}")))?;
        }
        (ctx.notify)("Watching for new mail. Stop with Ctrl+C.");
        let mut w = NewMailWatch::new(&paths);
        let mut events = 0;
        let mut out = std::io::stdout();
        loop {
            let ready = match w.poll(now()) {
                Ok(r) => r,
                Err(e) => {
                    (ctx.notify)(&e);
                    Vec::new()
                }
            };
            for ev in filter(&w, i.matches.as_deref(), ready, now(), ctx) {
                let line = serde_json::to_string(&ev).unwrap_or_default();
                // The reader has gone (`fam watch | head -1`): done.
                if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                    return Ok(WatchOutput { events });
                }
                events += 1;
            }
            std::thread::sleep(EVERY);
        }
    }

    fn render_text(&self, _o: &WatchOutput) -> String {
        String::new()
    }
}

fn filter(
    w: &NewMailWatch,
    query: Option<&str>,
    ready: Vec<MailEvent>,
    now: i64,
    ctx: &Context<'_>,
) -> Vec<MailEvent> {
    let Some(q) = query else { return ready };
    if ready.is_empty() {
        return ready;
    }
    let ids: Vec<i64> = ready.iter().map(|e| e.id).collect();
    match w.matching(q, &ids, now) {
        Ok(m) => ready.into_iter().filter(|e| m.contains(&e.id)).collect(),
        Err(e) => {
            (ctx.notify)(&format!("--match: {e}; skipped {} message(s)", ids.len()));
            Vec::new()
        }
    }
}
