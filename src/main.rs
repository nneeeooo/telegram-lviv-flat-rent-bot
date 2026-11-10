//! Lviv flat-rent watcher: reads Telegram channels/bots, filters offers, notifies the configured chats.
//!
//! `telegram-lviv-flat-rent-bot login`                 — one-time interactive login of the user account.
//! `telegram-lviv-flat-rent-bot`                       — watch sources and send matching flats.
//! `telegram-lviv-flat-rent-bot backfill <days> [--send]` — replay recent channel posts (stats; `--send` sends).

mod config;
mod filter;
mod listing;
mod lun;
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
/// LUN is a website: poll it gently.
const LUN_EVERY: Duration = Duration::from_secs(5 * 60);

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} not set"))
}

/// Card for a post that passes the filter and was not sent before.
fn evaluate(l: &Listing, cfg: &config::Config, rates: Option<Rates>, st: &mut state::State) -> Result<String, Skip> {
    let p = l.facts.clone().unwrap_or_else(|| parse::parse(&l.text));
    let place = l.place.clone().unwrap_or_else(|| zone::place(&l.text));
    filter::decide(&l.text, &p, &place, &cfg.filter, rates)?;
    let seen_text = st.seen_before(l.key.as_deref().unwrap_or(&l.text));
    // The same dom.ria / rieltor ad reaches us via @homierentbot and LUN: one link, one card.
    let seen_url = !l.url.starts_with("https://t.me/") && st.seen_before(&l.url);
    if seen_text || seen_url {
        return Err(Skip::Duplicate);
    }
    Ok(l.card(&p, &place, rates))
}

/// Send a card to every chat: with the listing's album photos if it has them (bot sources, LUN), else as
/// text with link preview. A failed photo download or upload falls back to text, so a flat is never lost
/// for a photo. One chat failing (e.g. a recipient blocked the bot) does not stop the others; the call
/// fails only if no chat got the card, so the watcher retries it next poll.
async fn deliver(user: &userbot::Userbot, bot: &tg::Bot, chats: &[i64], l: &Listing, card: &str) -> Result<()> {
    let mut photos = Vec::new();
    for p in &l.photos {
        match p {
            listing::Photo::Url(u) => photos.push(tg::PhotoData::Url(u.clone())),
            listing::Photo::Telegram(p) => match user.download(p).await {
                Ok(b) => photos.push(tg::PhotoData::Bytes(b)),
                Err(e) => eprintln!("photo of {}: {e:#}", l.id),
            },
        }
    }
    let mut last_err = None;
    let mut delivered = 0;
    for &chat in chats {
        let sent = if photos.is_empty() {
            bot.send(chat, card).await
        } else {
            match bot.send_photos(chat, card, &photos).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    eprintln!("album of {} to {chat}: {e:#}", l.id);
                    bot.send(chat, card).await
                }
            }
        };
        match sent {
            Ok(()) => delivered += 1,
            Err(e) => {
                eprintln!("card {} to {chat}: {e:#}", l.id);
                last_err = Some(e);
            }
        }
    }
    match (delivered, last_err) {
        (0, Some(e)) => Err(e),
        _ => Ok(()),
    }
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
    // One or more recipients, comma-separated: "343065165,123456789".
    let chats: Vec<i64> = env("TELEGRAM_CHAT_ID")?
        .split(',')
        .map(|s| s.trim().parse().with_context(|| format!("TELEGRAM_CHAT_ID: {s:?} is not a chat id")))
        .collect::<Result<_>>()?;
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
                    deliver(&user, &bot, &chats, l, card).await?;
                    tokio::time::sleep(Duration::from_millis(1100)).await; // Bot API: ~1 msg/s per chat
                } else {
                    println!("---\n{card}");
                }
            }
        }
        return Ok(());
    }

    // `history <bot username> [n]`: the cards our bot sent you, to state/history.json (for reviewing filter mistakes).
    if args.get(1).map(String::as_str) == Some("history") {
        let name = args.get(2).context("usage: history <bot username> [n]")?;
        let n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(500);
        let src = user.resolve(name, false).await?;
        let cards: Vec<_> = user.newest(&src, n).await?.into_iter().map(|l| (l.id, l.text)).collect();
        state::write_atomic(&state_dir.join("history.json"), &serde_json::to_vec_pretty(&cards)?)?;
        println!("{} cards -> state/history.json", cards.len());
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
            send_cards(&user, &bot, &chats, &cards, send).await?;
        }
        for page in cfg.sources.lun.iter().filter(|p| only.is_none_or(|o| o == "lun" || o == *p)) {
            let all = lun::fetch(&http, page).await?;
            let mut cards: Vec<(&Listing, String)> = all
                .iter()
                .filter_map(|l| {
                    if all_posts {
                        let card = l.card(l.facts.as_ref()?, l.place.as_ref()?, rates);
                        Some((l, format!("🧪 тест, без фільтра\n{card}")))
                    } else {
                        evaluate(l, &cfg, rates, &mut st).ok().map(|c| (l, c))
                    }
                })
                .take(n)
                .collect();
            cards.reverse();
            println!("LUN {page}: {} matches in {} listings (OLX skipped)", cards.len(), all.len());
            send_cards(&user, &bot, &chats, &cards, send).await?;
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

    // Start is logged only: a chat message on every restart is noise.
    println!("started: {} sources, poll every {}s\n{}", sources.len(), poll.as_secs(), cfg.summary());

    let mut lun_at: Option<Instant> = None;
    loop {
        if rates.is_none() || rates_at.elapsed() > RATES_EVERY {
            match Rates::fetch(&http).await {
                Ok(r) => (rates, rates_at) = (Some(r), Instant::now()),
                Err(e) => eprintln!("PrivatBank rates: {e:#}"),
            }
        }
        for src in &sources {
            if let Err(e) = check(&user, &bot, &chats, src, &cfg, rates, &mut st).await {
                eprintln!("@{}: {e:#}", src.name);
            }
        }
        if lun_at.is_none_or(|t| t.elapsed() > LUN_EVERY) {
            lun_at = Some(Instant::now());
            for page in &cfg.sources.lun {
                if let Err(e) = check_lun(&http, &user, &bot, &chats, page, &cfg, rates, &mut st).await {
                    eprintln!("LUN {page}: {e:#}");
                }
            }
        }
        tokio::time::sleep(poll).await;
    }
}

