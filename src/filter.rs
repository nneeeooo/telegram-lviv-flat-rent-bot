//! Which posts reach the chat: rules from config.toml applied to the parsed post.
//! Missing data never rejects a post; it is flagged in the message instead.

use crate::config::Filter;
use crate::parse::Parsed;
use crate::rates::Rates;
use crate::zone::Place;
use regex::Regex;
use std::sync::LazyLock;

/// "Шукаю квартиру" posts: people looking for a flat, not offering one.
static SEEKING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bшука(ю|ємо|є)\b|знайти\b[^.\n]{0,25}(житло|квартир)|\b(у|в) бюджеті").unwrap()
});
/// Landlords also write "шукаємо орендаря / квартирантів"; those are offers.
static WANTS_TENANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(орендар|квартирант|мешканц|наймач)").unwrap());

pub fn is_seeker(text: &str) -> bool {
    SEEKING.is_match(text) && !WANTS_TENANT.is_match(text)
}

/// Why a post was dropped (also used for backfill stats).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Skip {
    Empty,
    Seeker,
    Rented,
    NotFlat,
    Outside,
    Rooms,
    Price,
    Floor,
    TopFloor,
    Duplicate,
}

pub fn decide(text: &str, p: &Parsed, place: &Place, f: &Filter, rates: Option<Rates>) -> Result<(), Skip> {
    if text.trim().is_empty() {
        return Err(Skip::Empty);
    }
    if is_seeker(text) {
        return Err(Skip::Seeker);
    }
    if p.rented {
        return Err(Skip::Rented);
    }
    if p.not_flat {
        return Err(Skip::NotFlat);
    }
    if *place == Place::Outside {
        return Err(Skip::Outside);
    }
    if p.rooms.is_some_and(|r| !f.rooms.contains(&r)) {
        return Err(Skip::Rooms);
    }
    if let (Some(price), Some(rates)) = (p.price, rates) {
        if rates.to_uah(price) > f.max_price_uah {
            return Err(Skip::Price);
        }
    }
    if let Some((floor, total)) = p.floor {
        if floor > f.max_floor {
            return Err(Skip::Floor);
        }
        if total == Some(floor) {
            return Err(Skip::TopFloor);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::zone::place;

    fn cfg() -> Filter {
        Filter { max_price_uah: 25000, rooms: vec![1, 2], max_floor: 5 }
    }
    const RATES: Option<Rates> = Some(Rates { usd: 45.0, eur: 50.0 });

    fn check(text: &str) -> Result<(), Skip> {
        decide(text, &parse(text), &place(text), &cfg(), RATES)
    }

    #[test]
    fn seekers_are_dropped() {
        assert!(is_seeker("Шукаю 1к квартиру на Франківському до 15000"));
        assert!(is_seeker("Шукаємо двокімнатну, без тварин"));
        assert!(is_seeker("Дівчина шукає квартиру"));
        assert!(is_seeker("Не полишаю спроб знайти у Львові житло. В бюджеті орієнтовно 500$"));
        assert!(!is_seeker("Шукаємо охайних орендарів у 1к на Героїв УПА"));
        assert!(!is_seeker("Шукаю квартиранта, Київська 5"));
        assert!(!is_seeker("Пошукайте краще"));
    }

    #[test]
    fn rules() {
        assert_eq!(check("Оренда 2к, вул. Коновальця, 3/9 поверх, 20000 грн"), Ok(()));
        assert_eq!(check("Оренда 2к, вул. Коновальця, 3/9 поверх, 600$"), Err(Skip::Price)); // 27 000 грн
        assert_eq!(check("Оренда 2к, вул. Коновальця, 3/9 поверх, 500$"), Ok(())); // 22 500 грн
        assert_eq!(check("Оренда 3к, вул. Коновальця, 3/9 поверх, 20000 грн"), Err(Skip::Rooms));
        assert_eq!(check("Оренда 2к, вул. Коновальця, 7/9 поверх, 20000 грн"), Err(Skip::Floor));
        assert_eq!(check("Оренда 2к, вул. Коновальця, 5/5 поверх, 20000 грн"), Err(Skip::TopFloor));
        assert_eq!(check("Оренда 2к, вул. Замарстинівська, 2/9 поверх, 20000 грн"), Err(Skip::Outside));
        assert_eq!(check("Оренда 2к, Франківський район, 20000 грн"), Ok(()));
        assert_eq!(check("Оренда 2к, вул. Коновальця"), Ok(())); // missing data is flagged, not dropped
        assert_eq!(check("Оренда 2к, вул. Коновальця, 20000 грн. Здано✅"), Err(Skip::Rented));
    }
}
