//! Reading herdr's live state via `herdr api snapshot`.

use std::collections::BTreeSet;
use std::env;
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub agents: Vec<Agent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSession {
    pub kind: String,
    pub value: String,
}

/// A pane running Claude, as the timeline needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePane {
    pub pane_id: String,
    pub workspace_id: String,
    /// `None` until Claude's SessionStart hook has reported it.
    pub session_id: Option<String>,
    /// Terminal title, `None` when empty.
    pub title: Option<String>,
}

#[derive(Deserialize)]
struct Envelope {
    result: EnvelopeResult,
}

#[derive(Deserialize)]
struct EnvelopeResult {
    snapshot: Snapshot,
}

/// Parses the JSON printed by `herdr api snapshot`.
pub fn parse_snapshot(json: &str) -> Result<Snapshot, String> {
    serde_json::from_str::<Envelope>(json)
        .map(|e| e.result.snapshot)
        .map_err(|e| format!("invalid snapshot json: {e}"))
}

/// Runs `$HERDR_BIN_PATH api snapshot` (falls back to `herdr` on PATH).
pub fn fetch_snapshot() -> Result<Snapshot, String> {
    let bin = env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let output = Command::new(&bin)
        .args(["api", "snapshot"])
        .output()
        .map_err(|e| format!("could not run {bin}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{bin} api snapshot exited with {}", output.status));
    }
    parse_snapshot(&String::from_utf8_lossy(&output.stdout))
}

impl Snapshot {
    pub fn workspace_of(&self, pane_id: &str) -> Option<&str> {
        self.panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .map(|p| p.workspace_id.as_str())
    }

    pub fn workspace_label(&self, workspace_id: &str) -> Option<&str> {
        self.workspaces
            .iter()
            .find(|w| w.workspace_id == workspace_id)
            .map(|w| w.label.as_str())
    }

    pub fn live_pane_ids(&self) -> BTreeSet<String> {
        self.panes.iter().map(|p| p.pane_id.clone()).collect()
    }

    /// Every pane running Claude, across all workspaces.
    pub fn claude_panes(&self) -> Vec<ClaudePane> {
        self.agents
            .iter()
            .filter(|a| a.agent == "claude")
            .map(|a| ClaudePane {
                pane_id: a.pane_id.clone(),
                workspace_id: a.workspace_id.clone(),
                session_id: a
                    .agent_session
                    .as_ref()
                    .filter(|s| s.kind == "id" && !s.value.is_empty())
                    .map(|s| s.value.clone()),
                title: a
                    .terminal_title_stripped
                    .as_ref()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
            })
            .collect()
    }

    pub fn claude_panes_in(&self, workspace_id: &str) -> Vec<ClaudePane> {
        self.claude_panes()
            .into_iter()
            .filter(|p| p.workspace_id == workspace_id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn workspace_lookup() {
        let s = fixture();
        assert_eq!(s.workspace_of("w1:p3"), Some("w1"));
        assert_eq!(s.workspace_of("nope"), None);
        assert_eq!(s.workspace_label("w1"), Some("core-8920-store-blockers"));
        assert_eq!(s.live_pane_ids().len(), 6);
    }

    #[test]
    fn claude_panes_in_workspace() {
        let panes = fixture().claude_panes_in("w1");
        assert_eq!(
            panes,
            vec![
                ClaudePane {
                    pane_id: "w1:p1".into(),
                    workspace_id: "w1".into(),
                    session_id: Some("sess-1".into()),
                    title: Some("store blockers".into()),
                },
                // Claude detected but session not reported yet.
                ClaudePane {
                    pane_id: "w1:p2".into(),
                    workspace_id: "w1".into(),
                    session_id: None,
                    title: None,
                },
                // Session reported as a path rather than an id: ignored.
                ClaudePane {
                    pane_id: "w1:p5".into(),
                    workspace_id: "w1".into(),
                    session_id: None,
                    title: None,
                },
            ]
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_snapshot("not json").is_err());
        assert!(parse_snapshot(r#"{"result":{}}"#).is_err());
    }

    #[test]
    fn tolerates_missing_lists() {
        let s = parse_snapshot(r#"{"result":{"snapshot":{}}}"#).unwrap();
        assert!(s.claude_panes().is_empty());
    }
}
