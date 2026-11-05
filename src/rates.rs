//! PrivatBank cash exchange rates, to compare $ / € prices with the UAH cap.
//! Uses the bank's sell rate: what it costs you to buy the dollars for rent.

use crate::parse::{Currency, Price};
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub usd: f64,
    pub eur: f64,
}

#[derive(Deserialize)]
struct Row {
    ccy: String,
    /// Bank sells currency at this rate, e.g. "46.10000".
    sale: String,
}

impl Rates {
    pub async fn fetch(http: &reqwest::Client) -> Result<Self> {
        // coursid=5: cash rates in branches.
        let rows: Vec<Row> = http
            .get("https://api.privatbank.ua/p24api/pubinfo?exchange&coursid=5")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let get = |ccy: &str| -> Result<f64> {
            let r = rows.iter().find(|r| r.ccy == ccy).with_context(|| format!("PrivatBank: no {ccy}"))?;
            r.sale.parse().with_context(|| format!("PrivatBank: bad {ccy} rate {:?}", r.sale))
        };
        Ok(Rates { usd: get("USD")?, eur: get("EUR")? })
    }

    pub fn to_uah(&self, p: Price) -> u32 {
        let rate = match p.currency {
            Currency::Uah => 1.0,
            Currency::Usd => self.usd,
            Currency::Eur => self.eur,
        };
        (p.amount as f64 * rate).round() as u32
    }
}