/// Print or send (~1 msg/s, the Bot API limit per chat) a batch of cards.
async fn send_cards(user: &userbot::Userbot, bot: &tg::Bot, chats: &[i64], cards: &[(&Listing, String)], send: bool) -> Result<()> {
    for (l, card) in cards {
        if send {
            deliver(user, bot, chats, l, card).await?;
            tokio::time::sleep(Duration::from_millis(1100)).await;
        } else {
            println!("---\n{card}");
        }
    }
    Ok(())
}

/// New matching listings of one LUN page. LUN has no message order like Telegram, so progress is the
/// sent/seen key list; on the first run only the newest FIRST_RUN matches are sent, the rest marked seen.
#[allow(clippy::too_many_arguments)]
async fn check_lun(
    http: &reqwest::Client,
    user: &userbot::Userbot,
    bot: &tg::Bot,
    chats: &[i64],
    page: &str,
    cfg: &config::Config,
    rates: Option<Rates>,
    st: &mut state::State,
) -> Result<()> {
    let marker = format!("lun:{page}");
    let first_run = !st.last_seen.contains_key(&marker);
    let mut sent = 0;
    // Oldest first, so the chat reads in posting order.
    for l in lun::fetch(http, page).await?.iter().rev() {
        if let Ok(card) = evaluate(l, cfg, rates, st) {
            if !first_run || sent < FIRST_RUN {
                deliver(user, bot, chats, l, &card).await?;
                sent += 1;
            }
        }
    }
    st.last_seen.insert(marker, 1);
    st.save()
}

/// Send new matching posts of one source; remember progress after each post.
async fn check(
    user: &userbot::Userbot,
    bot: &tg::Bot,
    chats: &[i64],
    src: &userbot::Source,
    cfg: &config::Config,
    rates: Option<Rates>,
    st: &mut state::State,
) -> Result<()> {
    let after = st.last_seen.get(&src.name).copied();
    for l in user.fetch(src, after, FIRST_RUN).await? {
        if let Ok(card) = evaluate(&l, cfg, rates, st) {
            deliver(user, bot, chats, &l, &card).await?;
        }
        st.last_seen.insert(src.name.clone(), l.id);
        st.save()?;
    }
    Ok(())
}
