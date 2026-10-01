//! Activity tracking, feeding "last seen" displays.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use wispers_access_sdk::ShareId;

pub struct ActivityStore {
    path: PathBuf,
    /// Share id to milliseconds since the epoch.
    last_connected: Mutex<HashMap<String, u64>>,
}

impl ActivityStore {
    /// Reads the file at `path`, or starts empty: an unreadable file costs
    /// the labels until the shares are next reached, not the app.
    pub fn open(path: PathBuf) -> Self {
        let last_connected = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            last_connected: Mutex::new(last_connected),
        }
    }

    pub fn last_connected(&self, share: &ShareId) -> Option<u64> {
        self.lock().get(share.as_str()).copied()
    }

    pub fn mark_connected(&self, share: &ShareId) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut map = self.lock();
        map.insert(share.to_string(), now);
        self.persist(&map);
    }

    pub fn remove(&self, share: &ShareId) {
        let mut map = self.lock();
        if map.remove(share.as_str()).is_some() {
            self.persist(&map);
        }
    }

    fn persist(&self, map: &HashMap<String, u64>) {
        let written = serde_json::to_vec_pretty(map)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&self.path, json)
            });
        if let Err(e) = written {
            tracing::warn!(error = %e, "could not save when the shares were last reached");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.last_connected.lock().expect("unpoisoned")
    }
}
