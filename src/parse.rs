//! Free-form Ukrainian rent post -> structured fields. Heuristics tuned on real @orendakvartyr_ua posts;
//! every field is optional because posts often omit it.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Currency {
    Uah,
    Usd,
    Eur,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub amount: u32,
    pub currency: Currency,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Heating {
    Individual,
    Central,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seller {
    Owner,
    Agency,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub price: Option<Price>,
    pub rooms: Option<u8>,
    pub studio: bool,
    /// (floor, floors in building)
    pub floor: Option<(u8, Option<u8>)>,
    pub area: Option<f32>,
    pub heating: Heating,
    pub seller: Seller,
    /// Post is a house, a part of a house or commercial space, not a flat.
    pub not_flat: bool,
    /// Already rented ("здано", "стоп 🛑").
    pub rented: bool,
}

/// Lowercase, `ґ`->`г`, all apostrophes -> `'`, Latin look-alikes in common words fixed.
pub fn normalize(text: &str) -> String {
    text.to_lowercase()
        .replace('ґ', "г")
        .replace(['ʼ', '’', '`', '´'], "'")
        .replace('\u{a0}', " ")
        // Emoji variation selector / joiner count as word chars and glue "✔️2 поверх" together.
        .replace(['\u{fe0f}', '\u{200d}'], "")
}

fn re(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

static PRICE_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"ціна|цін[аи]|вартіст|💰|💵|💸|оренд"));
static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?x)
        (?:(?P<pre>[$€])\s*(?P<n0>\d{1,3}(?:[\s.,]\d{3})+|\d+))
      | (?:(?P<n>\d{1,3}(?:[\s.,]\d{3})+|\d+)\s*(?P<k>тис\.?|к\b)?\s*
           (?P<cur>грн|гривен|гривн|uah|₴|\$|дол|usd|у\.\s?о|€|євро|eur))")
});

pub fn price(t: &str) -> Option<Price> {
    let grab = |s: &str| -> Option<Price> {
        for c in PRICE.captures_iter(s) {
            let (num, cur, k) = match c.name("pre") {
                Some(p) => (c.name("n0")?.as_str(), p.as_str(), false),
                None => (c.name("n")?.as_str(), c.name("cur")?.as_str(), c.name("k").is_some()),
            };
            let mut amount: u32 = num.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok()?;
            if k {
                amount *= 1000;
            }
            let currency = match cur {
                c if c.starts_with('$') || c.starts_with("дол") || c == "usd" || c.starts_with("у.") => Currency::Usd,
                c if c == "€" || c == "євро" || c == "eur" => Currency::Eur,
                _ => Currency::Uah,
            };
            // Skip deposits, parking, utility amounts etc. that are implausible as monthly rent.
            let ok = match currency {
                Currency::Uah => (3_000..=300_000).contains(&amount),
                _ => (100..=10_000).contains(&amount),
            };
            if ok {
                return Some(Price { amount, currency });
            }
        }
        None
    };
    t.lines()
        .filter(|l| PRICE_LINE.is_match(l))
        .find_map(grab)
        .or_else(|| grab(t))
        .or_else(|| {
            // "Ціна 18000+кп": no currency written means hryvnias.
            let n = BARE_PRICE.captures(t)?.get(1)?.as_str();
            let amount: u32 = n.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok()?;
            (3_000..=300_000).contains(&amount).then_some(Price { amount, currency: Currency::Uah })
        })
}

static BARE_PRICE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:ціна|вартість)\w*\s*[:—–-]?\s*(\d{1,3}(?:[\s.]\d{3})+|\d{4,6})\b"));

