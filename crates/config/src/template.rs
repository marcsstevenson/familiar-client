// SPDX-License-Identifier: MIT OR Apache-2.0
//! The config a new install starts from (`fam init`, or the first `fam account add`).
//! Accounts are added below it with [`crate::ConfigDocument`].

/// A short, commented starting config: examples of the sections people most often change,
/// commented out so the defaults apply, then the version. Accounts are added after it
/// (comments after the last value would end up below the accounts). `[app] client_id` is left
/// out on purpose, so a new install follows Familiar's own client ID.
pub const TEMPLATE: &str = r#"# Familiar's settings. Every section is described in docs/config.md, and
# `fam config validate` checks this file. It never holds passwords, tokens or
# mail, but it will hold your signatures, so keep any copy of it private.
#
# Accounts follow `version` below; `fam account add` writes them for you.
# Some settings you might change later, commented out so the defaults apply:
#
# Appearance (also editable in the desktop app).
# [app]
# theme = "system"
# [desktop]
# accent = '#99D9EA'
# neon = false
#
# How much mail to keep on this computer.
# [sync]
# window = "1y"         # full messages for the last year
# cache_limit = "2GB"   # older messages opened on demand
#
# Single-key shortcuts, for example:
# [keys]
# archive = "e"
#
# What AI agents may do through `fam` and `fam mcp serve` (read and draft by default).
# [agent]
# allow = ["read", "draft"]
#
# Run a command when new mail arrives (see docs/hooks.md).
# [[hooks]]
# name = "Notify"
# on = "new_mail"
# run = "notify-send 'New mail'"

version = 1
"#;

/// Writes [`TEMPLATE`] to `path` if nothing is there, creating its folder. Returns true
/// when it wrote the file; an existing file is never touched.
pub fn create(path: &std::path::Path) -> std::io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(TEMPLATE.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_valid_and_only_warns_about_no_accounts() {
        let report = crate::check(TEMPLATE);
        assert_eq!(report.errors().count(), 0, "{:?}", report.problems);
    }

    #[test]
    fn create_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("familiar-template-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("config.toml");
        assert!(create(&path).unwrap());
        std::fs::write(&path, "version = 1\n# mine\n").unwrap();
        assert!(!create(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "version = 1\n# mine\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accounts_added_to_the_template_validate() {
        let mut doc = crate::ConfigDocument::parse(TEMPLATE).unwrap();
        doc.set(&["accounts", "hotmail", "email"], "a@hotmail.example")
            .unwrap();
        doc.set(&["accounts", "hotmail", "tenant"], "consumers")
            .unwrap();
        doc.set(&["accounts", "gmail", "provider"], "gmail")
            .unwrap();
        doc.set(&["accounts", "gmail", "email"], "a@gmail.example")
            .unwrap();
        let text = doc.to_string();
        let report = crate::check(&text);
        assert_eq!(report.problems.len(), 0, "{:?}\n{text}", report.problems);
        assert!(
            text.trim_end().ends_with("email = \"a@gmail.example\""),
            "{text}"
        );
    }
}
