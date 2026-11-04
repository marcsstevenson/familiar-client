// SPDX-License-Identifier: MIT OR Apache-2.0
//! A tiny Graph client for the tests, shaped like the provider will be: bearer auth,
//! `Retry-After` handling, and `@odata.nextLink` / `@odata.deltaLink` following.

#![allow(dead_code)]

use std::time::Duration;

use serde_json::Value;

pub struct Client {
    pub http: reqwest::Client,
    pub base: String,
    pub token: String,
    pub prefer: Vec<String>,
}

pub struct Resp {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}

impl Resp {
    pub fn error_code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }

    pub fn values(&self) -> &Vec<Value> {
        self.body["value"].as_array().expect("collection")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

impl Client {
    pub fn new(base: &str, token: &str) -> Self {
        Self {
            // Never route the loopback mock through a proxy from the environment.
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            base: base.to_string(),
            token: token.to_string(),
            prefer: Vec::new(),
        }
    }

    pub fn prefer(mut self, p: &str) -> Self {
        self.prefer.push(p.to_string());
        self
    }

    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        }
    }

    /// One GET, no retries.
    pub async fn get(&self, path: &str) -> Resp {
        let mut req = self.http.get(self.url(path));
        if !self.token.is_empty() {
            req = req.bearer_auth(&self.token);
        }
        for p in &self.prefer {
            req = req.header("Prefer", p);
        }
        let r = req.send().await.expect("mock reachable");
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        let body = r.json().await.unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            body,
        }
    }

    /// GET with retries on 429 and 503, honouring `Retry-After` (in seconds, scaled down to
    /// milliseconds here so tests stay fast). Returns the final response and retry count.
    pub async fn get_retrying(&self, path: &str) -> (Resp, u32) {
        let mut retries = 0;
        loop {
            let r = self.get(path).await;
            if (r.status == 429 || r.status == 503) && retries < 5 {
                let secs: u64 = r
                    .header("retry-after")
                    .and_then(|v| v.parse().ok())
                    .expect("throttle responses carry Retry-After");
                tokio::time::sleep(Duration::from_millis(secs * 10)).await;
                retries += 1;
                continue;
            }
            return (r, retries);
        }
    }

    /// Follows `@odata.nextLink` and returns every item.
    pub async fn get_all(&self, path: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let mut next = Some(self.url(path));
        while let Some(url) = next {
            let (r, _) = self.get_retrying(&url).await;
            assert_eq!(r.status, 200, "{url}: {}", r.body);
            out.extend(r.values().iter().cloned());
            next = r.body["@odata.nextLink"].as_str().map(str::to_string);
        }
        out
    }

    /// Runs one delta round from `start` (a delta path or a saved deltaLink) and returns
    /// the items, the number of pages, and the new deltaLink.
    pub async fn delta_round(&self, start: &str) -> (Vec<Value>, usize, String) {
        let mut items = Vec::new();
        let mut pages = 0;
        let mut url = self.url(start);
        loop {
            let (r, _) = self.get_retrying(&url).await;
            assert_eq!(r.status, 200, "{url}: {}", r.body);
            pages += 1;
            items.extend(r.values().iter().cloned());
            if let Some(n) = r.body["@odata.nextLink"].as_str() {
                assert!(r.body.get("@odata.deltaLink").is_none());
                url = n.to_string();
            } else {
                let d = r.body["@odata.deltaLink"]
                    .as_str()
                    .expect("last page has a deltaLink");
                return (items, pages, d.to_string());
            }
        }
    }
}

pub fn ids(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_string())
        .collect()
}

pub fn removed(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter(|v| v.get("@removed").is_some())
        .map(|v| v["id"].as_str().unwrap().to_string())
        .collect()
}

pub fn present(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter(|v| v.get("@removed").is_none())
        .map(|v| v["id"].as_str().unwrap().to_string())
        .collect()
}
