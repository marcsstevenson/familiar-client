// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where `config.toml` lives.
//!
//! In order: an explicit path (`--path`), then `FAMILIAR_CONFIG`, then the
//! platform default:
//!
//! | OS | Default |
//! | --- | --- |
//! | Windows | `%USERPROFILE%\.familiar\config.toml` |
//! | macOS | `~/Library/Application Support/Familiar/config.toml` |
//! | Linux and others | `$XDG_CONFIG_HOME/familiar/config.toml`, else `~/.config/familiar/config.toml` |

use std::path::{Path, PathBuf};

/// The environment variable that overrides the config path.
pub const ENV_VAR: &str = "FAMILIAR_CONFIG";

/// Where the path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Given on the command line.
    Explicit,
    /// From `FAMILIAR_CONFIG`.
    Env,
    /// The platform default.
    Default,
}

impl Source {
    /// A short name for output.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Explicit => "explicit",
            Source::Env => "env",
            Source::Default => "default",
        }
    }
}

/// Resolves the config path. `None` only when no home or app-data folder can be found.
pub fn resolve(explicit: Option<&Path>) -> Option<(PathBuf, Source)> {
    if let Some(p) = explicit {
        return Some((p.to_path_buf(), Source::Explicit));
    }
    if let Some(p) = std::env::var_os(ENV_VAR).filter(|v| !v.is_empty()) {
        return Some((PathBuf::from(p), Source::Env));
    }
    default_path().map(|p| (p, Source::Default))
}

/// The platform default path.
pub fn default_path() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        // Not under AppData: Windows redirects AppData for packaged apps
        // (the Claude desktop app) and everything they start, so fam run
        // from there and from a terminal would see different files.
        env("USERPROFILE").map(|d| d.join(".familiar").join("config.toml"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("Familiar")
                .join("config.toml")
        })
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))
            .map(|d| d.join("familiar").join("config.toml"))
    }
}
