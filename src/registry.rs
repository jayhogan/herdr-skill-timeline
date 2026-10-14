//! Which Claude sessions each pane has had, persisted so history survives
//! `/clear` and restarts. Shared by the TUI and the `record` event hook.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub first_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    /// pane id -> sessions, oldest first.
    pub panes: BTreeMap<String, Vec<SessionRecord>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            panes: BTreeMap::new(),
        }
    }
}

impl Registry {
    /// Adds `session_id` to the pane's history if new. Returns true if changed.
    pub fn record(&mut self, pane_id: &str, session_id: &str, now: DateTime<Utc>) -> bool {
        let sessions = self.panes.entry(pane_id.to_string()).or_default();
        if sessions.iter().any(|s| s.session_id == session_id) {
            return false;
        }
        sessions.push(SessionRecord {
            session_id: session_id.to_string(),
            first_seen: now,
        });
        true
    }

    /// Drops panes that no longer exist. Returns true if changed.
    pub fn prune(&mut self, live_pane_ids: &BTreeSet<String>) -> bool {
        let before = self.panes.len();
        self.panes
            .retain(|pane_id, _| live_pane_ids.contains(pane_id));
        self.panes.len() != before
    }

    pub fn sessions_for(&self, pane_id: &str) -> &[SessionRecord] {
        self.panes.get(pane_id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// `sessions.json` on disk, guarded by an advisory lock on `sessions.json.lock`.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `${XDG_STATE_HOME:-~/.local/state}/herdr-skill-timeline`.
    pub fn default_location() -> Self {
        let state_home = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/state")
            });
        Self::new(state_home.join("herdr-skill-timeline"))
    }

    fn file(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    /// Locks, loads the latest registry, applies `change`, writes it back if
    /// `change` returned true, and returns the resulting registry.
    pub fn update(&self, change: impl FnOnce(&mut Registry) -> bool) -> io::Result<Registry> {
        fs::create_dir_all(&self.dir)?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("sessions.json.lock"))?;
        lock.lock()?;
        let mut registry = self.load_unlocked()?;
        if change(&mut registry) {
            let tmp = self.dir.join("sessions.json.tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&registry)?)?;
            fs::rename(&tmp, self.file())?;
        }
        Ok(registry)
        // `lock` is released when dropped here.
    }

    /// Missing file -> empty. Corrupt file -> moved to `sessions.json.bak`, empty.
    fn load_unlocked(&self) -> io::Result<Registry> {
        let bytes = match fs::read(self.file()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Registry::default()),
            Err(e) => return Err(e),
        };
        match serde_json::from_slice(&bytes) {
            Ok(registry) => Ok(registry),
            Err(e) => {
                eprintln!("herdr-skill-timeline: sessions.json is corrupt ({e}); starting fresh");
                fs::rename(self.file(), self.dir.join("sessions.json.bak"))?;
                Ok(Registry::default())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn record_dedups_and_keeps_order() {
        let mut r = Registry::default();
        assert!(r.record("w1:p1", "a", t("2026-10-14T10:00:00Z")));
        assert!(!r.record("w1:p1", "a", t("2026-10-14T11:00:00Z")));
        assert!(r.record("w1:p1", "b", t("2026-10-14T12:00:00Z")));
        let ids: Vec<_> = r
            .sessions_for("w1:p1")
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(
            r.sessions_for("w1:p1")[0].first_seen,
            t("2026-10-14T10:00:00Z")
        );
        assert!(r.sessions_for("other").is_empty());
    }

    #[test]
    fn prune_drops_dead_panes() {
        let mut r = Registry::default();
        r.record("w1:p1", "a", Utc::now());
        r.record("w1:p2", "b", Utc::now());
        let live = BTreeSet::from(["w1:p1".to_string()]);
        assert!(r.prune(&live));
        assert!(!r.prune(&live));
        assert_eq!(r.panes.keys().collect::<Vec<_>>(), vec!["w1:p1"]);
    }

    #[test]
    fn store_round_trip_and_sequential_writers() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"));
        store.update(|r| r.record("p1", "a", Utc::now())).unwrap();
        // A second writer (e.g. the event hook) sees the first writer's data.
        let other = Store::new(dir.path().join("state"));
        let r = other.update(|r| r.record("p2", "b", Utc::now())).unwrap();
        assert_eq!(r.panes.len(), 2);
        let reread = store.update(|_| false).unwrap();
        assert_eq!(reread, r);
    }

    #[test]
    fn concurrent_writers_do_not_lose_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    Store::new(path)
                        .update(|r| r.record(&format!("p{i}"), "s", Utc::now()))
                        .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let r = Store::new(path).update(|_| false).unwrap();
        assert_eq!(r.panes.len(), 8);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("sessions.json"), "{not json").unwrap();
        let r = Store::new(state.clone())
            .update(|r| r.record("p1", "a", Utc::now()))
            .unwrap();
        assert_eq!(r.panes.len(), 1);
        assert_eq!(
            fs::read_to_string(state.join("sessions.json.bak")).unwrap(),
            "{not json"
        );
    }
}
