//! Lviv flat-rent watcher: reads Telegram channels/bots, filters offers, notifies one chat.
//!
//! `telegram-lviv-flat-rent-bot login`                 — one-time interactive login of the user account.
//! `telegram-lviv-flat-rent-bot`                       — watch sources and send matching flats.
//! `telegram-lviv-flat-rent-bot backfill <days> [--send]` — replay recent channel posts (stats; `--send` sends).

mod config;
mod filter;
mod listing;
mod parse;
mod rates;
mod session;
mod state;
mod tg;
mod userbot;
mod zone;

use anyhow::{Context, Result};
use filter::Skip;
use listing::Listing;
use rates::Rates;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Posts checked per source on the very first run.
const FIRST_RUN: usize = 3;
const RATES_EVERY: Duration = Duration::from_secs(12 * 3600);

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} not set"))
}

/// Card for a post that passes the filter and was not sent before.
fn evaluate(l: &Listing, cfg: &config::Config, rates: Option<Rates>, st: &mut state::State) -> Result<String, Skip> {
    let p = parse::parse(&l.text);
    let place = zone::place(&l.text);
    filter::decide(&l.text, &p, &place, &cfg.filter, rates)?;
    if st.seen_before(&l.text) {
        return Err(Skip::Duplicate);
    }
    Ok(l.card(&p, &place, rates))
}

