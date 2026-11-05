//! Bot API notifier: sends text or a photo album to one chat.

use anyhow::{Result, bail};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct Bot {
    http: reqwest::Client,
    base: String,
}

#[derive(Deserialize)]
struct Resp {
    ok: bool,
    description: Option<String>,
    parameters: Option<RespParams>,
}

#[derive(Deserialize)]
struct RespParams {
    retry_after: Option<u64>,
}

enum Body<'a> {
    Json(serde_json::Value),
    /// Rebuilt per attempt: a multipart form can be sent only once.
    Photos { chat: i64, caption: &'a str, photos: &'a [Vec<u8>] },
}

impl Bot {
    pub fn new(token: &str) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().expect("http client");
        Bot { http, base: format!("https://api.telegram.org/bot{token}") }
    }

    /// Text message; Telegram shows the link preview of the first URL.
    pub async fn send(&self, chat: i64, text: &str) -> Result<()> {
        self.call("sendMessage", Body::Json(json!({"chat_id": chat, "text": text}))).await
    }

    /// One photo or an album (2..=10) with `caption` under it.
    pub async fn send_photos(&self, chat: i64, caption: &str, photos: &[Vec<u8>]) -> Result<()> {
        let method = if photos.len() == 1 { "sendPhoto" } else { "sendMediaGroup" };
        self.call(method, Body::Photos { chat, caption, photos }).await
    }

    /// POST a method; on 429 waits `retry_after` once and retries.
    async fn call(&self, method: &str, body: Body<'_>) -> Result<()> {
        for _ in 0..2 {
            let req = self.http.post(format!("{}/{method}", self.base));
            let req = match &body {
                Body::Json(v) => req.json(v),
                Body::Photos { chat, caption, photos } => req.multipart(photo_form(*chat, caption, photos)),
            };
            let r: Resp = req.send().await?.json().await?;
            if r.ok {
                return Ok(());
            }
            match r.parameters.and_then(|p| p.retry_after) {
                Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
                None => bail!("{method}: {}", r.description.unwrap_or_default()),
            }
        }
        bail!("{method}: still rate-limited")
    }
}

fn photo_form(chat: i64, caption: &str, photos: &[Vec<u8>]) -> Form {
    let part = |i: usize| Part::bytes(photos[i].clone()).file_name(format!("p{i}.jpg")).mime_str("image/jpeg").unwrap();
    let form = Form::new().text("chat_id", chat.to_string());
    if photos.len() == 1 {
        return form.text("caption", caption.to_string()).part("photo", part(0));
    }
    // Album: caption on the first item shows under the whole album.
    let media: Vec<_> = (0..photos.len())
        .map(|i| {
            let mut m = json!({"type": "photo", "media": format!("attach://p{i}")});
            if i == 0 {
                m["caption"] = json!(caption);
            }
            m
        })
        .collect();
    (0..photos.len()).fold(form.text("media", json!(media).to_string()), |f, i| f.part(format!("p{i}"), part(i)))
}
