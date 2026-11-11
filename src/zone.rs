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

/// A street counts only where the post gives it as an address: after "вул." / "вулиця" / "просп." /
/// "кут", on an "Адреса:" line (one word may sit in between: "вул. Генерала Чупринки"), or right before a
/// house number ("Шафарика 2"). A bare name is too often something else: "Костел святої Ольги",
/// the agent "Ольга", "Мельник" as a surname, "мила квартира".
fn street(osm: &str) -> Option<Street> {
    let words: Vec<&str> = osm.split_whitespace().collect();
    let name = match words.as_slice() {
        [first, rest @ ..] if *first == "вулиця" || *first == "площа" => rest.to_vec(),
        [rest @ .., last] if *last == "вулиця" => rest.to_vec(),
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
    // Same line only; a house number is 1-3 digits (so "Ольга 0672820705" is a name and a phone).
    let pattern = format!(
        r"(?:\bвул\w*\.?|\bпросп\w*\.?|\bпр\.|\bкут\w*|адрес\w*:[^\n]*?)[^\S\n]*(?:\S+[^\S\n]+)?{s}\w*|\b{s}\w*[^\S\n]*,?[^\S\n]*\d{{1,3}}(?:\D|$)"
    );
    Some(Street { short: name.join(" "), re: Regex::new(&pattern).ok()? })
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
        assert_eq!(place("📍 Вул. Лазаренка (кут Сахарова)"), Place::Street("Академіка Андрія Сахарова".into()));
        assert_eq!(place("Оренда на вул. Коновальця, 22"), Place::Street("Євгена Коновальця".into()));
        assert_eq!(place("ЖК Кінескоп (Вул. Героїв УПА )"), Place::Street("Героїв УПА".into()));
        assert_eq!(place("біля вулиці Грюнвальдської"), Place::Street("Ґрюнвальдська".into()));
        assert_eq!(place("вул. Київська 5"), Place::Street("Київська".into()));
        // Княгині Ольги is out of the zone (removed on request): a named address elsewhere is Outside.
        assert_eq!(place("Адреса: Львів, Франківський район, вулиця Княгині Ольги, 120-а"), Place::Outside);
        assert_eq!(place("Оренда 2к, вул. Княгині Ольги 5а"), Place::Outside);
    }

    #[test]
    fn ordinary_words_are_not_streets() {
        assert_eq!(place("Мила затишна квартира, глибока шафа, київська область"), Place::Outside);
        assert_eq!(place("Бажаю здоров'я🤝 Не полишаю спроб знайти у Львові житло."), Place::Outside);
        // Real false matches: the flat is on another street, "Ольга" is a church / the agent.
        assert_eq!(place("Оренда | вул. Хотинська, 7\n📍Район Костелу святої Ольги та Єлизавети"), Place::Outside);
        assert_eq!(place("Оренда дворівневої квартири вул. Жовківська ( ближній центр)\nОльга\nПропозиція від АН"), Place::Outside);
        assert_eq!(place("Ціна 18000\nМельник Ірина, Петро Антонович"), Place::Outside);
        assert_eq!(place("📍 вул. Хотинська\nОльга 0672820705"), Place::Outside);
    }

    #[test]
    fn street_with_a_word_before_it() {
        assert_eq!(place("📍 вул. Генерала Чупринки"), Place::Street("Генерала Чупринки".into()));
        assert_eq!(place("Оренда, вулиця Генерала Чупринки"), Place::Street("Генерала Чупринки".into()));
        assert_eq!(place("2к квартира, Чупринки 59"), Place::Street("Генерала Чупринки".into()));
        assert_eq!(place("Франківський район · вул. Героїв УПА Перша"), Place::Street("Героїв УПА".into()));
        assert_eq!(place("ОРЕНДА 2-КІМНАТНОЇ КВАРТИРИ | вул. Антоновича🏡"), Place::Street("Володимира Антоновича".into()));
        assert_eq!(place("Шафарика 2, поруч Мельника 5"), Place::Street("Андрія Мельника".into()));
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
