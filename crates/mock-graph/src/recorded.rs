// SPDX-License-Identifier: MIT OR Apache-2.0
//! Recorded Graph exchanges, replayed verbatim ahead of the synthetic model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One recorded request and its response.
///
/// String values in `body` may contain `{{baseUrl}}`, which is replaced with the mock's
/// `http://127.0.0.1:<port>/v1.0` so recorded `@odata.nextLink`s point back at the mock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedExchange {
    /// HTTP method, for example `GET`.
    #[serde(default = "default_method")]
    pub method: String,
    /// Path including the version, for example `/v1.0/me/mailboxSettings`. Case-insensitive.
    pub path: String,
    /// Query parameters that must be present with these values. Others are ignored.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Only replay for this mailbox key. `None` replays for any mailbox.
    #[serde(default)]
    pub mailbox: Option<String>,
    /// Response status.
    #[serde(default = "default_status")]
    pub status: u16,
    /// Extra response headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Response body.
    #[serde(default)]
    pub body: serde_json::Value,
}

fn default_method() -> String {
    "GET".into()
}

fn default_status() -> u16 {
    200
}

/// A set of recorded exchanges, usually loaded from a JSON file.
///
/// Recordings must be scrubbed of personal data before they go in the repo: no real
/// addresses, names, subjects, bodies or tokens.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// The exchanges. The match with the most query parameters wins.
    pub exchanges: Vec<RecordedExchange>,
}

impl Recording {
    /// Parses a recording file.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The bundled sample (`fixtures/recorded-sample.json`): `mailboxSettings` and a
    /// paged `people` response, shaped from real Hotmail answers with the data replaced.
    pub fn sample() -> Self {
        Self::from_json(include_str!("../fixtures/recorded-sample.json"))
            .expect("bundled recording parses")
    }

    pub(crate) fn find(
        &self,
        method: &str,
        path: &str,
        params: &[(String, String)],
        mailbox: &str,
    ) -> Option<&RecordedExchange> {
        // The most specific match wins, so a paged recording's later pages (which add
        // `$skip` or `$skiptoken`) are not shadowed by the first page.
        self.exchanges
            .iter()
            .filter(|e| {
                e.method.eq_ignore_ascii_case(method)
                    && e.path.eq_ignore_ascii_case(path)
                    && e.mailbox.as_deref().is_none_or(|m| m == mailbox)
                    && e.query
                        .iter()
                        .all(|(k, v)| params.iter().any(|(pk, pv)| pk == k && pv == v))
            })
            .max_by_key(|e| e.query.len())
    }
}
