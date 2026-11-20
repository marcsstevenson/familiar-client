// SPDX-License-Identifier: MIT OR Apache-2.0
//! A tiny Gmail client for the tests: bearer auth, JSON in and out, batch requests.

#![allow(dead_code)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use serde_json::Value;

pub struct Client {
    pub http: reqwest::Client,
    pub origin: String,
    pub token: String,
}

pub struct Resp {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub text: String,
    pub body: Value,
}

impl Resp {
    pub fn reason(&self) -> &str {
        self.body["error"]["errors"][0]["reason"]
            .as_str()
            .unwrap_or_default()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn ids(&self, key: &str) -> Vec<String> {
        self.body[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v["id"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Client {
    pub fn new(origin: &str, token: &str) -> Self {
        Self {
            // Never route the loopback mock through a proxy from the environment.
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            origin: origin.to_string(),
            token: token.to_string(),
        }
    }

    /// `path` is under `/gmail/v1/users/me`, or absolute when it starts with `/`.
    fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{path}", self.origin)
        } else {
            format!("{}/gmail/v1/users/me/{path}", self.origin)
        }
    }

    pub async fn send(&self, req: reqwest::RequestBuilder) -> Resp {
        let req = if self.token.is_empty() {
            req
        } else {
            req.bearer_auth(&self.token)
        };
        let r = req.send().await.expect("mock reachable");
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        let text = r.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            text,
            body,
        }
    }

    pub async fn get(&self, path: &str) -> Resp {
        self.send(self.http.get(self.url(path))).await
    }

    pub async fn ok(&self, path: &str) -> Value {
        let r = self.get(path).await;
        assert_eq!(r.status, 200, "{path}: {}", r.text);
        r.body
    }

    pub async fn post(&self, path: &str, body: Value) -> Resp {
        self.send(self.http.post(self.url(path)).json(&body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Resp {
        self.send(self.http.put(self.url(path)).json(&body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Resp {
        self.send(self.http.patch(self.url(path)).json(&body)).await
    }

    pub async fn delete(&self, path: &str) -> Resp {
        self.send(self.http.delete(self.url(path))).await
    }

    /// Every message ID from `messages.list`, following `nextPageToken`.
    pub async fn list_all(&self, query: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut token = String::new();
        loop {
            let sep = if query.is_empty() { "" } else { "&" };
            let page = if token.is_empty() {
                String::new()
            } else {
                format!("{sep}pageToken={token}")
            };
            let r = self.ok(&format!("messages?{query}{page}")).await;
            out.extend(
                r["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|m| m["id"].as_str().unwrap().to_string()),
            );
            match r["nextPageToken"].as_str() {
                Some(t) => token = t.to_string(),
                None => return out,
            }
        }
    }

    /// Sends a batch of `(method, path)` requests and returns each part's status and JSON.
    pub async fn batch(&self, parts: &[(&str, &str)]) -> (Resp, Vec<(u16, Value)>) {
        let mut body = String::new();
        for (i, (method, path)) in parts.iter().enumerate() {
            body.push_str(&format!(
                "--b1\r\nContent-Type: application/http\r\nContent-ID: <item{i}>\r\n\r\n\
                 {method} {path}\r\n\r\n"
            ));
        }
        body.push_str("--b1--\r\n");
        let r = self
            .send(
                self.http
                    .post(format!("{}/batch/gmail/v1", self.origin))
                    .header("content-type", "multipart/mixed; boundary=b1")
                    .body(body),
            )
            .await;
        let parts = parse_batch(&r);
        (r, parts)
    }
}

pub fn parse_batch(r: &Resp) -> Vec<(u16, Value)> {
    let Some(ct) = r.header("content-type") else {
        return Vec::new();
    };
    let Some(b) = ct.split("boundary=").nth(1) else {
        return Vec::new();
    };
    r.text
        .split(&format!("--{b}"))
        .filter(|p| p.contains("HTTP/1.1"))
        .map(|p| {
            let http = &p[p.find("HTTP/1.1").unwrap()..];
            let status: u16 = http.split_whitespace().nth(1).unwrap().parse().unwrap();
            let json = http.split("\r\n\r\n").nth(1).unwrap_or("").trim();
            (status, serde_json::from_str(json).unwrap_or(Value::Null))
        })
        .collect()
}

pub fn b64(raw: &str) -> String {
    URL_SAFE.encode(raw)
}

pub fn unb64(s: &str) -> String {
    String::from_utf8(URL_SAFE.decode(s).unwrap()).unwrap()
}
