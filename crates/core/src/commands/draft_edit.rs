// SPDX-License-Identifier: MIT OR Apache-2.0
//! `fam draft edit`: write a draft in your own editor, in Markdown.
//!
//! The draft goes into a temporary file as a few header lines (To, Cc,
//! Bcc, Subject), a blank line and the body as Markdown, and `$VISUAL` or
//! `$EDITOR` opens it (Notepad on Windows and vi elsewhere without either).
//! When the editor closes, what changed is saved with `draft update`, the
//! Markdown turned into HTML. The signature and any quoted message below
//! your text are kept as they were. Without a draft ID it writes a new
//! message, which gets the account's signature as `draft create` does.

use std::path::{Path, PathBuf};

use crate::commands::draft::{
    ComposeArgs, ComposeSettings, account_of, compose_at, render as render_write,
};
use crate::commands::mail::{BodyFormat, BodyType, MailAddress, MailFetchInput, fetch_at};
use crate::commands::write::{WriteFlags, flags, submit_at};
use crate::compose::Recipient;
use crate::daemon::DaemonPaths;
use crate::error::{CommandError, ErrorKind};
use crate::markdown;
use crate::ops::{WriteOutput, WriteRequest};
use crate::registry::{Access, Command, CommandSpec, Context};
use schemars::JsonSchema;
use serde::Deserialize;

/// `fam draft edit`.
pub struct DraftEdit;

/// Input for `fam draft edit`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftEditInput {
    /// Local ID of the draft; leave it out to write a new message.
    #[serde(default)]
    #[schemars(extend("x-positional" = 0))]
    pub draft: Option<i64>,
    /// The account to write a new message from (config name or address); needed with more than one.
    #[serde(default)]
    pub account: Option<String>,
    /// Leave the account's signature off a new message.
    #[serde(default)]
    pub no_signature: bool,
    /// Wait until the server has the draft (needs famd).
    #[serde(default)]
    pub wait: bool,
    /// Seconds --wait waits at most (default 60).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Command for DraftEdit {
    type Input = DraftEditInput;
    type Output = WriteOutput;
    const SPEC: CommandSpec = CommandSpec {
        path: &["draft", "edit"],
        summary: "Write a draft in your editor, in Markdown",
        description: "Opens a draft (or, without an ID, a new message) in $VISUAL or $EDITOR as To/Cc/Bcc/Subject lines, a blank line and the body in Markdown. Saving and closing the editor saves the changes, with the Markdown as HTML; the signature and quoted message below your text are kept. Use an editor command that waits, such as code --wait.",
        access: Access::Draft,
        errors: &[
            ErrorKind::NotFound,
            ErrorKind::Config,
            ErrorKind::PermissionDenied,
            ErrorKind::DaemonUnavailable,
            ErrorKind::Offline,
            ErrorKind::Server,
        ],
    };

    fn run(&self, _ctx: &Context<'_>, i: DraftEditInput) -> Result<WriteOutput, CommandError> {
        let editor = editor_command();
        edit_at(
            &DaemonPaths::resolve()?,
            &i,
            &ComposeSettings::load()?,
            &|path: &Path| run_editor(&editor, path),
        )
    }

    fn render_text(&self, o: &WriteOutput) -> String {
        if o.ops.is_empty() {
            "No changes, so nothing was saved.".into()
        } else {
            render_write(o)
        }
    }
}

/// The editor to run: `$VISUAL`, then `$EDITOR`, then the platform's own.
pub fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.into())
}

/// Runs `editor` on `path` through the shell, as git does, so that an
/// editor with arguments (`code --wait`) works.
pub fn run_editor(editor: &str, path: &Path) -> Result<(), CommandError> {
    let mut cmd;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd = std::process::Command::new("cmd");
        // `/S`: cmd drops the outer quotes and runs the rest as written.
        cmd.arg("/S")
            .arg("/C")
            .raw_arg(format!("\"{editor} \"{}\"\"", path.display()));
    }
    #[cfg(not(windows))]
    {
        cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(path);
    }
    let status = cmd.status().map_err(|e| {
        CommandError::new(ErrorKind::Config, format!("couldn't start {editor}: {e}"))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(CommandError::new(
            ErrorKind::Config,
            format!("{editor} exited with {status}"),
        ))
    }
}

