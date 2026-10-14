//! `record` mode: the herdr event hook. Saves the event's pane's current
//! Claude session so history survives `/clear` even while the TUI is closed.
//! Always exits 0 and prints nothing to stdout.

use std::env;
use std::io;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::herdr::{Snapshot, fetch_snapshot};
use crate::registry::Store;

pub fn run() {
    let Ok(json) = env::var("HERDR_PLUGIN_EVENT_JSON") else {
        return;
    };
    let Some(pane_id) = pane_id_from_event(&json) else {
        return;
    };
    let result = fetch_snapshot().and_then(|snap| {
        record_pane(&pane_id, &snap, &Store::default_location(), Utc::now())
            .map_err(|e| e.to_string())
    });
    if let Err(e) = result {
        eprintln!("herdr-skill-timeline record: {e}");
    }
}

/// `data.pane_id` from a herdr plugin event payload.
pub fn pane_id_from_event(json: &str) -> Option<String> {
    let event: Value = serde_json::from_str(json).ok()?;
    event
        .pointer("/data/pane_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Records the pane's session if it is a Claude pane with a known session.
/// Returns true if the registry changed. Non-Claude panes never touch disk.
pub fn record_pane(
    pane_id: &str,
    snap: &Snapshot,
    store: &Store,
    now: DateTime<Utc>,
) -> io::Result<bool> {
    let Some(session_id) = snap
        .claude_panes()
        .into_iter()
        .find(|p| p.pane_id == pane_id)
        .and_then(|p| p.session_id)
    else {
        return Ok(false);
    };
    let mut changed = false;
    store.update(|r| {
        changed = r.record(pane_id, &session_id, now);
        changed
    })?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::parse_snapshot;

    fn snapshot() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn extracts_pane_id() {
        assert_eq!(
            pane_id_from_event(
                r#"{"event":"pane.agent_detected","data":{"pane_id":"w1:p1","agent":"claude"}}"#
            ),
            Some("w1:p1".into())
        );
        assert_eq!(pane_id_from_event(r#"{"data":{}}"#), None);
        assert_eq!(pane_id_from_event(r#"{"data":{"pane_id":""}}"#), None);
        assert_eq!(pane_id_from_event("garbage"), None);
    }

    #[test]
    fn records_claude_pane_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        assert!(record_pane("w1:p1", &snapshot(), &store, Utc::now()).unwrap());
        assert!(!record_pane("w1:p1", &snapshot(), &store, Utc::now()).unwrap());
        let r = store.update(|_| false).unwrap();
        assert_eq!(r.sessions_for("w1:p1")[0].session_id, "sess-1");
    }

    #[test]
    fn ignores_non_claude_and_sessionless_panes_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let store = Store::new(state.clone());
        for pane in ["w1:p4", "w1:p2", "w1:p3", "missing"] {
            assert!(!record_pane(pane, &snapshot(), &store, Utc::now()).unwrap());
        }
        assert!(!state.exists());
    }
}
