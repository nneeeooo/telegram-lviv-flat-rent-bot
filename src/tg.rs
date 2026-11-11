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

#[derive(Deserialize)]
struct UpdatesResp {
    ok: bool,
    result: Option<Vec<Update>>,
    description: Option<String>,
}

/// Incoming update; only text messages matter (commands).
#[derive(Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Deserialize)]
pub struct Message {
    pub chat: Chat,
    pub text: Option<String>,
}

#[derive(Deserialize)]
pub struct Chat {
    pub id: i64,
}

/// A photo to send: uploaded bytes, or a public URL Telegram fetches itself.
pub enum PhotoData {
    Bytes(Vec<u8>),
    Url(String),
}

enum Body<'a> {
    Json(serde_json::Value),
    /// Rebuilt per attempt: a multipart form can be sent only once.
    Photos { chat: i64, caption: &'a str, photos: &'a [PhotoData] },
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

    /// Long-poll for messages sent to the bot (waits up to 50 s; the HTTP timeout is 60 s).
    pub async fn updates(&self, offset: i64) -> Result<Vec<Update>> {
        let r: UpdatesResp = self
            .http
            .post(format!("{}/getUpdates", self.base))
            .json(&json!({"offset": offset, "timeout": 50, "allowed_updates": ["message"]}))
            .send()
            .await?
            .json()
            .await?;
        match (r.ok, r.result) {
            (true, Some(u)) => Ok(u),
            _ => bail!("getUpdates: {}", r.description.unwrap_or_default()),
        }
    }

    /// One photo or an album (2..=10) with `caption` under it.
    pub async fn send_photos(&self, chat: i64, caption: &str, photos: &[PhotoData]) -> Result<()> {
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

fn photo_form(chat: i64, caption: &str, photos: &[PhotoData]) -> Form {
    let part = |b: &Vec<u8>, i: usize| Part::bytes(b.clone()).file_name(format!("p{i}.jpg")).mime_str("image/jpeg").unwrap();
    let form = Form::new().text("chat_id", chat.to_string());
    if photos.len() == 1 {
        let form = form.text("caption", caption.to_string());
        return match &photos[0] {
            PhotoData::Bytes(b) => form.part("photo", part(b, 0)),
            PhotoData::Url(u) => form.text("photo", u.clone()),
        };
    }
    // Album: caption on the first item shows under the whole album.
    let media: Vec<_> = photos
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let src = match p {
                PhotoData::Bytes(_) => format!("attach://p{i}"),
                PhotoData::Url(u) => u.clone(),
            };
            let mut m = json!({"type": "photo", "media": src});
            if i == 0 {
                m["caption"] = json!(caption);
            }
            m
        })
        .collect();
    photos.iter().enumerate().fold(form.text("media", json!(media).to_string()), |f, (i, p)| match p {
        PhotoData::Bytes(b) => f.part(format!("p{i}"), part(b, i)),
        PhotoData::Url(_) => f,
    })
}
