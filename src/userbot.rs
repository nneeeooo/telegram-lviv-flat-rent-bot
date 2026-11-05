//! Telegram user client (MTProto): login and reading the configured sources.
//!
//! Privacy: the client only ever reads the chats named in `config.toml` (by polling their history).
//! The live update stream, which would carry every chat of the account, is dropped unread.

use crate::listing::Listing;
use crate::session;
use anyhow::{Context, Result, anyhow, bail};
use grammers_client::session::storages::MemorySession;
use grammers_client::session::types::PeerRef;
use grammers_client::media::{Media, Photo};
use grammers_client::message::Message;
use grammers_client::{Client, SenderPool, SignInError};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Userbot {
    pub client: Client,
    session: Arc<MemorySession>,
    session_path: PathBuf,
}

/// One chat we read: a channel or a bot's private chat with us.
pub struct Source {
    pub name: String,
    peer: PeerRef,
    is_channel: bool,
}

impl Userbot {
    pub fn connect(api_id: i32, session_path: PathBuf) -> Result<Self> {
        let session = Arc::new(session::load(&session_path)?);
        let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), api_id);
        tokio::spawn(runner.run());
        Ok(Userbot { client: Client::new(handle), session, session_path })
    }

    pub fn save(&self) -> Result<()> {
        session::save(&self.session, &self.session_path)
    }

    pub async fn ensure_authorized(&self) -> Result<()> {
        if !self.client.is_authorized().await? {
            bail!("Telegram user session missing or expired: run `telegram-lviv-flat-rent-bot login`");
        }
        Ok(())
    }

    /// Interactive login: phone, code from Telegram, 2FA password (hidden input).
    pub async fn login(&self, api_hash: &str) -> Result<()> {
        if self.client.is_authorized().await? {
            println!("Already logged in.");
            return self.save();
        }
        let phone = prompt("Phone (international format, e.g. +380...): ")?;
        let token = self.client.request_login_code(&phone, api_hash).await?;
        let code = prompt("Code from Telegram: ")?;
        match self.client.sign_in(&token, &code).await {
            Ok(_) => {}
            Err(SignInError::PasswordRequired(pt)) => {
                let hint = pt.hint().unwrap_or("none").to_string();
                let pw = rpassword::prompt_password(format!("2FA password (hint: {hint}): "))?;
                self.client.check_password(pt, pw.trim()).await.map_err(|e| anyhow!("{e}"))?;
            }
            Err(e) => bail!("sign in: {e}"),
        }
        self.save()?;
        println!("Logged in. Session saved to {}", self.session_path.display());
        Ok(())
    }

    pub async fn resolve(&self, name: &str, is_channel: bool) -> Result<Source> {
        let peer = self.client.resolve_username(name).await?.with_context(|| format!("@{name} not found"))?;
        let peer = peer.to_ref().await.map_err(|e| anyhow!("{e}"))?.with_context(|| format!("@{name}: no access hash"))?;
        Ok(Source { name: name.to_string(), peer, is_channel })
    }

    /// Messages newer than `after`, oldest first. `after = None` (first run) returns the latest `first_run` ones.
    pub async fn fetch(&self, src: &Source, after: Option<i32>, first_run: usize) -> Result<Vec<Listing>> {
        let mut it = self.client.iter_messages(src.peer).limit(50);
        let (mut out, mut albums) = (Vec::new(), Albums::default());
        while let Some(m) = it.next().await? {
            if after.is_some_and(|a| m.id() <= a) || (after.is_none() && out.len() >= first_run) {
                break;
            }
            out.extend(albums.take(src, &m));
        }
        out.reverse();
        Ok(out)
    }

    /// All incoming messages posted at or after `since` (unix seconds), oldest first.
    pub async fn fetch_since(&self, src: &Source, since: i64) -> Result<Vec<Listing>> {
        let mut it = self.client.iter_messages(src.peer);
        let (mut out, mut albums) = (Vec::new(), Albums::default());
        while let Some(m) = it.next().await? {
            if m.date().as_second() < since {
                break;
            }
            out.extend(albums.take(src, &m));
        }
        out.reverse();
        Ok(out)
    }

    /// Up to `max` newest incoming messages, newest first.
    pub async fn newest(&self, src: &Source, max: usize) -> Result<Vec<Listing>> {
        let mut it = self.client.iter_messages(src.peer).limit(max);
        let (mut out, mut albums) = (Vec::new(), Albums::default());
        while let Some(m) = it.next().await? {
            out.extend(albums.take(src, &m));
        }
        Ok(out)
    }

    /// Photo bytes (largest size Telegram keeps, ~100-300 KB).
    pub async fn download(&self, photo: &Photo) -> Result<Vec<u8>> {
        let mut it = self.client.iter_download(photo);
        let mut bytes = Vec::new();
        while let Some(chunk) = it.next().await? {
            bytes.extend(chunk);
        }
        Ok(bytes)
    }
}

