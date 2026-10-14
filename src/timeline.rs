//! One refresh pass: snapshot + registry + transcripts -> a `View` to draw.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::herdr::Snapshot;
use crate::parse::{Parser, SkillEvent};
use crate::registry::{Registry, Store};
use crate::transcript::{Tail, find_transcript};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Skill { name: String, at: DateTime<Utc> },
    NewSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub pane_id: String,
    pub title: Option<String>,
    /// Newest first. Empty means "(no skills yet)".
    pub rows: Vec<Row>,
}

impl Group {
    pub fn latest(&self) -> Option<DateTime<Utc>> {
        self.rows.iter().find_map(|r| match r {
            Row::Skill { at, .. } => Some(*at),
            Row::NewSession => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Workspace label.
    pub title: String,
    pub status: Status,
    pub groups: Vec<Group>,
}

/// Rows for one pane: all sessions' events merged newest first, with a
/// `NewSession` divider wherever consecutive rows come from different
/// sessions. `sessions` is in registry order (first seen first). An event
/// already present in an earlier session (same name and timestamp) is a copy
/// made by a continued/forked session and is dropped.
pub fn group_rows(sessions: &[Vec<SkillEvent>]) -> Vec<Row> {
    let mut owner: HashMap<(&str, DateTime<Utc>), usize> = HashMap::new();
    let mut merged: Vec<(usize, &SkillEvent)> = vec![];
    for (session, events) in sessions.iter().enumerate() {
        for event in events {
            let first = *owner
                .entry((&event.name, event.timestamp))
                .or_insert(session);
            if first == session {
                merged.push((session, event));
            }
        }
    }
    // Stable sort, then reverse: newest first, and same-timestamp events
    // keep "later in the transcript is newer".
    merged.sort_by_key(|(_, e)| e.timestamp);

    let mut rows = vec![];
    let mut previous = None;
    for (session, event) in merged.into_iter().rev() {
        if previous.is_some_and(|p| p != session) {
            rows.push(Row::NewSession);
        }
        previous = Some(session);
        rows.push(Row::Skill {
            name: event.name.clone(),
            at: event.timestamp,
        });
    }
    rows
}

/// Most recent activity first; panes with no skills last, by pane id.
pub fn sort_groups(groups: &mut [Group]) {
    groups.sort_by(|a, b| {
        b.latest()
            .cmp(&a.latest())
            .then_with(|| a.pane_id.cmp(&b.pane_id))
    });
}

/// Parsed events per session, read incrementally across passes.
pub struct Feeds {
    projects_dir: PathBuf,
    feeds: HashMap<String, Feed>,
}

#[derive(Default)]
struct Feed {
    tail: Option<Tail>,
    parser: Parser,
    events: Vec<SkillEvent>,
}

impl Feeds {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            feeds: HashMap::new(),
        }
    }

    /// All events for `session_id` so far, reading any newly appended lines.
    /// A transcript that doesn't exist yet yields no events and is looked
    /// for again next time.
    pub fn events(&mut self, session_id: &str) -> &[SkillEvent] {
        let feed = self.feeds.entry(session_id.to_string()).or_default();
        if feed.tail.is_none() {
            feed.tail = find_transcript(&self.projects_dir, session_id).map(Tail::new);
        }
        if let Some(tail) = feed.tail.as_mut()
            && let Ok(lines) = tail.read_new_lines()
        {
            for line in lines {
                feed.events.extend(feed.parser.feed(&line));
            }
        }
        &feed.events
    }
}

/// State carried between refresh passes of the TUI.
pub struct App {
    own_pane: String,
    store: Store,
    feeds: Feeds,
    last_view: Option<View>,
}

impl App {
    pub fn new(own_pane: String, store: Store, feeds: Feeds) -> Self {
        Self {
            own_pane,
            store,
            feeds,
            last_view: None,
        }
    }

    /// One pass. On a failed snapshot, or one that doesn't contain this
    /// pane, keeps the last good groups, marks the view unreachable, and
    /// does not touch the registry.
    pub fn refresh(&mut self, snapshot: Result<Snapshot, String>, now: DateTime<Utc>) -> View {
        let view = snapshot.ok().and_then(|snap| {
            let ws = snap.workspace_of(&self.own_pane)?.to_string();
            Some(self.build(&snap, &ws, now))
        });
        match view {
            Some(view) => {
                self.last_view = Some(view.clone());
                view
            }
            None => {
                let mut stale = self.last_view.clone().unwrap_or(View {
                    title: String::new(),
                    status: Status::Ok,
                    groups: vec![],
                });
                stale.status = Status::Unreachable;
                stale
            }
        }
    }

