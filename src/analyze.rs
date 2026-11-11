//! `analyze <sample.json>`: run the parser over a `backfill --dump` sample and report how often each
//! field is found. Misses go to `state/misses-<field>.json` for tuning the rules. Offline, no Telegram.

use crate::{filter, parse, zone};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

pub fn run(sample: &str, state_dir: &Path) -> Result<()> {
    let raw = std::fs::read(sample).with_context(|| format!("read {sample}"))?;
    let posts: Vec<(i64, String, String)> = serde_json::from_slice(&raw).context("sample: [id, url, text] list")?;

    let mut why: BTreeMap<&str, usize> = BTreeMap::new();
    let mut misses: BTreeMap<&str, Vec<(i64, String)>> = BTreeMap::new();
    let mut offers = 0;
    for (id, _url, text) in &posts {
        let p = parse::parse(text);
        let kind = if filter::is_seeker(text) {
            "seeker"
        } else if p.rented {
            "rented"
        } else if p.not_flat {
            "not flat"
        } else if p.price.is_none() && p.rooms.is_none() && p.area.is_none() && p.floor.is_none() {
            "chatter"
        } else {
            "offer"
        };
        *why.entry(kind).or_default() += 1;
        if kind != "offer" {
            continue;
        }
        offers += 1;
        let place = zone::place(text);
        let found = [
            ("price", p.price.is_some()),
            ("rooms", p.rooms.is_some()),
            ("floor", p.floor.is_some()),
            ("floors_total", p.floor.is_some_and(|(_, t)| t.is_some())),
            ("area", p.area.is_some()),
            ("heating", p.heating != parse::Heating::Unknown),
            ("seller", p.seller != parse::Seller::Unknown),
            ("phone", parse::phone(text).is_some()),
            ("address", place != zone::Place::Outside || zone::has_address(text)),
        ];
        for (field, ok) in found {
            if !ok {
                misses.entry(field).or_default().push((*id, text.chars().take(600).collect()));
            }
        }
    }

    println!("{} posts: {why:?}", posts.len());
    println!("{offers} offers, field found in:");
    for field in ["price", "rooms", "floor", "floors_total", "area", "heating", "seller", "phone", "address"] {
        let miss = misses.get(field).map_or(0, Vec::len);
        let pct = 100.0 * (offers - miss) as f64 / offers.max(1) as f64;
        println!("  {field:<13} {pct:5.1}%  ({miss} missing)");
    }
    for (field, list) in &misses {
        crate::state::write_atomic(&state_dir.join(format!("misses-{field}.json")), &serde_json::to_vec_pretty(list)?)?;
    }

    // Precision check: every parsed value next to the post, for reading by eye.
    let found: Vec<serde_json::Value> = posts
        .iter()
        .filter(|(_, _, text)| !filter::is_seeker(text))
        .filter_map(|(id, _, text)| {
            let p = parse::parse(text);
            let listing = !p.rented && !p.not_flat && (p.price.is_some() || p.rooms.is_some() || p.area.is_some() || p.floor.is_some());
            listing.then(|| {
                serde_json::json!({
                    "id": id,
                    "parsed": format!("{:?} rooms={:?} studio={} floor={:?} area={:?} {:?} {:?} place={:?} phone={:?}",
                        p.price, p.rooms, p.studio, p.floor, p.area, p.heating, p.seller, zone::place(text), parse::phone(text)),
                    "text": text.chars().take(700).collect::<String>(),
                })
            })
        })
        .collect();
    crate::state::write_atomic(&state_dir.join("parsed.json"), &serde_json::to_vec_pretty(&found)?)?;
    Ok(())
}
