//! LUN (lun.ua) listing pages. No API: the server-rendered page embeds its cards as JSON in the
//! Next.js payload (`self.__next_f.push([1,"…"])`), with price, rooms, floor, heating, coordinates,
//! phones, photos and the link to the source site. robots.txt allows these paths (no `?` query).
//!
//! OLX listings are skipped: @homierentbot already covers OLX.

use crate::listing::{Listing, Photo};
use crate::parse::{Currency, Heating, Parsed, Price, Seller};
use crate::zone::{self, Place};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

/// Photos per flat, as for bot sources.
const MAX_PHOTOS: usize = 5;
const IMAGE_BASE: &str = "https://market-images.lunstatic.net/lun-ua/1200/1200/images/";

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Card {
    id: i64,
    /// String or number, depending on the card.
    group_id: Option<serde_json::Value>,
    url_raw: Option<String>,
    price: Option<f64>,
    currency: Option<String>,
    room_count: Option<u8>,
    area_total: Option<f32>,
    floor: Option<u8>,
    floor_count: Option<u8>,
    heating_system_name: Option<String>,
    is_owner: Option<bool>,
    header: Option<String>,
    text: Option<String>,
    site: Option<Site>,
    /// [lon, lat]
    location: Option<Vec<f64>>,
    /// Both can be null.
    images: Option<Vec<Image>>,
    phones: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Site {
    internal_name: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Image {
    /// "offers/123" for LUN-hosted photos; a number for OLX ones (another host, and OLX is skipped).
    image_id: serde_json::Value,
}

static PUSH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"self\.__next_f\.push\(\[1,("(?:[^"\\]|\\.)*")\]\)"#).unwrap());

/// Fetch one LUN listing page; newest cards first, OLX ones dropped.
pub async fn fetch(http: &reqwest::Client, page: &str) -> Result<Vec<Listing>> {
    let html = http
        .get(page)
        .header("User-Agent", "telegram-lviv-flat-rent-bot (personal, polls every 5 min)")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let mut cards = cards(&html)?;
    cards.sort_by_key(|c| std::cmp::Reverse(c.id));
    Ok(cards.into_iter().filter(|c| !is_olx(c)).map(listing).collect())
}

fn is_olx(c: &Card) -> bool {
    c.site.as_ref().is_some_and(|s| s.internal_name.contains("olx"))
        || c.url_raw.as_deref().is_some_and(|u| u.contains("olx."))
}

/// The page's `realties.cards` array.
fn cards(html: &str) -> Result<Vec<Card>> {
    let mut payload = String::new();
    for c in PUSH.captures_iter(html) {
        payload.push_str(&serde_json::from_str::<String>(&c[1])?);
    }
    let at = payload.find(r#""realties":{"cards":"#).context("LUN: no cards on page (layout changed?)")?;
    let start = at + payload[at..].find('[').context("LUN: cards array")?;
    // Parse just the first JSON value after `"cards":` and ignore the rest of the payload.
    let mut de = serde_json::Deserializer::from_str(&payload[start..]).into_iter::<Vec<serde_json::Value>>();
    let raw = de.next().context("LUN: empty cards")?.context("LUN: cards JSON")?;
    // One odd card must not hide the others.
    Ok(raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).inspect_err(|e| eprintln!("LUN card skipped: {e}")).ok())
        .collect())
}

