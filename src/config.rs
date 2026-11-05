//! `config.toml`: what to watch and what passes the filter. Secrets live in env, not here.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub filter: Filter,
    pub sources: Sources,
}

#[derive(Deserialize, Debug)]
pub struct Filter {
    pub max_price_uah: u32,
    pub rooms: Vec<u8>,
    /// Floors 1..=max_floor, never the top floor of the building.
    pub max_floor: u8,
}

#[derive(Deserialize, Debug)]
pub struct Sources {
    /// Channel usernames without `@`.
    pub channels: Vec<String>,
    /// Bot usernames without `@`.
    pub bots: Vec<String>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
        toml::from_str(&raw).with_context(|| format!("parse {path}"))
    }

    pub fn summary(&self) -> String {
        let f = &self.filter;
        format!(
            "Фільтр: до {} грн, кімнат {:?}, поверх 1–{} (не останній).\nДжерела: канали {:?}, боти {:?}",
            f.max_price_uah, f.rooms, f.max_floor, self.sources.channels, self.sources.bots
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn repo_config_parses() {
        let cfg = super::Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/config.toml")).unwrap();
        assert_eq!(cfg.filter.max_price_uah, 25000);
        assert_eq!(cfg.filter.rooms, vec![1, 2]);
    }
}
