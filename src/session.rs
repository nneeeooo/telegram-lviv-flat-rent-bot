//! Telegram user session persisted as a JSON file.
//!
//! grammers' built-in persistent storage is SQLite (a C build with clang, painful on Alpine), so we keep
//! a `MemorySession` and save only what a restart needs: the home datacenter and its auth keys.
//! Peers are re-resolved by username on start; update state is unused (we poll, not stream).
//!
//! The file grants full access to the account: keep it out of git, readable only by the owner.

use anyhow::{Context, Result};
use grammers_client::session::storages::MemorySession;
use grammers_client::session::types::DcOption;
use grammers_client::session::{Session, SessionData};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Saved {
    home_dc: i32,
    dc_options: Vec<DcOption>,
}

pub fn load(path: &Path) -> Result<MemorySession> {
    let Ok(raw) = std::fs::read(path) else { return Ok(MemorySession::default()) };
    let saved: Saved = serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))?;
    let mut data = SessionData::default();
    data.home_dc = saved.home_dc;
    data.dc_options.extend(saved.dc_options.into_iter().map(|o| (o.id, o)));
    Ok(MemorySession::from(data))
}

pub fn save(session: &MemorySession, path: &Path) -> Result<()> {
    let home_dc = session.home_dc_id()?;
    let dc_options = (1..=5).filter_map(|id| session.dc_option(id).ok().flatten()).collect();
    let json = serde_json::to_vec(&Saved { home_dc, dc_options })?;
    crate::state::write_atomic(path, &json)
}
