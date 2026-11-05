//! One rent offer as read from a source, and its short notification card.

use crate::parse::{Currency, Heating, Parsed, Seller};
use crate::rates::Rates;
use crate::zone::Place;

#[derive(Debug, Clone)]
pub struct Listing {
    /// Telegram message id inside the source.
    pub id: i32,
    /// Link to the original post (bot chats have no per-message link: the bot's page).
    pub url: String,
    pub text: String,
    /// Album photos (bot sources only; channel posts rely on the link preview).
    pub photos: Vec<grammers_client::media::Photo>,
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
    /// Unified card, in the style of @homierentbot:
    /// ```text
    /// 💰 550 $ ≈ 25 355 грн
    /// 📍 Франківський р-н · вул. Коновальця
    /// 🏠 2 кімн. · 47 м² · поверх 6 з 9
    /// 🔥 індивідуальне опалення · 👤 власник
    /// ❔ не вказано: …
    /// 🔗 https://t.me/orendakvartyr_ua/247032
    /// ```
    pub fn card(&self, p: &Parsed, place: &Place, rates: Option<Rates>) -> String {
        let mut missing = Vec::new();
        let mut lines = Vec::new();

        match p.price {
            Some(pr) if pr.currency == Currency::Uah => lines.push(format!("💰 {} грн", thousands(pr.amount))),
            Some(pr) => {
                let sym = if pr.currency == Currency::Usd { "$" } else { "€" };
                let uah = rates.map(|r| format!(" ≈ {} грн", thousands(r.to_uah(pr)))).unwrap_or_default();
                lines.push(format!("💰 {} {sym}{uah}", thousands(pr.amount)));
            }
            None => missing.push("ціна"),
        }

        lines.push(match place {
            Place::Street(s) => format!("📍 Франківський р-н · вул. {s}"),
            Place::DistrictOnly => "📍 Франківський р-н · ⚠️ вулицю не вказано".to_string(),
            Place::Outside => "📍 ⛔ поза зоною".to_string(), // only in unfiltered test cards
        });

        let mut flat = Vec::new();
        match (p.studio, p.rooms) {
            (true, _) => flat.push("студія".to_string()),
            (false, Some(r)) => flat.push(format!("{r} кімн.")),
            (false, None) => missing.push("кімнати"),
        }
        if let Some(a) = p.area {
            flat.push(format!("{a} м²"));
        }
        match p.floor {
            Some((f, Some(t))) => flat.push(format!("поверх {f} з {t}")),
            Some((f, None)) => {
                flat.push(format!("поверх {f}"));
                missing.push("поверховість");
            }
            None => missing.push("поверх"),
        }
        if !flat.is_empty() {
            lines.push(format!("🏠 {}", flat.join(" · ")));
        }

        let mut extra = Vec::new();
        match p.heating {
            Heating::Individual => extra.push("🔥 індивідуальне опалення"),
            Heating::Central => extra.push("🏢 центральне опалення"),
            Heating::Unknown => missing.push("опалення"),
        }
        match p.seller {
            Seller::Owner => extra.push("👤 власник"),
            Seller::Agency => extra.push("🤝 рієлтор"),
            Seller::Unknown => {}
        }
        if !extra.is_empty() {
            lines.push(extra.join(" · "));
        }
        if !missing.is_empty() {
            lines.push(format!("❔ не вказано: {}", missing.join(", ")));
        }
        lines.push(format!("🔗 {}", self.url));
        lines.join("
")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::zone::place;

    #[test]
    fn card_format() {
        let text = "Оренда 2-кімн, вул. Коновальця 5\n6/9 поверх 47 кв м\nІндивідуальне опалення\nЦіна 550$\n#Власник";
        let l = Listing { id: 7, url: "https://t.me/c/7".into(), text: text.into(), photos: vec![] };
        let card = l.card(&parse(text), &place(text), Some(Rates { usd: 45.0, eur: 50.0 }));
        assert_eq!(
            card,
            "💰 550 $ ≈ 24 750 грн\n📍 Франківський р-н · вул. Коновальця\n🏠 2 кімн. · 47 м² · поверх 6 з 9\n🔥 індивідуальне опалення · 👤 власник\n🔗 https://t.me/c/7"
        );
    }

    #[test]
    fn card_from_homierentbot() {
        // Real @homierentbot message (shortened).
        let text = "450 $\nФранківський район · вул. Героїв УПА Перша\nКімнат — 1\nПлоща — 20 м² (кухня 3 м²)\n\
                    Поверх — 5 з 5\n\n🐾 БЕЗ тварин\n🔥 електроопалення\n🧺 пральна машина\n\nРієлтор\nOLX";
        let l = Listing { id: 1, url: "https://www.olx.ua/d/uk/obyavlenie/x.html".into(), text: text.into(), photos: vec![] };
        let card = l.card(&parse(text), &place(text), Some(Rates { usd: 45.0, eur: 50.0 }));
        assert_eq!(
            card,
            "💰 450 $ ≈ 20 250 грн\n📍 Франківський р-н · вул. Героїв УПА\n🏠 1 кімн. · 20 м² · поверх 5 з 5\n\
             🔥 індивідуальне опалення · 🤝 рієлтор\n🔗 https://www.olx.ua/d/uk/obyavlenie/x.html"
        );
    }

    #[test]
    fn card_flags_missing() {
        let text = "Здам 1к, Франківський район";
        let l = Listing { id: 7, url: "https://t.me/c/7".into(), text: text.into(), photos: vec![] };
        let card = l.card(&parse(text), &place(text), None);
        assert_eq!(
            card,
            "📍 Франківський р-н · ⚠️ вулицю не вказано\n🏠 1 кімн.\n❔ не вказано: ціна, поверх, опалення\n🔗 https://t.me/c/7"
        );
    }
}