static ROOMS: LazyLock<Vec<(Regex, Option<u8>)>> = LazyLock::new(|| {
    vec![
        (re(r"кімнат\w*\s*[:—-]\s*(\d)"), None),
        // 2-кімн, 2-х кімнатна, 2-ох кімнатна, 1-но кімнатної, 1но, 1 кім.кв.
        (re(r"\b(\d)\s*-?\s*(?:о?х|но|ти)?\s*-?\s*кім"), None),
        // 3 ізольовані кімнати
        (re(r"\b(\d)\s+(?:\w+\s+)?кімнат"), None),
        (re(r"\b(\d)\s*-?\s*к\b"), None),
        // 1км квартира (slang, glued; "5 км від центру" has no "квартир" after it)
        (re(r"\b([1-4])\s*-?км\.?\s+кварт"), None),
        (re(r"одно\s*-?\s*кім|1\s*-?\s*дно\s*-?\s*кім"), Some(1)),
        (re(r"дво\s*-?\s*кім|двох\s*-?\s*кім|\b(?:дві|два)\s+(?:\w+\s+)?кімнат"), Some(2)),
        (re(r"три\s*-?\s*кім|трьох\s*-?\s*кім|\bтри\s+(?:\w+\s+)?кімнат"), Some(3)),
        (re(r"чотири\s*-?\s*кім|чотирьох\s*-?\s*кім"), Some(4)),
    ]
});
static STUDIO: LazyLock<Regex> = LazyLock::new(|| re(r"студі[яюї]|студійн|smart|смарт"));
static KITCHEN_STUDIO: LazyLock<Regex> = LazyLock::new(|| re(r"кухн\w*\s*-?\s*студі\w*"));
/// Bedrooms, when the post never says "N-кімнатна": "окрема спальня" + kitchen-living = 1 room
/// (Lviv counts bedrooms, not the kitchen-living room).
static BEDROOMS: LazyLock<Vec<(Regex, u8)>> = LazyLock::new(|| {
    vec![
        (re(r"\b(?:3|три)\s+(?:\w+\s+)?спальн"), 3),
        (re(r"\b(?:2|дві)\s+(?:\w+\s+)?спальн"), 2),
        (re(r"(?:окрем\w*|ізольован\w*|відокремлен\w*)\s+спальн|спальн\w*\s+(?:кімнат\w*\s+)?(?:та|і|\+)\s+кухн"), 1),
    ]
});

/// Rooms from the earliest mention; studio = 1 room; else counted bedrooms.
pub fn rooms(t: &str) -> (Option<u8>, bool) {
    let found = ROOMS
        .iter()
        .filter_map(|(r, fixed)| {
            let c = r.captures(t)?;
            let n = match fixed {
                Some(n) => *n,
                None => c.get(1)?.as_str().parse().ok()?,
            };
            Some((c.get(0)?.start(), n))
        })
        .filter(|(_, n)| (1..=9).contains(n))
        .min_by_key(|(pos, _)| *pos)
        .map(|(_, n)| n);
    // "кухня-студія" is a kitchen-living room, not a studio flat.
    let studio = found.is_none() && STUDIO.is_match(&KITCHEN_STUDIO.replace_all(t, ""));
    let bedrooms = || BEDROOMS.iter().find(|(r, _)| r.is_match(t)).map(|(_, n)| *n);
    (found.or(studio.then_some(1)).or_else(bedrooms), studio)
}

/// "поверх" as a noun only: not "поверховий" / "поверхів" / "поверхня" (a 9-storey building is not the
/// 9th floor; a cooking surface is not a floor). "повер" is a cut-off spelling ("2/5 повер буд").
const FL: &str = r"(?:поверх(?:і|у|ом)?\b|поверсі|повер\b|пов\b\.?)";

static FLOOR: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // 6/9 поверх, 3 з 3 поверх
        r"\b(\d{1,2})\s*(?:/|\\|з|із|зі)\s*(\d{1,2})\s*-?\s*(?:й|ий)?\s*FL",
        // поверх: 3/12, поверх 3 із 4
        r"FL\s*[:—–-]?\s*(\d{1,2})\s*-?\s*(?:й|ий)?\s*(?:/|\\|з|із|зі)\s*(\d{1,2})",
        // 7-й поверх із 10, 3 поверх з 3
        r"\b(\d{1,2})\s*-?\s*(?:й|ий|му|ому)?\s*FL\s*(?:з|із|зі)\s*(\d{1,2})",
        // realtor shorthand, wall type after the total: 4/5ц, 7/16ц, 12/12 ц., 2п./3ц., 14п/15, 4/9п
        r"\b(\d{1,2})\s*п?\.?\s*/\s*(\d{1,2})\s*(?:ц|п|б|к|м)\b",
        r"\b(\d{1,2})\s*п\.?\s*/\s*(\d{1,2})\b",
        // поверх: 5, поверх — 4-й
        r"FL\s*[:—–-]?\s*(\d{1,2})\b",
        // 5 поверсі, 8-й поверх
        r"\b(\d{1,2})\s*-?\s*(?:й|ий|му|ому)?\s*FL",
        // a bare "2/4" or "🏗️ 3/3" alone on its line (lists of features in many posts)
        r"(?m)^[^\w\n]*(\d{1,2})\s*/\s*(\d{1,2})[^\w\n]*(?:\([^)\n]*\))?[^\w\n]*$",
    ]
    .iter()
    .map(|p| re(&p.replace("FL", FL)))
    .collect()
});