    fn build(&mut self, snap: &Snapshot, workspace_id: &str, now: DateTime<Utc>) -> View {
        let all_claude = snap.claude_panes();
        let live = snap.live_pane_ids();
        let record_all = |r: &mut Registry| {
            let mut changed = false;
            for pane in &all_claude {
                if let Some(session) = &pane.session_id {
                    changed |= r.record(&pane.pane_id, session, now);
                }
            }
            changed | r.prune(&live)
        };
        // If the state file can't be used, fall back to this pass's sessions
        // so the current session still shows.
        let registry = self.store.update(record_all).unwrap_or_else(|e| {
            eprintln!("herdr-skill-timeline: state file unavailable: {e}");
            let mut r = Registry::default();
            record_all(&mut r);
            r
        });

        let mut groups: Vec<Group> = snap
            .claude_panes_in(workspace_id)
            .into_iter()
            .map(|pane| {
                let sessions: Vec<Vec<SkillEvent>> = registry
                    .sessions_for(&pane.pane_id)
                    .iter()
                    .map(|s| self.feeds.events(&s.session_id).to_vec())
                    .collect();
                Group {
                    rows: group_rows(&sessions),
                    pane_id: pane.pane_id,
                    title: pane.title,
                }
            })
            .collect();
        sort_groups(&mut groups);

        View {
            title: snap
                .workspace_label(workspace_id)
                .unwrap_or(workspace_id)
                .to_string(),
            status: Status::Ok,
            groups,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{AgentSession, parse_snapshot};
    use crate::parse::Via;
    use std::fs;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn ev(name: &str, at: &str) -> SkillEvent {
        SkillEvent {
            name: name.into(),
            timestamp: t(at),
            via: Via::Tool,
        }
    }

    fn skill(name: &str, at: &str) -> Row {
        Row::Skill {
            name: name.into(),
            at: t(at),
        }
    }

    #[test]
    fn rows_newest_first_with_session_dividers() {
        let sessions = vec![
            vec![
                ev("brainstorming", "2026-10-14T10:00:00Z"),
                ev("writing-plans", "2026-10-14T11:00:00Z"),
            ],
            vec![], // a session with no skills adds no divider
            vec![ev("tdd", "2026-10-14T12:00:00Z")],
        ];
        assert_eq!(
            group_rows(&sessions),
            vec![
                skill("tdd", "2026-10-14T12:00:00Z"),
                Row::NewSession,
                skill("writing-plans", "2026-10-14T11:00:00Z"),
                skill("brainstorming", "2026-10-14T10:00:00Z"),
            ]
        );
        assert!(group_rows(&[vec![], vec![]]).is_empty());
    }

    #[test]
    fn continued_session_does_not_repeat_copied_history() {
        // A continued/forked session's transcript starts with a copy of the
        // earlier session's history: same skills, same timestamps.
        let sessions = vec![
            vec![
                ev("brainstorming", "2026-10-14T10:00:00Z"),
                ev("writing-plans", "2026-10-14T11:00:00Z"),
            ],
            vec![
                ev("brainstorming", "2026-10-14T10:00:00Z"),
                ev("writing-plans", "2026-10-14T11:00:00Z"),
                ev("tdd", "2026-10-14T12:00:00Z"),
            ],
        ];
        assert_eq!(
            group_rows(&sessions),
            vec![
                skill("tdd", "2026-10-14T12:00:00Z"),
                Row::NewSession,
                skill("writing-plans", "2026-10-14T11:00:00Z"),
                skill("brainstorming", "2026-10-14T10:00:00Z"),
            ]
        );
    }

    #[test]
    fn resumed_older_session_with_newest_skill_comes_first() {
        // A ran brainstorming, /clear started B (writing-plans), then
        // /resume went back to A, which ran tdd.
        let sessions = vec![
            vec![
                ev("brainstorming", "2026-10-14T10:00:00Z"),
                ev("tdd", "2026-10-14T12:00:00Z"),
            ],
            vec![ev("writing-plans", "2026-10-14T11:00:00Z")],
        ];
        let rows = group_rows(&sessions);
        assert_eq!(
            rows,
            vec![
                skill("tdd", "2026-10-14T12:00:00Z"),
                Row::NewSession,
                skill("writing-plans", "2026-10-14T11:00:00Z"),
                Row::NewSession,
                skill("brainstorming", "2026-10-14T10:00:00Z"),
            ]
        );
        let group = Group {
            pane_id: "p".into(),
            title: None,
            rows,
        };
        assert_eq!(group.latest(), Some(t("2026-10-14T12:00:00Z")));
    }

    #[test]
    fn same_skill_twice_in_one_session_is_kept() {
        let sessions = vec![vec![
            ev("simplify", "2026-10-14T10:00:00Z"),
            ev("simplify", "2026-10-14T10:00:00Z"),
        ]];
        assert_eq!(group_rows(&sessions).len(), 2);
    }

    #[test]
    fn groups_sorted_by_latest_then_empty_by_id() {
        let g = |id: &str, rows: Vec<Row>| Group {
            pane_id: id.into(),
            title: None,
            rows,
        };
        let mut groups = vec![
            g("p3", vec![]),
            g("p1", vec![skill("a", "2026-10-14T10:00:00Z")]),
            g("p0", vec![]),
            g("p2", vec![skill("b", "2026-10-14T11:00:00Z")]),
        ];
        sort_groups(&mut groups);
        let ids: Vec<_> = groups.iter().map(|g| g.pane_id.as_str()).collect();
        assert_eq!(ids, vec!["p2", "p1", "p0", "p3"]);
    }

    struct Env {
        _dir: tempfile::TempDir,
        projects: PathBuf,
        app: App,
        state: PathBuf,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        fs::create_dir_all(projects.join("-repo")).unwrap();
        fs::write(
            projects.join("-repo/sess-1.jsonl"),
            include_str!("../tests/fixtures/transcript.jsonl"),
        )
        .unwrap();
        let state = dir.path().join("state");
        let app = App::new(
            "w1:p3".into(),
            Store::new(state.clone()),
            Feeds::new(projects.clone()),
        );
        Env {
            _dir: dir,
            projects,
            app,
            state,
        }
    }

    fn snapshot() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn refresh_builds_groups_for_own_workspace() {
        let mut e = env();
        let view = e.app.refresh(Ok(snapshot()), Utc::now());
        assert_eq!(view.title, "core-8920-store-blockers");
        assert_eq!(view.status, Status::Ok);
        let ids: Vec<_> = view.groups.iter().map(|g| g.pane_id.as_str()).collect();
        // p1 has skills; p2/p5 are Claude with no usable session; w2 excluded;
        // codex pane p4 and our own pane p3 excluded.
        assert_eq!(ids, vec!["w1:p1", "w1:p2", "w1:p5"]);
        let p1 = &view.groups[0];
        assert_eq!(p1.title.as_deref(), Some("store blockers"));
        assert_eq!(p1.rows.len(), 5);
        // Same-timestamp skills keep file order, so the later one is newest.
        assert_eq!(p1.rows[0], skill("simplify", "2026-10-14T21:50:00.000Z"));
        assert_eq!(
            p1.rows[1],
            skill(
                "superpowers:test-driven-development",
                "2026-10-14T21:50:00.000Z"
            )
        );
        assert!(view.groups[1].rows.is_empty());
    }

    #[test]
    fn history_survives_session_change() {
        let mut e = env();
        e.app.refresh(Ok(snapshot()), Utc::now());

        // `/clear` in p1: herdr now reports sess-2.
        fs::write(
            e.projects.join("-repo/sess-2.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-10-14T23:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_Z","name":"Skill","input":{"skill":"verification-before-completion"}}]}}
"#,
        )
        .unwrap();
        let mut snap = snapshot();
        snap.agents[0].agent_session = Some(AgentSession {
            kind: "id".into(),
            value: "sess-2".into(),
        });
        let view = e.app.refresh(Ok(snap), Utc::now());
        let rows = &view.groups[0].rows;
        assert_eq!(
            rows[0],
            skill("verification-before-completion", "2026-10-14T23:00:00Z")
        );
        assert_eq!(rows[1], Row::NewSession);
        assert_eq!(rows.len(), 7);
    }

