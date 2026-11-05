//! Runtime state without a database: one JSON file with the last message id seen per source.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    /// Source username -> highest message id already handled.
    pub last_seen: HashMap<String, i64>,
    /// Fingerprints of recently sent posts: channels repost the same flat again and again.
    #[serde(default)]
    pub sent: VecDeque<u64>,
    #[serde(skip)]
    path: PathBuf,
}

const SENT_CAP: usize = 5000;

/// Stable (FNV-1a) hash of the post's letters and digits only, so emoji / spacing edits don't matter.
pub fn fingerprint(text: &str) -> u64 {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| {
            let mut b = [0; 4];
            c.encode_utf8(&mut b).as_bytes().to_vec()
        })
        .fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

impl State {
    /// True if this text was sent before; otherwise remembers it.
    pub fn seen_before(&mut self, text: &str) -> bool {
        let f = fingerprint(text);
        if self.sent.contains(&f) {
            return true;
        }
        self.sent.push_back(f);
        if self.sent.len() > SENT_CAP {
            self.sent.pop_front();
        }
        false
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let mut s: State = match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))?,
            Err(_) => State::default(),
        };
        s.path = path;
        Ok(s)
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(self)?)
    }
}

/// Write via temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}