/// "третій поверх", "на другому поверсі".
static FLOOR_WORD: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"\b(перш|друг|трет|четверт|п'ят|шост|сьом|восьм|дев'ят|десят)\w*\s+{FL}"
    ))
});

pub fn floor(t: &str) -> Option<(u8, Option<u8>)> {
    let ok = |f: u8, total: Option<u8>| f >= 1 && f <= 40 && total.is_none_or(|n| n >= f && n <= 40);
    FLOOR
        .iter()
        .find_map(|r| {
            let c = r.captures(t)?;
            let f: u8 = c.get(1)?.as_str().parse().ok()?;
            let total: Option<u8> = c.get(2).and_then(|m| m.as_str().parse().ok());
            ok(f, total).then_some((f, total))
        })
        .or_else(|| {
            let w = FLOOR_WORD.captures(t)?.get(1)?.as_str();
            let f = ["перш", "друг", "трет", "четверт", "п'ят", "шост", "сьом", "восьм", "дев'ят", "десят"]
                .iter()
                .position(|s| *s == w)? as u8
                + 1;
            Some((f, None))
        })
}

/// Total area. Kitchen sizes ("кухня 15 м²") are skipped; "Площа: 80м" counts even without "²".
static AREA: LazyLock<Regex> = LazyLock::new(|| {
    // Bare "40 м" / "45 метрів" only with 2 digits and a list-like end: "300 м до зупинки" is a distance.
    re(r"(?m)(\d{2,3}(?:[.,]\d)?)\s*(?:м²|м2|м\.?\s*кв|кв\.?\s*\\?\s*м|квм|кв2|кв\.? метр)|\b(\d{2})\s*(?:м|метр\w*)\b[^\S\n]*(?:на\b|,|\(|$)")
});
static AREA_LABEL: LazyLock<Regex> =
    LazyLock::new(|| re(r"площ\w*[^\d\n]{0,15}(\d{2,3}(?:[.,]\d)?)(?:\D|$)"));
static KITCHEN: LazyLock<Regex> = LazyLock::new(|| re(r"кухн\w*[^\d\n]{0,12}$"));

pub fn area(t: &str) -> Option<f32> {
    let num = |s: &str| s.replace(',', ".").parse::<f32>().ok().filter(|a| (12.0..=400.0).contains(a));
    if let Some(a) = AREA_LABEL.captures(t).and_then(|c| num(c.get(1)?.as_str())) {
        return Some(a);
    }
    AREA.captures_iter(t).find_map(|c| {
        let m = c.get(1).or_else(|| c.get(2))?;
        // "Кухня: 15 м²" is not the flat's area.
        let before = &t[..m.start()];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        (!KITCHEN.is_match(&before[line_start..])).then(|| num(m.as_str())).flatten()
    })
}

static INDIVIDUAL: LazyLock<Regex> = LazyLock::new(|| {
    re(r"інд[иу]відуальн\w*(?:\s+\S+){0,2}\s+опален|опален\w*\s*[:—–-]?\s*(?:інд[иу]відуальн|газов|електр|конвектор|пічн|автономн)|котел\b|котла\b|власн\w* котельн|автономн\w* опален|електроопален|електричн\w* опален|газов\w*\s+опален|пічн\w*\s+(?:\S+\s+)?опален|конвектор")
});
static CENTRAL: LazyLock<Regex> = LazyLock::new(|| {
    re(r"централ\w*\s+опален|опален\w*\s*[:—–-]?\s*(?:центральн|централізован|будинков|від будинков)|будинков\w* котельн|будинков\w*\s+опален|дахов\w*\s+(?:газов\w*\s+)?котельн|від центрального")
});