/// Photos sent per flat (an album holds up to 10; 5 keep traffic low).
pub const MAX_PHOTOS: usize = 5;

/// Telegram sends an album as one message per photo sharing a `grouped_id`; the caption sits on the
/// lowest id. Reading newest first, the bare photos come before their caption: park them until it arrives.
#[derive(Default)]
struct Albums(HashMap<i64, Vec<(i32, Photo)>>);

impl Albums {
    /// Listing for this message, or None if it is a bare album photo (kept for its caption) or our own.
    fn take(&mut self, src: &Source, m: &Message) -> Option<Listing> {
        // In a bot chat our own messages (commands) are outgoing; skip them.
        if m.outgoing() {
            return None;
        }
        // Channel posts get Telegram's link preview instead; photos only for bot chats.
        if src.is_channel {
            return Some(src.listing(m, Vec::new()));
        }
        let photo = match m.media() {
            Some(Media::Photo(p)) => Some(p),
            _ => None,
        };
        if let (Some(g), Some(p), true) = (m.grouped_id(), &photo, m.text().is_empty()) {
            self.0.entry(g).or_default().push((m.id(), p.clone()));
            return None;
        }
        let mut photos: Vec<(i32, Photo)> = photo.map(|p| (m.id(), p)).into_iter().collect();
        if let Some(g) = m.grouped_id() {
            photos.extend(self.0.remove(&g).unwrap_or_default());
        }
        photos.sort_by_key(|(id, _)| *id);
        Some(src.listing(m, photos.into_iter().map(|(_, p)| p).take(MAX_PHOTOS).collect()))
    }
}

impl Source {
    fn listing(&self, m: &Message, photos: Vec<Photo>) -> Listing {
        let url = if self.is_channel {
            format!("https://t.me/{}/{}", self.name, m.id())
        } else {
            // Bot chats have no public per-message link: use the listing's own link if the bot gives one.
            message_url(m).unwrap_or_else(|| format!("https://t.me/{}", self.name))
        };
        Listing { id: m.id(), url, text: m.text().to_string(), photos }
    }
}

/// First URL of an inline button or a hidden text link.
fn message_url(m: &Message) -> Option<String> {
    use grammers_client::tl::enums::{
        InlineButtonType, KeyboardInlineButton, KeyboardInlineButtonRow, MessageEntity, ReplyMarkup,
    };
    if let Some(ReplyMarkup::ReplyInlineMarkup(k)) = m.reply_markup() {
        for KeyboardInlineButtonRow::Row(row) in k.rows {
            for KeyboardInlineButton::Button(b) in row.buttons {
                if let InlineButtonType::Url(u) = b.r#type {
                    return Some(u.url);
                }
            }
        }
    }
    m.fmt_entities()?.iter().find_map(|e| match e {
        MessageEntity::TextUrl(t) => Some(t.url.clone()),
        _ => None,
    })
}

fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
