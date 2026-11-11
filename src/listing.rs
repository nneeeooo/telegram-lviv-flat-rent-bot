//! One rent offer as read from a source, and its short notification card.

use crate::parse::{Currency, Heating, Parsed, Seller};
use crate::rates::Rates;
use crate::zone::Place;

#[derive(Debug, Clone)]
pub enum Photo {
    /// Photo seen by the user account: downloaded, then uploaded by the bot.
    Telegram(grammers_client::media::Photo),
    /// Public image URL: Telegram fetches it itself.
    Url(String),
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// Id inside the source (Telegram message id, LUN page id).
    pub id: i64,
    /// Link to the original post (bot chats have no per-message link: the bot's page).
    pub url: String,
    pub text: String,
    /// Album photos (bot and LUN sources; channel posts rely on the link preview).
    pub photos: Vec<Photo>,
    /// Structured sources (LUN) fill these; free-text posts are parsed from `text`.
    pub facts: Option<Parsed>,
    pub place: Option<Place>,
    pub phone: Option<String>,
    /// Dedupe key; default is the post text.
    pub key: Option<String>,
    /// Source badge on top of the card: "🧢 OLX", "🦊 LUN", "⭐️ Viktor".
    pub source: &'static str,
}

/// Source badge by the listing's own site: dom.ria shows up via @homierentbot and LUN alike.
pub fn badge(url: &str, default: &'static str) -> &'static str {
    if url.contains("dom.ria.com") { "📘 Dom.ria" } else { default }
}

/// Sites that hide the phone behind a click: a missing phone there is not news.
fn hides_phone(url: &str) -> bool {
    url.contains("olx.") || url.contains("dom.ria.com")
}

/// 25000 -> "25 000".
fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

impl Listing {
    /// Unified card in blocks separated by a blank line: source badge, price and place, the flat, who rents and how to
    /// call, the link, then every missing fact on its own line. Empty blocks are left out.
    /// ```text
    /// 🦊 LUN
    ///
    /// 💰 550 $ ≈ 25 355 грн
    /// 📍 вул. Євгена Коновальця · Франківський р-н
    ///
    /// 🏠 2 кімн. · 47 м²
    /// 🪜 поверх 6 з 9
    ///
    /// 👤 власник
    /// 📞 +380…
    ///
    /// 🔗 https://t.me/orendakvartyr_ua/247032
    ///
    /// ❔ не вказано: опалення
    /// ```
    pub fn card(&self, p: &Parsed, place: &Place, rates: Option<Rates>) -> String {
        let (mut head, mut flat, mut contact, mut missing) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());

        match p.price {
            Some(pr) if pr.currency == Currency::Uah => head.push(format!("💰 {} грн", thousands(pr.amount))),
            Some(pr) => {
                let sym = if pr.currency == Currency::Usd { "$" } else { "€" };
                let uah = rates.map(|r| format!(" ≈ {} грн", thousands(r.to_uah(pr)))).unwrap_or_default();
                head.push(format!("💰 {} {sym}{uah}", thousands(pr.amount)));
            }
            None => missing.push("❔ не вказано: ціна"),
        }

        match place {
            Place::Street(s) => head.push(format!("📍 вул. {s} · Франківський р-н")),
            Place::DistrictOnly => {
                head.push("📍 Франківський р-н".to_string());
                missing.push("❔ не вказано: вулиця");
            }
            Place::Outside => head.push("📍 ⛔ поза зоною".to_string()), // only in unfiltered test cards
        }

        let mut rooms = Vec::new();
        match (p.studio, p.rooms) {
            (true, _) => rooms.push("студія 1 кімн.".to_string()),
            (false, Some(r)) => rooms.push(format!("{r} кімн.")),
            (false, None) => missing.push("❔ не вказано: кімнати"),
        }
        match p.area {
            Some(a) => rooms.push(format!("{a} м²")),
            None => missing.push("❔ не вказано: площа"),
        }
        if !rooms.is_empty() {
            flat.push(format!("🏠 {}", rooms.join(" · ")));
        }

        match p.floor {
            Some((f, Some(t))) => flat.push(format!("🪜 поверх {f} з {t}")),
            Some((f, None)) => {
                flat.push(format!("🪜 поверх {f}"));
                missing.push("❔ не вказано: поверховість");
            }
            None => missing.push("❔ не вказано: поверх"),
        }

        match p.heating {
            Heating::Individual => flat.push("🔥 індивідуальне опалення".to_string()),
            Heating::Central => flat.push("🏢 центральне опалення".to_string()),
            Heating::Unknown => missing.push("❔ не вказано: опалення"),
        }

        match p.seller {
            Seller::Owner => contact.push("👤 власник".to_string()),
            Seller::Agency => contact.push("🤝 рієлтор".to_string()),
            Seller::Unknown => missing.push("❔ не вказано: власник чи рієлтор"),
        }

        match &self.phone {
            Some(phone) => contact.push(format!("📞 {phone}")),
            None if !hides_phone(&self.url) => missing.push("❔ не вказано: телефон"),
            None => {}
        }

        let link = vec![format!("🔗 {}", self.url)];
        let missing: Vec<String> = missing.into_iter().map(String::from).collect();
        let source: Vec<String> = [self.source].into_iter().filter(|s| !s.is_empty()).map(String::from).collect();
        [source, head, flat, contact, link, missing]
            .into_iter()
            .filter(|b| !b.is_empty())
            .map(|b| b.join("\n"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::zone::place;

    #[test]
    fn card_format() {
        let text = "Оренда 2-кімн, вул. Коновальця 5\n6/9 поверх 47 кв м\nІндивідуальне опалення\nЦіна 550$\n#Власник\n097-13-12-911";
        let l = Listing {
            id: 7,
            url: "https://t.me/c/7".into(),
            text: text.into(),
            phone: crate::parse::phone(text),
            ..Default::default()
        };
        let card = l.card(&parse(text), &place(text), Some(Rates { usd: 45.0, eur: 50.0 }));
        assert_eq!(
            card,
            "💰 550 $ ≈ 24 750 грн\n📍 вул. Євгена Коновальця · Франківський р-н\n\n\
             🏠 2 кімн. · 47 м²\n🪜 поверх 6 з 9\n🔥 індивідуальне опалення\n\n\
             👤 власник\n📞 +380971312911\n\n\
             🔗 https://t.me/c/7"
        );
    }

    #[test]
    fn card_from_homierentbot() {
        // Real @homierentbot message (shortened).
        let text = "450 $\nФранківський район · вул. Героїв УПА Перша\nКімнат — 1\nПлоща — 20 м² (кухня 3 м²)\n\
                    Поверх — 5 з 5\n\n🐾 БЕЗ тварин\n🔥 електроопалення\n🧺 пральна машина\n\nРієлтор\nOLX";
        let l = Listing { id: 1, url: "https://www.olx.ua/d/uk/obyavlenie/x.html".into(), text: text.into(), ..Default::default() };
        let card = l.card(&parse(text), &place(text), Some(Rates { usd: 45.0, eur: 50.0 }));
        assert_eq!(
            card,
            "💰 450 $ ≈ 20 250 грн\n📍 вул. Героїв УПА · Франківський р-н\n\n\
             🏠 1 кімн. · 20 м²\n🪜 поверх 5 з 5\n🔥 індивідуальне опалення\n\n\
             🤝 рієлтор\n\n\
             🔗 https://www.olx.ua/d/uk/obyavlenie/x.html"
        );
    }

    #[test]
    fn card_starts_with_source_badge() {
        let text = "Оренда 1к, вул. Коновальця 5, 15000 грн";
        let l = Listing { url: "https://t.me/c/7".into(), text: text.into(), source: "⭐️ Viktor", ..Default::default() };
        assert!(l.card(&parse(text), &place(text), None).starts_with("⭐️ Viktor\n\n💰 15 000 грн\n"));
    }

    #[test]
    fn card_flags_missing() {
        let text = "Здам студію, Франківський район";
        let l = Listing { id: 7, url: "https://t.me/c/7".into(), text: text.into(), ..Default::default() };
        let card = l.card(&parse(text), &place(text), None);
        assert_eq!(
            card,
            "📍 Франківський р-н\n\n🏠 студія 1 кімн.\n\n🔗 https://t.me/c/7\n\n\
             ❔ не вказано: ціна\n❔ не вказано: вулиця\n❔ не вказано: площа\n❔ не вказано: поверх\n\
             ❔ не вказано: опалення\n❔ не вказано: власник чи рієлтор\n❔ не вказано: телефон"
        );
    }
}