    #[test]
    fn transcript_appearing_later_is_picked_up() {
        let mut e = env();
        let mut snap = snapshot();
        snap.agents[0].agent_session = Some(AgentSession {
            kind: "id".into(),
            value: "sess-late".into(),
        });
        let view = e.app.refresh(Ok(snap.clone()), Utc::now());
        assert!(view.groups.iter().all(|g| g.rows.is_empty()));
        fs::write(
            e.projects.join("-repo/sess-late.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-10-14T23:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_L","name":"Skill","input":{"skill":"late"}}]}}
"#,
        )
        .unwrap();
        let view = e.app.refresh(Ok(snap), Utc::now());
        assert_eq!(
            view.groups[0].rows,
            vec![skill("late", "2026-10-14T23:00:00Z")]
        );
    }

    #[test]
    fn unreachable_keeps_last_view_and_registry() {
        let mut e = env();
        let good = e.app.refresh(Ok(snapshot()), Utc::now());

        let failed = e.app.refresh(Err("boom".into()), Utc::now());
        assert_eq!(failed.status, Status::Unreachable);
        assert_eq!(failed.groups, good.groups);

        // A snapshot without our own pane (e.g. herdr mid-restart, empty
        // pane list) must not prune the history.
        let mut empty = snapshot();
        empty.panes.clear();
        empty.agents.clear();
        let view = e.app.refresh(Ok(empty), Utc::now());
        assert_eq!(view.status, Status::Unreachable);
        let registry = Store::new(e.state.clone()).update(|_| false).unwrap();
        assert_eq!(registry.sessions_for("w1:p1").len(), 1);
    }

    #[test]
    fn unreachable_before_any_good_pass() {
        let mut e = env();
        let view = e.app.refresh(Err("boom".into()), Utc::now());
        assert_eq!(view.status, Status::Unreachable);
        assert!(view.groups.is_empty());
    }
}