/// Send a card: with the listing's album photos if it has them (bot sources), else as text with link preview.
/// A failed photo download or upload falls back to text, so a flat is never lost for a photo.
async fn deliver(user: &userbot::Userbot, bot: &tg::Bot, chat: i64, l: &Listing, card: &str) -> Result<()> {
    if !l.photos.is_empty() {
        let mut photos = Vec::new();
        for p in &l.photos {
            match user.download(p).await {
                Ok(b) => photos.push(b),
                Err(e) => eprintln!("photo of {}: {e:#}", l.id),
            }
        }
        if !photos.is_empty() {
            match bot.send_photos(chat, card, &photos).await {
                Ok(()) => return Ok(()),
                Err(e) => eprintln!("album of {}: {e:#}", l.id),
            }
        }
    }
    bot.send(chat, card).await
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let state_dir = PathBuf::from(std::env::var("STATE_DIR").unwrap_or_else(|_| "state".into()));
    let api_id: i32 = env("TG_API_ID")?.trim().parse().context("TG_API_ID must be a number")?;
    let user = userbot::Userbot::connect(api_id, state_dir.join("tg-session.json"))?;
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some("login") {
        return user.login(env("TG_API_HASH")?.trim()).await;
    }

    let token = env("TELEGRAM_BOT_TOKEN")?;
    let chat: i64 = env("TELEGRAM_CHAT_ID")?.trim().parse().context("TELEGRAM_CHAT_ID must be a number")?;
    let cfg = config::Config::load(&std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into()))?;
    let poll = Duration::from_secs(std::env::var("POLL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60));
    let bot = tg::Bot::new(&token);
    let http = reqwest::Client::new();
    let mut rates = Rates::fetch(&http).await.inspect_err(|e| eprintln!("PrivatBank rates: {e:#}")).ok();
    let mut rates_at = Instant::now();

    user.ensure_authorized().await?;

    if args.get(1).map(String::as_str) == Some("backfill") {
        let days: i64 = args.get(2).and_then(|s| s.parse().ok()).context("usage: backfill <days> [--send]")?;
        let send = args.iter().any(|a| a == "--send");
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64 - days * 86400;
        // Throwaway state: backfill never touches the watcher's progress or sent list.
        let mut st = state::State::default();
        let names = cfg.sources.channels.iter().map(|n| (n, true)).chain(cfg.sources.bots.iter().map(|n| (n, false)));
        for (name, is_channel) in names {
            let src = user.resolve(name, is_channel).await?;
            let all = user.fetch_since(&src, since).await?;
            // `--dump`: raw texts to state/sample-<source>.json, for tuning the parser.
            if args.iter().any(|a| a == "--dump") {
                let texts: Vec<_> = all.iter().filter(|l| !l.text.trim().is_empty()).map(|l| (l.id, &l.url, &l.text)).collect();
                state::write_atomic(&state_dir.join(format!("sample-{name}.json")), &serde_json::to_vec_pretty(&texts)?)?;
            }
            let mut skipped: BTreeMap<Skip, usize> = BTreeMap::new();
            let mut cards = Vec::new();
            for l in &all {
                match evaluate(l, &cfg, rates, &mut st) {
                    Ok(card) => cards.push((l, card)),
                    Err(s) => *skipped.entry(s).or_default() += 1,
                }
            }
            println!("@{name}: {} posts in {days} d, {} pass. Skipped: {skipped:?}", all.len(), cards.len());
            for (l, card) in &cards {
                if send {
                    deliver(&user, &bot, chat, l, card).await?;
                    tokio::time::sleep(Duration::from_millis(1100)).await; // Bot API: ~1 msg/s per chat
                } else {
                    println!("---\n{card}");
                }
            }
        }
        return Ok(());
    }

    // `latest <n> [--send]`: the n newest matching posts per source, searching back up to LATEST_SCAN messages.
    if args.get(1).map(String::as_str) == Some("latest") {
        // Options: --scan <messages> (default 100), --only <source>, --all (no filter: test cards).
        let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1));
        let n: usize = args.get(2).and_then(|s| s.parse().ok()).context("usage: latest <n> [--send]")?;
        let scan: usize = opt("--scan").and_then(|s| s.parse().ok()).unwrap_or(100);
        let only = opt("--only");
        let all_posts = args.iter().any(|a| a == "--all");
        let send = args.iter().any(|a| a == "--send");
        let mut st = state::State::default(); // throwaway, as in backfill
        let names = cfg.sources.channels.iter().map(|s| (s, true)).chain(cfg.sources.bots.iter().map(|s| (s, false)));
        for (name, is_channel) in names.filter(|(s, _)| only.is_none_or(|o| o == *s)) {
            let src = user.resolve(name, is_channel).await?;
            let all = user.newest(&src, scan).await?;
            let mut cards: Vec<(&Listing, String)> = all
                .iter()
                .filter_map(|l| {
                    if all_posts {
                        let p = parse::parse(&l.text);
                        let card = l.card(&p, &zone::place(&l.text), rates);
                        // Skip service messages ("Надіслав 8."): a listing has a price or rooms.
                        (p.price.is_some() || p.rooms.is_some()).then(|| (l, format!("🧪 тест, без фільтра\n{card}")))
                    } else {
                        evaluate(l, &cfg, rates, &mut st).ok().map(|c| (l, c))
                    }
                })
                .take(n)
                .collect();
            cards.reverse(); // oldest first, like live
            println!("@{name}: {} matches in the last {} messages", cards.len(), all.len());
            for (l, card) in &cards {
                if send {
                    deliver(&user, &bot, chat, l, card).await?;
                    tokio::time::sleep(Duration::from_millis(1100)).await;
                } else {
                    println!("---\n{card}");
                }
            }
        }
        return Ok(());
    }

    let mut sources = Vec::new();
    for name in &cfg.sources.channels {
        sources.push(user.resolve(name, true).await?);
    }
    for name in &cfg.sources.bots {
        sources.push(user.resolve(name, false).await?);
    }
    user.save()?;
    let mut st = state::State::load(state_dir.join("state.json"))?;

    bot.send(chat, &format!("🏠 Бот запущено.\n{}", cfg.summary())).await?;
    println!("started: {} sources, poll every {}s", sources.len(), poll.as_secs());

    loop {
        if rates.is_none() || rates_at.elapsed() > RATES_EVERY {
            match Rates::fetch(&http).await {
                Ok(r) => (rates, rates_at) = (Some(r), Instant::now()),
                Err(e) => eprintln!("PrivatBank rates: {e:#}"),
            }
        }
        for src in &sources {
            if let Err(e) = check(&user, &bot, chat, src, &cfg, rates, &mut st).await {
                eprintln!("@{}: {e:#}", src.name);
            }
        }
        tokio::time::sleep(poll).await;
    }
}

/// Send new matching posts of one source; remember progress after each post.
async fn check(
    user: &userbot::Userbot,
    bot: &tg::Bot,
    chat: i64,
    src: &userbot::Source,
    cfg: &config::Config,
    rates: Option<Rates>,
    st: &mut state::State,
) -> Result<()> {
    let after = st.last_seen.get(&src.name).copied();
    for l in user.fetch(src, after, FIRST_RUN).await? {
        if let Ok(card) = evaluate(&l, cfg, rates, st) {
            deliver(user, bot, chat, &l, &card).await?;
        }
        st.last_seen.insert(src.name.clone(), l.id);
        st.save()?;
    }
    Ok(())
}