/// What the file holds.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Written {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
}

impl Written {
    fn to_file(&self) -> String {
        format!(
            "To: {}\nCc: {}\nBcc: {}\nSubject: {}\n\n{}\n",
            self.to, self.cc, self.bcc, self.subject, self.body
        )
    }

    fn parse(text: &str) -> Result<Self, String> {
        // Notepad writes a byte order mark and CRLF line ends.
        let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let (head, body) = text.split_once("\n\n").unwrap_or((text.as_str(), ""));
        let mut w = Written {
            body: body.trim().to_string(),
            ..Self::default()
        };
        for line in head.lines().filter(|l| !l.trim().is_empty()) {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| format!("{line:?} isn't a header line (Name: value)"))?;
            let value = value.trim().to_string();
            match name.trim().to_ascii_lowercase().as_str() {
                "to" => w.to = value,
                "cc" => w.cc = value,
                "bcc" => w.bcc = value,
                "subject" => w.subject = value,
                other => {
                    return Err(format!(
                        "unknown header {other:?}; the lines above the first blank line are To, Cc, Bcc and Subject"
                    ));
                }
            }
        }
        Ok(w)
    }
}

fn addresses(v: &[MailAddress]) -> String {
    v.iter()
        .map(|a| {
            if a.name.is_empty() || a.name == a.addr {
                a.addr.clone()
            } else {
                format!("{} <{}>", a.name, a.addr)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A draft's HTML in three parts: up to and including `<body>`, the text
/// written above the signature or quoted message, and those (kept as is).
struct Body<'a> {
    prefix: &'a str,
    text: &'a str,
    tail: &'a str,
}

/// Where what Familiar, Outlook and the desktop app put below your text
/// starts.
const TAIL_MARKERS: &[&str] = &[
    "id=\"signature\"",
    "id=\"divrplyfwdmsg\"",
    "id=\"appendonsend\"",
];

fn split(html: &str) -> Body<'_> {
    // ASCII lowering keeps byte offsets.
    let lower = html.to_ascii_lowercase();
    let start = lower
        .find("<body")
        .and_then(|b| lower[b..].find('>').map(|e| b + e + 1))
        .unwrap_or(0);
    let mut end = lower.find("</body").unwrap_or(html.len()).max(start);
    for m in TAIL_MARKERS {
        let Some(at) = lower[start..end].find(m).map(|p| start + p) else {
            continue;
        };
        let Some(mut tag) = lower[start..at].rfind('<').map(|p| start + p) else {
            continue;
        };
        // The rule above a quoted message belongs with it.
        if *m == "id=\"divrplyfwdmsg\"" {
            if let Some(hr) = lower[start..tag].rfind("<hr").map(|p| start + p) {
                if familiar_search::text::html_to_text(&html[hr..tag])
                    .trim()
                    .is_empty()
                {
                    tag = hr;
                }
            }
        }
        end = end.min(tag);
    }
    Body {
        prefix: &html[..start],
        text: &html[start..end],
        tail: &html[end..],
    }
}

/// The temp file a draft is edited in.
fn temp_path(draft: Option<i64>) -> PathBuf {
    let name = match draft {
        Some(d) => format!("familiar-draft-{d}-{}.md", std::process::id()),
        None => format!("familiar-draft-new-{}.md", std::process::id()),
    };
    std::env::temp_dir().join(name)
}

/// Writes `w` to a temp file, runs `edit` on it and reads it back. An error
/// after editing names the file, which is kept so nothing written is lost.
fn edit_file(
    draft: Option<i64>,
    w: &Written,
    edit: &dyn Fn(&Path) -> Result<(), CommandError>,
) -> Result<(Written, PathBuf), CommandError> {
    let path = temp_path(draft);
    std::fs::write(&path, w.to_file())
        .map_err(|e| CommandError::new(ErrorKind::Config, format!("{}: {e}", path.display())))?;
    if let Err(e) = edit(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CommandError::new(ErrorKind::Config, format!("{}: {e}", path.display())))?;
    let edited = Written::parse(&text).map_err(|e| kept(&path, CommandError::usage(e)))?;
    Ok((edited, path))
}

/// `e`, saying that the text is still in `path`.
fn kept(path: &Path, e: CommandError) -> CommandError {
    CommandError::new(
        e.kind,
        format!(
            "{}; what you wrote is still in {}",
            e.message,
            path.display()
        ),
    )
}

/// A recipient line as the update wants it: `None` when unchanged.
fn line(before: &str, after: &str) -> Result<Option<Vec<Recipient>>, CommandError> {
    if before.trim() == after.trim() {
        Ok(None)
    } else if after.trim().is_empty() {
        Ok(Some(Vec::new()))
    } else {
        Recipient::parse_all(&[after.to_string()]).map(Some)
    }
}

/// `fam draft edit` with the editor given: `edit` changes the file in place.
pub fn edit_at(
    paths: &DaemonPaths,
    i: &DraftEditInput,
    settings: &ComposeSettings,
    edit: &dyn Fn(&Path) -> Result<(), CommandError>,
) -> Result<WriteOutput, CommandError> {
    let f = flags(false, i.wait, i.wait_secs);
    match i.draft {
        None => edit_new(paths, i, settings, edit, &f),
        Some(id) => edit_existing(paths, id, edit, &f),
    }
}

fn edit_new(
    paths: &DaemonPaths,
    i: &DraftEditInput,
    settings: &ComposeSettings,
    edit: &dyn Fn(&Path) -> Result<(), CommandError>,
    f: &WriteFlags,
) -> Result<WriteOutput, CommandError> {
    let (w, path) = edit_file(None, &Written::default(), edit)?;
    if w == Written::default() {
        let _ = std::fs::remove_file(&path);
        return Ok(nothing());
    }
    let some = |s: &str| {
        if s.is_empty() {
            Vec::new()
        } else {
            vec![s.to_string()]
        }
    };
    let args = ComposeArgs {
        account: i.account.clone(),
        to: some(&w.to),
        cc: some(&w.cc),
        bcc: some(&w.bcc),
        subject: (!w.subject.is_empty()).then(|| w.subject.clone()),
        body: Some(w.body.clone()),
        markdown: true,
        no_signature: i.no_signature,
        ..ComposeArgs::default()
    };
    let out = compose_at(paths, &args, settings, None, f).map_err(|e| kept(&path, e))?;
    let _ = std::fs::remove_file(&path);
    Ok(out)
}

fn edit_existing(
    paths: &DaemonPaths,
    id: i64,
    edit: &dyn Fn(&Path) -> Result<(), CommandError>,
    f: &WriteFlags,
) -> Result<WriteOutput, CommandError> {
    need_draft(paths, id)?;
    let m = fetch_at(
        paths,
        &MailFetchInput {
            message: id,
            format: BodyFormat::Html,
            local: false,
        },
    )?;
    let raw = m.body.unwrap_or_default();
    let html = match m.body_type {
        Some(BodyType::Html) => raw,
        _ => crate::compose::text_to_html(&raw),
    };
    let body = split(&html);
    let before = Written {
        to: addresses(&m.header.to),
        cc: addresses(&m.header.cc),
        bcc: String::new(),
        subject: m.header.subject.clone(),
        body: markdown::from_html(body.text),
    };
    let (after, path) = edit_file(Some(id), &before, edit)?;
    let html = if after.body == before.body {
        None
    } else {
        let mut new = format!("{}{}", body.prefix, markdown::to_html(&after.body));
        if body.tail.contains("id=\"Signature\"") || body.tail.contains("id=\"signature\"") {
            // A blank line between the sign-off and the signature.
            new.push_str("<div><br></div>\n");
        }
        new.push_str(body.tail);
        let (html, images) = crate::compose::embedded_images(&new).map_err(|e| kept(&path, e))?;
        Some((html, images))
    };
    let parsed = (|| {
        Ok::<_, CommandError>((
            line(&before.to, &after.to)?,
            line(&before.cc, &after.cc)?,
            // Bcc isn't kept on this device, so a blank line leaves it alone.
            if after.bcc.is_empty() {
                None
            } else {
                line("", &after.bcc)?
            },
        ))
    })();
    let (to, cc, bcc) = parsed.map_err(|e| kept(&path, e))?;
    let subject = (after.subject != before.subject).then(|| after.subject.clone());
    if html.is_none() && to.is_none() && cc.is_none() && bcc.is_none() && subject.is_none() {
        let _ = std::fs::remove_file(&path);
        return Ok(nothing());
    }
    let (html, attachments) = match html {
        Some((h, a)) => (Some(h), a),
        None => (None, Vec::new()),
    };
    let out = submit_at(
        paths,
        WriteRequest::DraftUpdate {
            message: id,
            from: None,
            subject,
            html,
            to,
            cc,
            bcc,
            attachments,
            remove_attachments: Vec::new(),
        },
        f,
    )
    .map_err(|e| kept(&path, e))?;
    let _ = std::fs::remove_file(&path);
    Ok(out)
}

fn nothing() -> WriteOutput {
    WriteOutput {
        dry_run: false,
        waited: false,
        ops: Vec::new(),
    }
}

/// Refuses before the editor opens if `id` isn't a draft.
fn need_draft(paths: &DaemonPaths, id: i64) -> Result<(), CommandError> {
    let (_, m) = account_of(paths, id)?;
    let r = crate::commands::mail::open_store(paths)?;
    let folder = r
        .folder(m.folder)
        .map_err(crate::commands::mail::store_err)?;
    if folder.and_then(|f| f.well_known).as_deref() != Some("drafts") {
        return Err(CommandError::usage(format!(
            "message {id} isn't a draft (it isn't in Drafts)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trips() {
        let w = Written {
            to: "Ana <ana@x.test>, bo@y.test".into(),
            cc: String::new(),
            bcc: String::new(),
            subject: "Hi: there".into(),
            body: "Line one\nline two\n\n**bold**".into(),
        };
        assert_eq!(Written::parse(&w.to_file()).unwrap(), w);
    }

    #[test]
    fn notepad_files_parse() {
        let w =
            Written::parse("\u{feff}To: a@x.test\r\nSubject: S\r\n\r\nBody\r\nmore\r\n").unwrap();
        assert_eq!(w.to, "a@x.test");
        assert_eq!(w.subject, "S");
        assert_eq!(w.body, "Body\nmore");
    }

    #[test]
    fn unknown_headers_are_refused() {
        assert!(Written::parse("From: me\n\nbody").is_err());
        assert!(Written::parse("just text\n\nbody").is_err());
    }

    #[test]
    fn split_keeps_signature_and_quote() {
        let html = "<html><head><style>p{}</style></head><body><div>Hello<br>\n<div><br></div>\n<div id=\"Signature\">-- Pat</div></div><hr><div id=\"divRplyFwdMsg\"><b>From:</b> x</div><br>old</body></html>";
        let b = split(html);
        assert_eq!(b.prefix, "<html><head><style>p{}</style></head><body>");
        assert_eq!(b.text, "<div>Hello<br>\n<div><br></div>\n");
        assert!(b.tail.starts_with("<div id=\"Signature\">"));
        assert_eq!(markdown::from_html(b.text), "Hello");
    }

    #[test]
    fn split_takes_the_rule_with_the_quote() {
        let html = "<div>Reply</div><hr style=\"x\"><div id=\"divRplyFwdMsg\">From: y</div>";
        let b = split(html);
        assert_eq!(b.text, "<div>Reply</div>");
        assert!(b.tail.starts_with("<hr"));
    }

    #[cfg(not(windows))]
    #[test]
    fn editor_runs_through_the_shell() {
        let dir = std::env::temp_dir().join(format!("fam-editor-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("d.md");
        std::fs::write(&file, "old").unwrap();
        // An editor command with arguments, as $EDITOR often is.
        run_editor("printf '%s' new >", &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        let err = run_editor("false", &file).unwrap_err();
        assert!(err.message.contains("false exited"), "{err:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_without_markers_is_all_text() {
        let b = split("<p>Just this</p>");
        assert_eq!((b.prefix, b.text, b.tail), ("", "<p>Just this</p>", ""));
    }
}
