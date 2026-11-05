//! Where the flat is: a street of the zone (data/streets.json, from OpenStreetMap), only the
//! Франківський district, or somewhere else.

use crate::parse::normalize;
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// Street of the zone, short name for display ("Коновальця").
    Street(String),
    /// No zone street, no other address, but the post says Франківський district.
    DistrictOnly,
    /// An address outside the zone, or no location at all.
    Outside,
}

#[derive(Deserialize)]
struct Streets {
    listed: Vec<String>,
    adjacent: Vec<String>,
    /// Zone outline, [lon, lat] points.
    hull: Vec<[f64; 2]>,
}

struct Street {
    short: String,
    re: Regex,
}

static DATA: LazyLock<Streets> =
    LazyLock::new(|| serde_json::from_str(include_str!("../data/streets.json")).expect("data/streets.json"));

/// Matchers built once from the OSM names.
static STREETS: LazyLock<Vec<Street>> =
    LazyLock::new(|| DATA.listed.iter().chain(&DATA.adjacent).filter_map(|n| street(n)).collect());

/// Point inside the zone outline (ray casting).
pub fn in_zone(lat: f64, lon: f64) -> bool {
    let h = &DATA.hull;
    let mut inside = false;
    let mut j = h.len() - 1;
    for i in 0..h.len() {
        let ([xi, yi], [xj, yj]) = (h[i], h[j]);
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// "вулиця Євгена Коновальця" -> plain match on "коновальц…".
/// "Київська вулиця" (adjective, also an ordinary word) -> only after "вул." / before a house number.
fn street(osm: &str) -> Option<Street> {
    let words: Vec<&str> = osm.split_whitespace().collect();
    let (adjective, name) = match words.as_slice() {
        [first, rest @ ..] if *first == "вулиця" || *first == "площа" => (false, rest.to_vec()),
        [rest @ .., last] if *last == "вулиця" => (true, rest.to_vec()),
        _ => return None,
    };
    // Last word identifies the street; too short to be distinctive -> keep the last two.
    let key = match name.as_slice() {
        [.., a, b] if b.chars().count() < 5 => format!("{a} {b}"),
        [.., b] => b.to_string(),
        [] => return None,
    };
    let mut stem = normalize(&key);
    if stem.ends_with(['а', 'я', 'і', 'и', 'ї', 'о', 'е', 'у', 'ю']) {
        stem.pop();
    }
    if stem.chars().count() < 4 {
        return None;
    }
    let s = regex::escape(&stem);
    // Names that are also ordinary words ("бажаю здоров'я") need an address context too.
    let common = ["Здоровʼя", "Воля", "Мила"].contains(&key.as_str());
    let pattern = if adjective || common || name.len() == 1 && key.chars().count() < 7 {
        format!(r"(?:вул\w*\.?|просп\w*\.?|адрес\w*:[^\n]*)\s*{s}\w*|\b{s}\w*,?\s*\d")
    } else {
        format!(r"\b{s}\w*")
    };
    Some(Street { short: key, re: Regex::new(&pattern).ok()? })
}

static DISTRICT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"франківськ\w*\s+(?:р-н|район|р\.)|р-н\s+франківськ").unwrap());
/// The post names some address (that is not ours).
static ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bвул\w*\.?|\bпросп\w*|\bпр\.|адрес\w*:|📍").unwrap());

pub fn place(text: &str) -> Place {
    let t = normalize(text);
    if let Some(s) = STREETS.iter().find(|s| s.re.is_match(&t)) {
        return Place::Street(s.short.clone());
    }
    // "Адреса: Львів, Франківський район, Лазаренка" names a street, but it is not in the zone.
    if DISTRICT.is_match(&t) && !ADDRESS.is_match(&t) {
        return Place::DistrictOnly;
    }
    Place::Outside
}

#[cfg(test)]
mod tests {
    use super::{Place, place};

    #[test]
    fn zone_streets() {
        assert_eq!(place("📍 Вул. Лазаренка (кут Сахарова)"), Place::Street("Сахарова".into()));
        assert_eq!(place("Оренда на вул. Коновальця, 22"), Place::Street("Коновальця".into()));
        assert_eq!(place("ЖК Кінескоп (Вул. Героїв УПА )"), Place::Street("Героїв УПА".into()));
        assert_eq!(place("біля вулиці Грюнвальдської"), Place::Street("Ґрюнвальдська".into()));
        assert_eq!(place("вул. Київська 5"), Place::Street("Київська".into()));
        assert_eq!(place("Адреса: Львів, Франківський район, вулиця Княгині Ольги, 120-а"), Place::Street("Ольги".into()));
    }

    #[test]
    fn ordinary_words_are_not_streets() {
        assert_eq!(place("Мила затишна квартира, глибока шафа, київська область"), Place::Outside);
        assert_eq!(place("Бажаю здоров'я🤝 Не полишаю спроб знайти у Львові житло."), Place::Outside);
    }

    #[test]
    fn coordinates() {
        assert!(super::in_zone(49.8300, 24.0040)); // Мельника / Єфремова
        assert!(!super::in_zone(49.7983, 24.0107)); // Трускавецька, Франківський but south of the zone
        assert!(!super::in_zone(49.8420, 24.0310)); // city centre
    }

    #[test]
    fn district_only_and_outside() {
        assert_eq!(place("2к квартира, Франківський район, 20000 грн"), Place::DistrictOnly);
        assert_eq!(place("Адреса: Львів, Франківський район, Лазаренка"), Place::Outside);
        assert_eq!(place("📍 вул. Замарстинівська, 170"), Place::Outside);
        assert_eq!(place("квартира в центрі"), Place::Outside);
    }
}