pub fn heating(t: &str) -> Heating {
    if INDIVIDUAL.is_match(t) {
        Heating::Individual
    } else if CENTRAL.is_match(t) {
        Heating::Central
    } else {
        Heating::Unknown
    }
}

static OWNER: LazyLock<Regex> = LazyLock::new(|| re(r"вла[cс]ник|без коміс"));
static AGENCY: LazyLock<Regex> = LazyLock::new(|| re(r"від ан\b|\bан\b|ріелт|рієлт|агенц|\bфоп\b|коміс"));

pub fn seller(t: &str) -> Seller {
    if OWNER.is_match(t) {
        Seller::Owner
    } else if AGENCY.is_match(t) {
        Seller::Agency
    } else {
        Seller::Unknown
    }
}

/// Ukrainian phone in any common spelling: 0985211717, 097-13-12-911, +380 63 133 8828, (067) 310-23-32.
/// Not glued to a preceding digit, so "15000" never starts a number.
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:^|[^\d+])((?:\+\s*)?(?:3\s*8\s*)?\(?0(?:[\s()-]*\d){9})"));

/// First full phone number of the post as `+380XXXXXXXXX`. Masked numbers ("0987049***") are skipped.
pub fn phone(text: &str) -> Option<String> {
    PHONE.captures_iter(text).find_map(|c| {
        let m = c.get(1)?;
        if text[m.end()..].starts_with('*') {
            return None;
        }
        let digits: String = m.as_str().chars().filter(char::is_ascii_digit).collect();
        let local = digits.strip_prefix("38").unwrap_or(&digits);
        (local.len() == 10).then(|| format!("+38{local}"))
    })
}

static NOT_FLAT: LazyLock<Regex> = LazyLock::new(|| re(r"будин|котедж|приміщен|магазин|офіс|склад|комерці|\bмаф|гараж|\bсто\b|будівл|\bбар(?:у|\b)|кафе|ресторан|кав'ярн|салон"));
static RENTED: LazyLock<Regex> = LazyLock::new(|| re(r"\bздано|стоп\s*🛑|🛑"));

/// A room in a shared flat ("оренда кімнати", "підселення"), not a flat.
static ROOM_SHARE: LazyLock<Regex> =
    LazyLock::new(|| re(r"оренд\w*\s+кімнати|здається\s+(?:\w+\s+)?кімната|підселенн"));

/// Head of the post (first ~150 chars) says what is rented.
fn not_flat(t: &str) -> bool {
    let head: String = t.chars().take(150).collect();
    (NOT_FLAT.is_match(&head) && !head.contains("квартир")) || ROOM_SHARE.is_match(&head)
}

pub fn parse(text: &str) -> Parsed {
    let t = normalize(text);
    let (rooms, studio) = rooms(&t);
    Parsed {
        price: price(&t),
        rooms,
        studio,
        floor: floor(&t),
        area: area(&t),
        heating: heating(&t),
        seller: seller(&t),
        not_flat: not_flat(&t),
        rented: RENTED.is_match(&t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Parsed {
        parse(s)
    }

    #[test]
    fn prices() {
        let uah = |n| Some(Price { amount: n, currency: Currency::Uah });
        let usd = |n| Some(Price { amount: n, currency: Currency::Usd });
        assert_eq!(p("💵 Ціна: 500 $/місяць").price, usd(500));
        assert_eq!(p("Ціна 15000 грн").price, uah(15000));
        assert_eq!(p("- Вартість: 25000 гривень").price, uah(25000));
        assert_eq!(p("💰 Ціна: 25 000 грн + КП").price, uah(25000));
        assert_eq!(p("Вартість оренди становить 24тис.грн + КП").price, uah(24000));
        assert_eq!(p("Ціна 750 дол +КП").price, usd(750));
        assert_eq!(p("- Вартість: 900 доларів").price, usd(900));
        assert_eq!(p("Ціна:700$+КП,ріелтор").price, usd(700));
        assert_eq!(p("Площа 80 м² | 3 поверх\n💵 Ціна: 2300$ ✅\nпаркомісце (200$)").price, usd(2300));
        assert_eq!(p("- Вартість: 1000 євро").price.map(|p| p.currency), Some(Currency::Eur));
        assert_eq!(p("Ціна 18000+кп").price, uah(18000));
        assert_eq!(p("квартира біля парку").price, None);
    }

    #[test]
    fn rooms_and_studio() {
        assert_eq!(p("Оренда 2-кімн біля парку").rooms, Some(2));
        assert_eq!(p("Оренда 2-х кімнатної квартири").rooms, Some(2));
        assert_eq!(p("ОРЕНДА 1 кімнатної квартири").rooms, Some(1));
        assert_eq!(p("Оренда 1дно кімнатної квартири").rooms, Some(1));
        assert_eq!(p("однокімнатну квартиру + кухня студія").rooms, Some(1));
        assert_eq!(p("Двокімнатна квартира з просторою кухнею").rooms, Some(2));
        assert_eq!(p("Дві ізольовані кімнати").rooms, Some(2));
        assert_eq!(p("Кімнат: 1").rooms, Some(1));
        assert_eq!(p("2кім квартири + кухня-студія").rooms, Some(2));
        let s = p("сучасна квартира студійного типу, студія 35 м²");
        assert_eq!((s.rooms, s.studio), (Some(1), true));
        assert!(!p("2-кімнатна, кухня-студія").studio);
    }

    #[test]
    fn floors() {
        assert_eq!(p("6/9 поверх 47 кв м").floor, Some((6, Some(9))));
        assert_eq!(p("Поверх: 3/12").floor, Some((3, Some(12))));
        assert_eq!(p("- 3 поверх з 3;").floor, Some((3, Some(3))));
        assert_eq!(p("📐 43,1 м² | 7-й поверх із 10").floor, Some((7, Some(10))));
        assert_eq!(p("Поверх: 3 із 4 🏢").floor, Some((3, Some(4))));
        assert_eq!(p("〰️поверх 5/16").floor, Some((5, Some(16))));
        assert_eq!(p("🏢 Поверх: 5").floor, Some((5, None)));
        assert_eq!(p("квартира площею 45 м² на 5 поверсі").floor, Some((5, None)));
        assert_eq!(p("▪️ Поверх — 4-й").floor, Some((4, None)));
        assert_eq!(p(r"Поверх 9\10(технічний поверх є)").floor, Some((9, Some(10))));
        assert_eq!(p("Поверх: 0").floor, None);
        assert_eq!(p("двоповерховий будинок").floor, None);
        assert_eq!(p("в 9-поверховому будинку").floor, None);
    }

    #[test]
    fn heating_seller_area() {
        assert_eq!(p("Індивідуальне газове опалення").heating, Heating::Individual);
        assert_eq!(p("▪️Опалення - індивідуальне").heating, Heating::Individual);
        assert_eq!(p("Тип опалення: від будинкової котельні.").heating, Heating::Central);
        assert_eq!(p("Опалення центральне.").heating, Heating::Central);
        assert_eq!(p("🔥 електроопалення").heating, Heating::Individual);
        assert_eq!(p("техніка,індувідуальне опалення").heating, Heating::Individual);
        assert_eq!(p("Підігрів підлоги від центрального опалення").heating, Heating::Central);
        assert_eq!(p("#Влаcник").seller, Seller::Owner); // Latin "c", as in real posts
        assert_eq!(p("Пропозиція від АН").seller, Seller::Agency);
        assert_eq!(p("6/9 поверх 47 кв м").area, Some(47.0));
        assert_eq!(p("📐 43,1 м²").area, Some(43.1));
    }

    #[test]
    fn phones() {
        assert_eq!(phone("Володимир АН \n0985211717").as_deref(), Some("+380985211717"));
        assert_eq!(phone("📞 097-13-12-911 Іванка").as_deref(), Some("+380971312911"));
        assert_eq!(phone("📲Пропозиція від АН \n+380 63 133 8828").as_deref(), Some("+380631338828"));
        assert_eq!(phone("☎️Деталі:\n0 50 066 24 92— Роман").as_deref(), Some("+380500662492"));
        assert_eq!(phone("(067) 310-23-32").as_deref(), Some("+380673102332"));
        assert_eq!(phone("+380938834***Здано✅").as_deref(), None);
        assert_eq!(phone("066-22-8***").as_deref(), None);
        assert_eq!(phone("Ціна 15000 грн, 47 кв м").as_deref(), None);
    }

    /// Spellings found in 30 days of @orendakvartyr_ua (Nov 2026) that the first parser missed.
    #[test]
    fn real_channel_spellings() {
        // floor
        assert_eq!(p("✔️2 поверх").floor, Some((2, None)));
        assert_eq!(p("Третій поверх, площа 45 м2").floor, Some((3, None)));
        assert_eq!(p("Оренда Чешка 2 кім на вул. Під Голоском, 52 кв.м., 4/5ц, ремонт").floor, Some((4, Some(5))));
        assert_eq!(p("36 м.кв., 12/12 ц., сучасний ремонт").floor, Some((12, Some(12))));
        assert_eq!(p("60м.кв. 2п./3ц.").floor, Some((2, Some(3))));
        assert_eq!(p("ЖК Авалон на пр. Чорновола 16Ж. 14п/15.").floor, Some((14, Some(15))));
        assert_eq!(p("- 2/5 повер буд").floor, Some((2, Some(5))));
        assert_eq!(p("Характеристики:\n🏗️ 3/3 (парадний вхід)\n▫️ кухня").floor, Some((3, Some(3))));
        assert_eq!(p("- Індукційна варильна поверхня").floor, None);
        // rooms
        assert_eq!(p("Здається простора 2-ох кімнатна квартира").rooms, Some(2));
        assert_eq!(p("Оренда 1но кімнатної кварти в новобудові").rooms, Some(1));
        assert_eq!(p("• 3 ізольовані кімнати").rooms, Some(3));
        assert_eq!(p("Оренда 2-к квартири в особняку").rooms, Some(2));
        assert_eq!(p("🔹 Кухня-студія + окрема спальня з балконом").rooms, Some(1));
        assert_eq!(p("— Планування: кухня-вітальня, 3 спальні, 2 санвузли").rooms, Some(3));
        let s = p("Пропонується в оренду сучасна квартира студійного типу");
        assert_eq!((s.rooms, s.studio), (Some(1), true));
        // heating
        assert_eq!(p("Будинкове опалення. Підігрів підлоги.").heating, Heating::Central);
        assert_eq!(p("— Опалення: будинкове").heating, Heating::Central);
        assert_eq!(p("Централізоване опалення").heating, Heating::Central);
        assert_eq!(p("✔️ Опалення: дахова газова котельня — економне;").heating, Heating::Central);
        assert_eq!(p("• газове опалення").heating, Heating::Individual);
        assert_eq!(p("- Пічне газове опалення").heating, Heating::Individual);
        assert_eq!(p("3/3,опалення конвектор, пральна машинка").heating, Heating::Individual);
        assert_eq!(p("опалення автономне,1пов").heating, Heating::Individual);
        // area
        assert_eq!(p("Загальна площа 87 кв \\м. Квартира").area, Some(87.0));
        assert_eq!(p("Площа: 80м").area, Some(80.0));
        assert_eq!(p("- 56кв2").area, Some(56.0));
        assert_eq!(p("Загальна площа - 45").area, Some(45.0));
        assert_eq!(p("Квартира 40 м на 7 поверсі з 11").area, Some(40.0));
        assert_eq!(p("Кухня: 15 м²\nПлоща: 120 м²").area, Some(120.0));
        assert_eq!(p("зупинка 300 м, багато паркомісць").area, None);
        // price
        assert_eq!(p("💸 17,000 грн").price, Some(Price { amount: 17000, currency: Currency::Uah }));
        // not a flat
        assert!(p("🏠 ОРЕНДА КІМНАТИ | ПІДСЕЛЕННЯ ДО ВЛАСНИЦІ").not_flat);
        assert!(p("Оренда сучасного МАФу на вул. Наукова, 48").not_flat);
        assert!(p("Оренда двох приміщень по 10 м2").not_flat);
    }

    #[test]
    fn not_flat_and_rented() {
        assert!(p("Оренда приміщення на вул. Богдана Хмельницького").not_flat);
        assert!(p("Оренда просторого будинку у Львові, Рясне-1").not_flat);
        assert!(!p("Оренда 2-кімнатної квартири\nЦегляний будинок").not_flat);
        assert!(p("+380938834***Здано✅").rented);
        assert!(p("Наразі стоп 🛑").rented);
    }
}