fn listing(c: Card) -> Listing {
    let currency = match c.currency.as_deref() {
        Some("usd") => Some(Currency::Usd),
        Some("eur") => Some(Currency::Eur),
        Some("uah") => Some(Currency::Uah),
        _ => None,
    };
    let facts = Parsed {
        price: c.price.zip(currency).map(|(p, currency)| Price { amount: p.round() as u32, currency }),
        rooms: c.room_count,
        studio: false,
        floor: c.floor.filter(|f| *f >= 1).map(|f| (f, c.floor_count)),
        area: c.area_total,
        heating: match c.heating_system_name.as_deref() {
            Some(h) if h.contains("individual") || h.contains("autonom") => Heating::Individual,
            Some(h) if h.contains("central") => Heating::Central,
            _ => Heating::Unknown,
        },
        seller: match c.is_owner {
            Some(true) => Seller::Owner,
            Some(false) => Seller::Agency,
            None => Seller::Unknown,
        },
        not_flat: false,
        rented: false,
    };
    let header = c.header.clone().unwrap_or_default();
    let street = header.split(',').next().unwrap_or_default().trim().to_string();
    let in_zone = c.location.as_deref().is_some_and(|l| l.len() == 2 && zone::in_zone(l[1], l[0]));
    let place = match zone::place(&format!("вул. {header}")) {
        Place::Street(s) => Place::Street(s),
        _ if in_zone => Place::Street(street),
        _ => Place::Outside,
    };
    Listing {
        id: c.id,
        url: c.url_raw.unwrap_or_else(|| "https://lun.ua/".into()),
        text: c.text.unwrap_or_default(),
        photos: c
            .images
            .unwrap_or_default()
            .iter()
            .filter_map(|i| i.image_id.as_str())
            .take(MAX_PHOTOS)
            .map(|id| Photo::Url(format!("{IMAGE_BASE}{id}.jpg")))
            .collect(),
        facts: Some(facts),
        place: Some(place),
        phone: c.phones.unwrap_or_default().first().map(|p| format!("+{}", p.trim_start_matches('+'))),
        // The same flat on several sites shares a group.
        source: "🦊 LUN",
        key: Some(format!("lun:{}", c.group_id.map(|g| g.to_string().trim_matches('"').to_string()).unwrap_or_else(|| c.id.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal page in LUN's real shape (escaped JSON inside a script push).
    fn page(cards: &str) -> String {
        let payload = format!(r#"31:["$","$L32",null,{{"realties":{{"cards":{cards},"nearbyCards":[]}}}}]"#);
        format!("<script>self.__next_f.push([1,{}])</script>", serde_json::to_string(&payload).unwrap())
    }

    #[test]
    fn parses_cards_and_skips_olx() {
        let html = page(
            r#"[{"id":4731263865,"groupId":"g1","urlRaw":"https://rieltor.ua/lvov/flats-rent/view/13195052/",
                "price":500,"currency":"usd","roomCount":2,"areaTotal":73,"floor":3,"floorCount":15,
                "heatingSystemName":"individual","isOwner":false,"header":"Мельника вул., 5",
                "text":"Здається квартира","site":{"internalName":"rieltor.ua"},"location":[24.004,49.830],
                "images":[{"imageId":"offers/1"},{"imageId":"offers/2"}],"phones":["380979072029"]},
               {"id":2,"urlRaw":"https://www.olx.ua/d/x.html","site":{"internalName":"olx.ua"}}]"#,
        );
        let cards = cards(&html).unwrap();
        assert_eq!(cards.len(), 2);
        let l: Vec<Listing> = cards.into_iter().filter(|c| !is_olx(c)).map(listing).collect();
        assert_eq!(l.len(), 1);
        let l = &l[0];
        let f = l.facts.as_ref().unwrap();
        assert_eq!(f.price, Some(Price { amount: 500, currency: Currency::Usd }));
        assert_eq!((f.rooms, f.floor, f.heating), (Some(2), Some((3, Some(15))), Heating::Individual));
        assert_eq!(l.place, Some(Place::Street("Мельника".into())));
        assert_eq!(l.phone.as_deref(), Some("+380979072029"));
        assert_eq!(l.url, "https://rieltor.ua/lvov/flats-rent/view/13195052/");
        assert!(matches!(&l.photos[0], Photo::Url(u) if u.ends_with("images/offers/1.jpg")));
        assert_eq!(l.key.as_deref(), Some("lun:g1"));
    }
}
